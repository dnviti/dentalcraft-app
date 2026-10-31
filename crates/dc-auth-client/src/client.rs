//! [`AuthClient`]: the state machine the application talks to.
//!
//! All network and keyring work runs on background threads; the UI thread
//! only calls [`AuthClient::state`] / [`AuthClient::poll`] (cheap, never
//! blocking) and the `begin_` / `try_` / `sign_out` starters.

use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Deserialize;

use crate::browser::{BrowserOpener, SystemBrowser};
use crate::callback::wait_for_callback;
use crate::error::{AuthError, AuthErrorCode, Result};
use crate::http::{Http, Reply};
use crate::jwt::{self, Expect, Jwks};
use crate::oidc::{self, AuthorizeParams, Discovery, Lang, TokenResponse};
use crate::pkce::{Pkce, random_token};
use crate::store::{KeyringTokenStore, TokenStore};

/// The public client id of the desktop app.
pub const DEFAULT_CLIENT_ID: &str = "dentalcraft-desktop";
/// Scopes requested at sign-in.
pub const SCOPES: &str = "openid profile email offline_access";
/// How long the browser step may take before the sign-in is abandoned.
pub const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(300);

const LEEWAY_SECS: u64 = 60;
const EXPIRY_SKEW: Duration = Duration::from_secs(60);
const DEFAULT_TOKEN_TTL: u64 = 3600;
const DISCOVERY_TTL: Duration = Duration::from_secs(3600);
const JWKS_TTL: Duration = Duration::from_secs(3600);
const JWKS_MIN_REFETCH: Duration = Duration::from_secs(10);

/// What the server grants the user.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct Entitlements {
    /// Ids of the modules the user may open, e.g. `"aligners"`.
    #[serde(default)]
    pub modules: Vec<String>,
}

impl Entitlements {
    /// True if the module id is granted.
    pub fn has_module(&self, id: &str) -> bool {
        self.modules.iter().any(|m| m == id)
    }
}

/// The signed-in user, from `GET /api/v1/me`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct UserInfo {
    /// Stable user id (`usr_...`).
    pub sub: String,
    /// E-mail address.
    #[serde(default)]
    pub email: String,
    /// Display name.
    #[serde(default)]
    pub name: String,
    /// Granted modules.
    #[serde(default)]
    pub entitlements: Entitlements,
}

/// Where the sign-in stands. Show it with [`AuthErrorCode::i18n_key`] for errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthState {
    /// Nobody is signed in.
    SignedOut,
    /// The browser was opened on `url`; the app waits for the redirect. Show
    /// the URL so the user can open it by hand if no browser started.
    WaitingForBrowser {
        /// The authorization URL.
        url: String,
    },
    /// Network work in progress: discovery, code exchange, silent restore.
    Working,
    /// Signed in.
    SignedIn(UserInfo),
    /// The last operation failed; show `code.i18n_key()`, not `detail`.
    Error(AuthError),
}

struct Session {
    access_token: String,
    expires_at: Instant,
    refresh_token: Option<String>,
    user: UserInfo,
}

struct SignIn {
    id: u64,
    cancel: Arc<AtomicBool>,
}

struct Shared {
    state: AuthState,
    version: u64,
    polled: u64,
    session: Option<Session>,
    signin: Option<SignIn>,
    next_id: u64,
    discovery: Option<(Instant, Discovery)>,
    jwks: Option<(Instant, Jwks)>,
    warning: Option<AuthErrorCode>,
}

struct Inner {
    issuer: String,
    client_id: String,
    http: Http,
    store: Arc<dyn TokenStore>,
    opener: Arc<dyn BrowserOpener>,
    shared: Mutex<Shared>,
    /// Single-flight guard: refresh tokens rotate, so exactly one refresh
    /// (or sign-out) may be in flight.
    refresh_lock: Mutex<()>,
    on_change: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

/// Desktop OIDC client. Cheap to clone; all clones share one state.
#[derive(Clone)]
pub struct AuthClient {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for AuthClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthClient")
            .field("issuer", &self.inner.issuer)
            .field("client_id", &self.inner.client_id)
            .finish_non_exhaustive()
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn status_error(reply: &Reply, what: &str) -> AuthError {
    let code = if reply.status >= 500 || reply.status == 429 {
        AuthErrorCode::Network
    } else {
        AuthErrorCode::ServerRejected
    };
    AuthError::new(code, format!("{what}: HTTP {}", reply.status))
}

impl AuthClient {
    /// A client for `issuer` using the OS keyring and the system browser.
    /// `issuer` must be `https`, or `http` on a loopback host (development).
    pub fn new(issuer: &str, client_id: &str) -> Result<Self> {
        Self::with_parts(
            issuer,
            client_id,
            Arc::new(KeyringTokenStore),
            Arc::new(SystemBrowser),
        )
    }

    /// Like [`AuthClient::new`] with the token store and the browser opener
    /// supplied (tests, or a file-based store on systems without a keyring).
    pub fn with_parts(
        issuer: &str,
        client_id: &str,
        store: Arc<dyn TokenStore>,
        opener: Arc<dyn BrowserOpener>,
    ) -> Result<Self> {
        let issuer = oidc::normalize_issuer(issuer)?;
        let shared = Shared {
            state: AuthState::SignedOut,
            version: 1,
            polled: 0,
            session: None,
            signin: None,
            next_id: 0,
            discovery: None,
            jwks: None,
            warning: None,
        };
        Ok(Self {
            inner: Arc::new(Inner {
                issuer,
                client_id: client_id.to_owned(),
                http: Http::new(),
                store,
                opener,
                shared: Mutex::new(shared),
                refresh_lock: Mutex::new(()),
                on_change: Mutex::new(None),
            }),
        })
    }

    /// The normalized issuer URL (also the keyring account).
    pub fn issuer(&self) -> &str {
        &self.inner.issuer
    }

    /// Registers a callback run on a worker thread after every state change,
    /// e.g. `move || ctx.request_repaint()`.
    pub fn on_change(&self, f: impl Fn() + Send + Sync + 'static) {
        *lock(&self.inner.on_change) = Some(Arc::new(f));
    }

    /// The current state.
    pub fn state(&self) -> AuthState {
        lock(&self.inner.shared).state.clone()
    }

    /// The new state if it changed since the last `poll` (the very first call
    /// returns the initial state).
    pub fn poll(&self) -> Option<AuthState> {
        let mut sh = lock(&self.inner.shared);
        if sh.version == sh.polled {
            return None;
        }
        sh.polled = sh.version;
        Some(sh.state.clone())
    }

    /// The signed-in user, if any.
    pub fn user(&self) -> Option<UserInfo> {
        lock(&self.inner.shared)
            .session
            .as_ref()
            .map(|s| s.user.clone())
    }

    /// The access token if it is still valid. Memory only; never persisted.
    pub fn access_token(&self) -> Option<String> {
        self.inner.fresh_access_token(Duration::ZERO)
    }

    /// A valid access token, refreshing it first if it expires within a minute.
    /// Blocking: call it from a worker thread. Concurrent callers share one refresh.
    pub fn ensure_access_token(&self) -> Result<String> {
        if let Some(t) = self.inner.fresh_access_token(EXPIRY_SKEW) {
            return Ok(t);
        }
        self.inner.refresh(false)?;
        self.inner
            .fresh_access_token(Duration::ZERO)
            .ok_or_else(|| AuthErrorCode::NotSignedIn.into())
    }

    /// Refreshes now, e.g. after the API answered 401. Blocking.
    /// `InvalidGrant` means the session is gone: the state becomes `SignedOut`.
    pub fn refresh_now(&self) -> Result<()> {
        self.inner.refresh(true)
    }

    /// A non-fatal problem of the last operation (the keyring could not store
    /// the refresh token: the session works but will not survive a restart).
    pub fn storage_warning(&self) -> Option<AuthErrorCode> {
        lock(&self.inner.shared).warning
    }

    /// Starts the browser sign-in and returns immediately. Watch [`AuthClient::state`]:
    /// `Working`, then `WaitingForBrowser { url }`, then `SignedIn` or `Error`.
    /// A sign-in already in progress is cancelled first.
    pub fn begin_sign_in(&self, lang: Lang) {
        let inner = self.inner.clone();
        let (id, cancel) = {
            let mut sh = lock(&inner.shared);
            if let Some(old) = sh.signin.take() {
                old.cancel.store(true, Ordering::SeqCst);
            }
            sh.next_id += 1;
            let cancel = Arc::new(AtomicBool::new(false));
            sh.signin = Some(SignIn {
                id: sh.next_id,
                cancel: cancel.clone(),
            });
            (sh.next_id, cancel)
        };
        inner.set_state(AuthState::Working);
        let worker = inner.clone();
        let spawned = std::thread::Builder::new()
            .name("dc-auth-signin".into())
            .spawn(move || worker.run_sign_in(id, &cancel, lang));
        if spawned.is_err() {
            inner.finish_sign_in(
                id,
                Err(AuthError::new(
                    AuthErrorCode::Internal,
                    "cannot start thread",
                )),
            );
        }
    }

    /// Cancels a sign-in in progress (closes the loopback listener). The state
    /// goes back to what it was before: `SignedIn` if a session exists, else `SignedOut`.
    pub fn cancel_sign_in(&self) {
        let cancelled = {
            let mut sh = lock(&self.inner.shared);
            match sh.signin.take() {
                Some(s) => {
                    s.cancel.store(true, Ordering::SeqCst);
                    true
                }
                None => false,
            }
        };
        if cancelled {
            self.inner.set_state_resting();
        }
    }

    /// At startup: silently signs in again with the refresh token in the
    /// keyring, on a background thread. No token: stays `SignedOut`.
    pub fn try_restore(&self) {
        let inner = self.inner.clone();
        inner.set_state(AuthState::Working);
        let worker = inner.clone();
        let spawned = std::thread::Builder::new()
            .name("dc-auth-restore".into())
            .spawn(move || {
                let _ = worker.restore();
            });
        if spawned.is_err() {
            inner.set_state(AuthState::Error(AuthErrorCode::Internal.into()));
        }
    }

    /// Blocking form of [`AuthClient::try_restore`]; returns the resulting state.
    pub fn restore_blocking(&self) -> AuthState {
        self.inner.set_state(AuthState::Working);
        let _ = self.inner.restore();
        self.state()
    }

    /// Signs out on a background thread: the state is `SignedOut` at once, then the
    /// refresh token is revoked at the server and removed from the keyring.
    pub fn sign_out(&self) {
        let inner = self.inner.clone();
        // Local state first, so the UI never shows a signed-in user while we talk to the network.
        let held = inner.take_session();
        let _ = std::thread::Builder::new()
            .name("dc-auth-signout".into())
            .spawn(move || {
                let _ = inner.revoke_and_forget(held);
            });
    }

    /// Blocking sign-out. Local state and the keyring entry are cleared even when
    /// the revocation fails (then the error is returned).
    pub fn sign_out_blocking(&self) -> Result<()> {
        let held = self.inner.take_session();
        self.inner.revoke_and_forget(held)
    }
}

impl Inner {
    // ---- state ---------------------------------------------------------

    fn set_state(&self, st: AuthState) {
        {
            let mut sh = lock(&self.shared);
            sh.state = st;
            sh.version += 1;
        }
        self.notify();
    }

    fn notify(&self) {
        let cb = lock(&self.on_change).clone();
        if let Some(cb) = cb {
            cb();
        }
    }

    fn resting_state(sh: &Shared) -> AuthState {
        match &sh.session {
            Some(s) => AuthState::SignedIn(s.user.clone()),
            None => AuthState::SignedOut,
        }
    }

    fn set_state_resting(&self) {
        {
            let mut sh = lock(&self.shared);
            sh.state = Self::resting_state(&sh);
            sh.version += 1;
        }
        self.notify();
    }

    fn set_state_if_current(&self, id: u64, st: AuthState) -> bool {
        {
            let mut sh = lock(&self.shared);
            if sh.signin.as_ref().map(|s| s.id) != Some(id) {
                return false;
            }
            sh.state = st;
            sh.version += 1;
        }
        self.notify();
        true
    }

    fn warn(&self, w: Option<AuthErrorCode>) {
        lock(&self.shared).warning = w;
    }

    /// Drops the session and any sign-in in progress (publishes nothing).
    fn clear_local(&self) {
        let mut sh = lock(&self.shared);
        sh.session = None;
        if let Some(s) = sh.signin.take() {
            s.cancel.store(true, Ordering::SeqCst);
        }
    }

    fn fresh_access_token(&self, skew: Duration) -> Option<String> {
        let sh = lock(&self.shared);
        let s = sh.session.as_ref()?;
        (Instant::now() + skew < s.expires_at).then(|| s.access_token.clone())
    }

    // ---- protocol ------------------------------------------------------

    fn discovery(&self) -> Result<Discovery> {
        if let Some((at, d)) = &lock(&self.shared).discovery
            && at.elapsed() < DISCOVERY_TTL
        {
            return Ok(d.clone());
        }
        let reply = self
            .http
            .get(
                &format!("{}/.well-known/openid-configuration", self.issuer),
                None,
            )
            .map_err(|e| match e.code {
                AuthErrorCode::Network | AuthErrorCode::Timeout => e,
                _ => AuthError::new(AuthErrorCode::DiscoveryFailed, e.detail),
            })?;
        if reply.status != 200 {
            return Err(AuthError::new(
                AuthErrorCode::DiscoveryFailed,
                format!("discovery: HTTP {}", reply.status),
            ));
        }
        let d = Discovery::parse(&reply.body, &self.issuer)?;
        lock(&self.shared).discovery = Some((Instant::now(), d.clone()));
        Ok(d)
    }

    fn jwks(&self, disc: &Discovery, kid: Option<&str>) -> Result<Jwks> {
        if let Some((at, k)) = &lock(&self.shared).jwks {
            let age = at.elapsed();
            let has = kid.is_none_or(|kid| k.contains(kid));
            if (age < JWKS_TTL && has) || age < JWKS_MIN_REFETCH {
                return Ok(k.clone());
            }
        }
        let reply = self.http.get(&disc.jwks_uri, None)?;
        if reply.status != 200 {
            return Err(status_error(&reply, "jwks"));
        }
        let k = Jwks::parse(&reply.body)
            .map_err(|e| AuthError::new(AuthErrorCode::DiscoveryFailed, format!("jwks: {e}")))?;
        lock(&self.shared).jwks = Some((Instant::now(), k.clone()));
        Ok(k)
    }

    fn verify_id_token(
        &self,
        disc: &Discovery,
        token: &str,
        nonce: Option<&str>,
    ) -> Result<jwt::IdClaims> {
        let invalid = |e: jwt::JwtError| AuthError::new(AuthErrorCode::TokenInvalid, e.to_string());
        let kid = jwt::peek_kid(token).map_err(invalid)?;
        let jwks = self.jwks(disc, kid.as_deref())?;
        let expect = Expect {
            issuer: &self.issuer,
            client_id: &self.client_id,
            nonce,
            now: unix_now(),
            leeway: LEEWAY_SECS,
        };
        jwt::verify_id_token(token, &jwks, &expect).map_err(invalid)
    }

    fn fetch_me(&self, access_token: &str) -> Result<UserInfo> {
        let reply = self
            .http
            .get(&format!("{}/api/v1/me", self.issuer), Some(access_token))?;
        if reply.status != 200 {
            return Err(status_error(&reply, "me"));
        }
        let user: UserInfo = reply.json()?;
        if user.sub.is_empty() {
            return Err(AuthError::new(AuthErrorCode::ServerRejected, "me: no sub"));
        }
        Ok(user)
    }

    fn token_request(&self, disc: &Discovery, form: &[(&str, &str)]) -> Result<TokenResponse> {
        let reply = self.http.post_form(&disc.token_endpoint, form)?;
        if reply.status == 200 {
            return reply.json();
        }
        if reply.oauth_error().as_deref() == Some("invalid_grant") {
            return Err(AuthError::new(AuthErrorCode::InvalidGrant, "invalid_grant"));
        }
        Err(status_error(&reply, "token"))
    }

    /// Verifies the tokens of a response and builds the session (`/me` included).
    fn establish(
        &self,
        disc: &Discovery,
        tok: TokenResponse,
        nonce: Option<&str>,
    ) -> Result<Session> {
        let id_token = tok
            .id_token
            .as_deref()
            .ok_or_else(|| AuthError::new(AuthErrorCode::TokenInvalid, "no id_token"))?;
        let claims = self.verify_id_token(disc, id_token, nonce)?;
        let user = self.fetch_me(&tok.access_token)?;
        if user.sub != claims.sub {
            return Err(AuthError::new(
                AuthErrorCode::TokenInvalid,
                "sub of id_token and /me differ",
            ));
        }
        Ok(Session {
            expires_at: Instant::now()
                + Duration::from_secs(tok.expires_in.unwrap_or(DEFAULT_TOKEN_TTL)),
            access_token: tok.access_token,
            refresh_token: tok.refresh_token,
            user,
        })
    }

    // ---- sign-in -------------------------------------------------------

    fn run_sign_in(&self, id: u64, cancel: &AtomicBool, lang: Lang) {
        let result = self.sign_in_flow(id, cancel, lang);
        self.finish_sign_in(id, result);
    }

    fn finish_sign_in(&self, id: u64, result: Result<Session>) {
        let mut persist = None;
        {
            let mut sh = lock(&self.shared);
            if sh.signin.as_ref().map(|s| s.id) != Some(id) {
                return; // superseded or cancelled: the newer owner of the state speaks
            }
            sh.signin = None;
            match result {
                Ok(session) => {
                    persist = session.refresh_token.clone();
                    sh.state = AuthState::SignedIn(session.user.clone());
                    sh.session = Some(session);
                }
                Err(e) if e.code == AuthErrorCode::Cancelled => sh.state = Self::resting_state(&sh),
                Err(e) => sh.state = AuthState::Error(e),
            }
            sh.version += 1;
        }
        if let Some(rt) = persist {
            self.persist_refresh_token(&rt);
        }
        self.notify();
    }

    fn persist_refresh_token(&self, token: &str) {
        match self.store.save(&self.issuer, token) {
            Ok(()) => self.warn(None),
            Err(_) => self.warn(Some(AuthErrorCode::StorageUnavailable)),
        }
    }

    fn sign_in_flow(&self, id: u64, cancel: &AtomicBool, lang: Lang) -> Result<Session> {
        let disc = self.discovery()?;
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .map_err(|e| AuthError::new(AuthErrorCode::ListenerFailed, e.to_string()))?;
        let port = listener
            .local_addr()
            .map_err(|e| AuthError::new(AuthErrorCode::ListenerFailed, e.to_string()))?
            .port();
        let redirect = oidc::redirect_uri(port);
        let pkce = Pkce::generate()?;
        let state = random_token(32)?;
        let nonce = random_token(32)?;
        let url = oidc::authorize_url(&AuthorizeParams {
            endpoint: &disc.authorization_endpoint,
            client_id: &self.client_id,
            redirect_uri: &redirect,
            scope: SCOPES,
            state: &state,
            nonce: &nonce,
            code_challenge: &pkce.challenge,
            lang,
        })?;
        if !self.set_state_if_current(id, AuthState::WaitingForBrowser { url: url.clone() }) {
            return Err(AuthErrorCode::Cancelled.into());
        }
        // A failure to launch is not fatal: the UI shows the URL.
        let _opened = self.opener.open(&url);
        let cb = wait_for_callback(
            &listener,
            lang,
            &state,
            &self.issuer,
            cancel,
            Instant::now() + SIGN_IN_TIMEOUT,
        )?;
        drop(listener);
        if !self.set_state_if_current(id, AuthState::Working) {
            return Err(AuthErrorCode::Cancelled.into());
        }
        // The code lives 60 s: exchange it at once.
        let tok = self.token_request(
            &disc,
            &[
                ("grant_type", "authorization_code"),
                ("client_id", &self.client_id),
                ("code", &cb.code),
                ("code_verifier", &pkce.verifier),
                ("redirect_uri", &redirect),
            ],
        )?;
        if cancel.load(Ordering::SeqCst) {
            return Err(AuthErrorCode::Cancelled.into());
        }
        self.establish(&disc, tok, Some(&nonce))
    }

    // ---- refresh / restore / sign-out -----------------------------------

    /// Exchanges the refresh token for new tokens and persists the rotated one
    /// at once (the old one is spent). The caller holds `refresh_lock`.
    fn refresh_locked(&self, disc: &Discovery) -> Result<TokenResponse> {
        let in_memory = lock(&self.shared)
            .session
            .as_ref()
            .and_then(|s| s.refresh_token.clone());
        let rt = match in_memory {
            Some(t) => t,
            None => self
                .store
                .load(&self.issuer)?
                .ok_or(AuthErrorCode::NotSignedIn)?,
        };
        let tok = match self.token_request(
            disc,
            &[
                ("grant_type", "refresh_token"),
                ("client_id", &self.client_id),
                ("refresh_token", &rt),
            ],
        ) {
            Ok(t) => t,
            Err(e) if e.code == AuthErrorCode::InvalidGrant => {
                // Spent, revoked or expired (or reuse detected): this sign-in is over.
                self.clear_local();
                let _ = self.store.delete(&self.issuer);
                self.set_state(AuthState::SignedOut);
                return Err(e);
            }
            Err(e) => return Err(e),
        };
        let new_rt = tok.refresh_token.clone().unwrap_or(rt);
        if let Some(s) = lock(&self.shared).session.as_mut() {
            s.refresh_token = Some(new_rt.clone());
        }
        self.persist_refresh_token(&new_rt);
        Ok(tok)
    }

    /// Forgets everything after a response that cannot be trusted.
    fn wipe(&self, err: AuthError) -> AuthError {
        self.clear_local();
        let _ = self.store.delete(&self.issuer);
        self.set_state(AuthState::Error(err.clone()));
        err
    }

    fn refresh(&self, force: bool) -> Result<()> {
        let _guard = lock(&self.refresh_lock);
        if !force && self.fresh_access_token(EXPIRY_SKEW).is_some() {
            return Ok(()); // another caller refreshed while we waited
        }
        let (known_sub, has_session) = {
            let sh = lock(&self.shared);
            (
                sh.session.as_ref().map(|s| s.user.sub.clone()),
                sh.session.is_some(),
            )
        };
        if !has_session {
            return Err(AuthErrorCode::NotSignedIn.into());
        }
        let disc = self.discovery()?;
        let tok = self.refresh_locked(&disc)?;
        if let Some(id_token) = tok.id_token.as_deref() {
            match self.verify_id_token(&disc, id_token, None) {
                Ok(c) if Some(&c.sub) == known_sub.as_ref() => {}
                Ok(_) => {
                    return Err(self.wipe(AuthError::new(
                        AuthErrorCode::TokenInvalid,
                        "sub changed on refresh",
                    )));
                }
                Err(e) if e.code == AuthErrorCode::TokenInvalid => return Err(self.wipe(e)),
                Err(e) => return Err(e),
            }
        }
        // Entitlements may have changed; keep the old profile if /me is unreachable.
        let fresh_user = self
            .fetch_me(&tok.access_token)
            .ok()
            .filter(|u| Some(&u.sub) == known_sub.as_ref());
        let changed = {
            let mut sh = lock(&self.shared);
            let Some(s) = sh.session.as_mut() else {
                return Err(AuthErrorCode::NotSignedIn.into());
            };
            s.access_token = tok.access_token;
            s.expires_at =
                Instant::now() + Duration::from_secs(tok.expires_in.unwrap_or(DEFAULT_TOKEN_TTL));
            match fresh_user {
                Some(u) if u != s.user => {
                    s.user = u;
                    true
                }
                _ => false,
            }
        };
        if changed {
            self.set_state_resting();
        }
        Ok(())
    }

    fn restore(&self) -> Result<()> {
        let _guard = lock(&self.refresh_lock);
        if lock(&self.shared).session.is_some() {
            self.set_state_resting();
            return Ok(());
        }
        let outcome = (|| {
            if self.store.load(&self.issuer)?.is_none() {
                return Ok(None);
            }
            let disc = self.discovery()?;
            let tok = self.refresh_locked(&disc)?;
            self.establish(&disc, tok, None).map(Some)
        })();
        match outcome {
            Ok(None) => {
                self.set_state(AuthState::SignedOut);
                Ok(())
            }
            Ok(Some(mut session)) => {
                // refresh_locked stored the rotated token; carry it into the session
                if session.refresh_token.is_none() {
                    session.refresh_token = self.store.load(&self.issuer).ok().flatten();
                }
                let user = session.user.clone();
                lock(&self.shared).session = Some(session);
                self.set_state(AuthState::SignedIn(user));
                Ok(())
            }
            // Spent token: refresh_locked already published SignedOut.
            Err(e) if e.code == AuthErrorCode::InvalidGrant => Err(e),
            // No usable keyring at startup is not an error screen.
            Err(e) if e.code == AuthErrorCode::StorageUnavailable => {
                self.warn(Some(AuthErrorCode::StorageUnavailable));
                self.set_state(AuthState::SignedOut);
                Err(e)
            }
            Err(e) if e.code == AuthErrorCode::TokenInvalid => Err(self.wipe(e)),
            Err(e) => {
                self.set_state(AuthState::Error(e.clone()));
                Err(e)
            }
        }
    }

    /// Drops the session and any sign-in in progress, publishes `SignedOut`
    /// and returns the refresh token that was held in memory.
    fn take_session(&self) -> Option<String> {
        let rt = {
            let mut sh = lock(&self.shared);
            let rt = sh.session.take().and_then(|s| s.refresh_token);
            if let Some(s) = sh.signin.take() {
                s.cancel.store(true, Ordering::SeqCst);
            }
            rt
        };
        self.set_state(AuthState::SignedOut);
        rt
    }

    /// Revokes the refresh token(s) at the server and removes the stored one.
    /// The keyring entry is deleted even when the revocation fails.
    fn revoke_and_forget(&self, held: Option<String>) -> Result<()> {
        let _guard = lock(&self.refresh_lock);
        // A refresh that was in flight may have rotated the token since `held`.
        let mut tokens: Vec<String> = Vec::new();
        for t in [self.store.load(&self.issuer).ok().flatten(), held]
            .into_iter()
            .flatten()
        {
            if !tokens.contains(&t) {
                tokens.push(t);
            }
        }
        let revoked = if tokens.is_empty() {
            Ok(())
        } else {
            self.discovery().and_then(|d| {
                let mut first_err = None;
                for t in &tokens {
                    let r = self
                        .http
                        .post_form(
                            &d.revocation_endpoint,
                            &[("client_id", &self.client_id), ("token", t)],
                        )
                        .and_then(|reply| {
                            if reply.status == 200 {
                                Ok(())
                            } else {
                                Err(status_error(&reply, "revoke"))
                            }
                        });
                    if let Err(e) = r {
                        first_err.get_or_insert(e);
                    }
                }
                first_err.map_or(Ok(()), Err)
            })
        };
        let deleted = self.store.delete(&self.issuer);
        revoked.and(deleted)
    }
}
