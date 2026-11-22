//! The whole client against an in-process mock of dentalcraft-server: real
//! sockets, a real Ed25519-signed id_token, a simulated browser. No keyring,
//! no browser, no external process.

use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use dc_auth_client::pkce::challenge_s256;
use dc_auth_client::{
    AuthClient, AuthErrorCode, AuthState, FallbackTokenStore, FileTokenStore, Lang,
    MemoryTokenStore, TokenStore,
};
use ed25519_dalek::{Signer, SigningKey};
use serde_json::{Value, json};

const CLIENT: &str = "dentalcraft-desktop";

struct Pending {
    nonce: String,
    challenge: String,
    redirect: String,
}

struct Ms {
    key: SigningKey,
    issuer: String,
    pending: HashMap<String, Pending>,
    valid_rt: HashSet<String>,
    spent_rt: HashSet<String>,
    access: HashSet<String>,
    counter: u64,
    refresh_calls: u32,
    revoked: Vec<String>,
    refresh_delay: Duration,
    signin_expires_in: u64,
    wrong_nonce: bool,
    /// Seconds the server clock runs ahead of this computer (negative: behind).
    clock_ahead: i64,
}

struct Mock {
    issuer: String,
    st: Arc<Mutex<Ms>>,
    stop: Arc<AtomicBool>,
}

impl Drop for Mock {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn form(body: &str) -> HashMap<String, String> {
    url::form_urlencoded::parse(body.as_bytes())
        .into_owned()
        .collect()
}

impl Ms {
    fn id_token(&self, nonce: Option<&str>) -> String {
        let header = json!({"alg": "EdDSA", "typ": "JWT", "kid": "k1"});
        let t = now().saturating_add_signed(self.clock_ahead);
        let mut claims = json!({"iss": self.issuer, "sub": "usr_1", "aud": CLIENT, "azp": CLIENT, "iat": t, "exp": t + 3600});
        if let Some(n) = nonce {
            claims["nonce"] = json!(n);
        }
        let input = format!(
            "{}.{}",
            B64.encode(header.to_string()),
            B64.encode(claims.to_string())
        );
        format!(
            "{input}.{}",
            B64.encode(self.key.sign(input.as_bytes()).to_bytes())
        )
    }

    fn issue(&mut self, nonce: Option<&str>, expires_in: u64) -> Value {
        self.counter += 1;
        let (at, rt) = (
            format!("dcat_{}", self.counter),
            format!("dcrt_{}", self.counter),
        );
        self.access.insert(at.clone());
        self.valid_rt.insert(rt.clone());
        let nonce = if self.wrong_nonce && nonce.is_some() {
            Some("not-the-nonce")
        } else {
            nonce
        };
        json!({"access_token": at, "token_type": "Bearer", "expires_in": expires_in, "refresh_token": rt, "id_token": self.id_token(nonce)})
    }
}

fn handle(st: &Mutex<Ms>, stream: &mut TcpStream) {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut buf = Vec::new();
    let mut chunk = [0u8; 2048];
    let head_end = loop {
        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break p + 4;
        }
        let n = stream.read(&mut chunk).unwrap_or(0);
        if n == 0 {
            return;
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let len: usize = head
        .lines()
        .find_map(|l| {
            l.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .map(|v| v.trim().parse().unwrap())
        })
        .unwrap_or(0);
    while buf.len() < head_end + len {
        let n = stream.read(&mut chunk).unwrap_or(0);
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let body = String::from_utf8_lossy(&buf[head_end..]).into_owned();
    let mut first = head.lines().next().unwrap().split(' ');
    let (method, path) = (first.next().unwrap(), first.next().unwrap());
    let bearer = head
        .lines()
        .find(|l| l.to_ascii_lowercase().starts_with("authorization:"))
        .and_then(|l| l.split_once(' ').map(|x| x.1))
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::to_owned);
    assert!(
        !head.to_ascii_lowercase().contains("authorization: basic"),
        "public client must not authenticate"
    );

    let (status, json): (u16, Value) = match (method, path) {
        ("GET", "/.well-known/openid-configuration") => {
            let iss = st.lock().unwrap().issuer.clone();
            (
                200,
                json!({"issuer": iss, "authorization_endpoint": format!("{iss}/oauth/authorize"), "token_endpoint": format!("{iss}/oauth/token"),
                       "revocation_endpoint": format!("{iss}/oauth/revoke"), "jwks_uri": format!("{iss}/oauth/jwks.json")}),
            )
        }
        ("GET", "/oauth/jwks.json") => {
            let s = st.lock().unwrap();
            (
                200,
                json!({"keys": [{"kty": "OKP", "crv": "Ed25519", "kid": "k1", "use": "sig", "alg": "EdDSA", "x": B64.encode(s.key.verifying_key().as_bytes())}]}),
            )
        }
        ("POST", "/oauth/token") => {
            let f = form(&body);
            let delay = st.lock().unwrap().refresh_delay;
            match f.get("grant_type").map(String::as_str) {
                Some("authorization_code") => {
                    let mut s = st.lock().unwrap();
                    let p = f.get("code").and_then(|c| s.pending.remove(c));
                    match p {
                        Some(p)
                            if f.get("redirect_uri") == Some(&p.redirect)
                                && f.get("client_id").map(String::as_str) == Some(CLIENT)
                                && f.get("code_verifier")
                                    .is_some_and(|v| challenge_s256(v) == p.challenge) =>
                        {
                            let exp = s.signin_expires_in;
                            (200, s.issue(Some(&p.nonce), exp))
                        }
                        _ => (400, json!({"error": "invalid_grant"})),
                    }
                }
                Some("refresh_token") => {
                    std::thread::sleep(delay);
                    let mut s = st.lock().unwrap();
                    s.refresh_calls += 1;
                    let rt = f.get("refresh_token").cloned().unwrap_or_default();
                    if s.valid_rt.remove(&rt) {
                        s.spent_rt.insert(rt);
                        (200, s.issue(None, 3600))
                    } else {
                        if s.spent_rt.contains(&rt) {
                            s.valid_rt.clear(); // reuse: the family is revoked
                        }
                        (400, json!({"error": "invalid_grant"}))
                    }
                }
                _ => (400, json!({"error": "unsupported_grant_type"})),
            }
        }
        ("POST", "/oauth/revoke") => {
            let f = form(&body);
            let mut s = st.lock().unwrap();
            let t = f.get("token").cloned().unwrap_or_default();
            s.valid_rt.remove(&t);
            s.revoked.push(t);
            (200, json!({}))
        }
        ("GET", "/api/v1/me") => {
            let s = st.lock().unwrap();
            if bearer.as_ref().is_some_and(|b| s.access.contains(b)) {
                (
                    200,
                    json!({"sub": "usr_1", "email": "ann@example.com", "name": "Ann Example", "entitlements": {"modules": ["aligners"]}}),
                )
            } else {
                (401, json!({"error": {"code": "unauthorized"}}))
            }
        }
        _ => (404, json!({})),
    };
    let body = json.to_string();
    let reason = if status == 200 { "OK" } else { "Error" };
    let _ = write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

impl Mock {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let issuer = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let st = Arc::new(Mutex::new(Ms {
            key: SigningKey::from_bytes(&[42; 32]),
            issuer: issuer.clone(),
            pending: HashMap::new(),
            valid_rt: HashSet::new(),
            spent_rt: HashSet::new(),
            access: HashSet::new(),
            counter: 0,
            refresh_calls: 0,
            revoked: Vec::new(),
            refresh_delay: Duration::ZERO,
            signin_expires_in: 3600,
            wrong_nonce: false,
            clock_ahead: 0,
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let (st2, stop2) = (st.clone(), stop.clone());
        std::thread::spawn(move || {
            while !stop2.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut s, _)) => {
                        s.set_nonblocking(false).unwrap();
                        let st3 = st2.clone();
                        std::thread::spawn(move || handle(&st3, &mut s));
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(5)),
                }
            }
        });
        Mock { issuer, st, stop }
    }

    fn with<R>(&self, f: impl FnOnce(&mut Ms) -> R) -> R {
        f(&mut self.st.lock().unwrap())
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Browser {
    Good,
    BadState,
    BadIss,
    Denied,
    Idle,
}

/// A browser that follows the authorize URL the way the real server would:
/// a code is minted for the request and the loopback callback is hit.
fn browser(mock: &Mock, mode: Browser) -> Arc<dyn dc_auth_client::BrowserOpener> {
    let st = mock.st.clone();
    let issuer = mock.issuer.clone();
    Arc::new(move |auth_url: &str| {
        if mode == Browser::Idle {
            return true;
        }
        let u = url::Url::parse(auth_url).unwrap();
        let q: HashMap<String, String> = u.query_pairs().into_owned().collect();
        assert_eq!(q["code_challenge_method"], "S256");
        assert_eq!(q["client_id"], CLIENT);
        assert!(
            q["redirect_uri"].starts_with("http://127.0.0.1:")
                && q["redirect_uri"].ends_with("/callback")
        );
        let code = {
            let mut s = st.lock().unwrap();
            s.counter += 1;
            let code = format!("dcac_{}", s.counter);
            s.pending.insert(
                code.clone(),
                Pending {
                    nonce: q["nonce"].clone(),
                    challenge: q["code_challenge"].clone(),
                    redirect: q["redirect_uri"].clone(),
                },
            );
            code
        };
        let (state, iss) = match mode {
            Browser::BadIss => (q["state"].clone(), "https://evil.example".to_owned()),
            _ => (q["state"].clone(), issuer.clone()),
        };
        let callback = |state: &str| {
            let mut cb = url::Url::parse(&q["redirect_uri"]).unwrap();
            {
                let mut p = cb.query_pairs_mut();
                if mode == Browser::Denied {
                    p.append_pair("error", "access_denied");
                } else {
                    p.append_pair("code", &code);
                }
                p.append_pair("state", state).append_pair("iss", &iss);
            }
            cb
        };
        // BadState: a forged redirect first (refused, the wait goes on), then
        // the real one
        let mut hits = Vec::new();
        if mode == Browser::BadState {
            hits.push(callback("forged"));
        }
        hits.push(callback(&state));
        std::thread::spawn(move || {
            for cb in hits {
                let port = cb.port().unwrap();
                let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
                let target = format!("{}?{}", cb.path(), cb.query().unwrap());
                write!(
                    s,
                    "GET {target} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
                )
                .unwrap();
                let mut out = String::new();
                let _ = s.read_to_string(&mut out);
            }
        });
        true
    })
}

fn client(mock: &Mock, store: Arc<dyn TokenStore>, mode: Browser) -> AuthClient {
    AuthClient::with_parts(&mock.issuer, CLIENT, store, browser(mock, mode)).unwrap()
}

fn wait_for(c: &AuthClient, what: &str, pred: impl Fn(&AuthState) -> bool) -> AuthState {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let s = c.state();
        if pred(&s) {
            return s;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}; state: {s:?}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn sign_in(c: &AuthClient) -> AuthState {
    c.begin_sign_in(Lang::En);
    wait_for(c, "sign-in to finish", |s| {
        matches!(s, AuthState::SignedIn(_) | AuthState::Error(_))
    })
}

fn expect_error(c: &AuthClient, code: AuthErrorCode) {
    match sign_in(c) {
        AuthState::Error(e) => assert_eq!(e.code, code, "{e}"),
        other => panic!("expected {code:?}, got {other:?}"),
    }
}

fn mem() -> Arc<MemoryTokenStore> {
    Arc::new(MemoryTokenStore::new())
}

#[test]
fn sign_in_end_to_end() {
    let mock = Mock::start();
    let store = mem();
    let c = client(&mock, store.clone(), Browser::Good);
    assert_eq!(c.poll(), Some(AuthState::SignedOut));
    assert_eq!(c.poll(), None);

    let seen = Arc::new(Mutex::new(Vec::<AuthState>::new()));
    let (c2, seen2) = (c.clone(), seen.clone());
    c.on_change(move || seen2.lock().unwrap().push(c2.state()));

    let AuthState::SignedIn(user) = sign_in(&c) else {
        panic!("not signed in: {:?}", c.state())
    };
    assert_eq!(user.sub, "usr_1");
    assert_eq!(user.email, "ann@example.com");
    assert_eq!(user.name, "Ann Example");
    assert!(user.entitlements.has_module("aligners"));
    assert!(!user.entitlements.has_module("implants"));
    assert!(c.access_token().is_some());
    assert_eq!(c.user().unwrap(), user);
    assert_eq!(store.load(c.issuer()).unwrap().as_deref(), Some("dcrt_2"));
    assert_eq!(c.storage_warning(), None);
    assert!(matches!(c.poll(), Some(AuthState::SignedIn(_))));

    let seen = seen.lock().unwrap();
    assert!(
        seen.iter().any(
            |s| matches!(s, AuthState::WaitingForBrowser { url } if url.contains("code_challenge="))
        ),
        "{seen:?}"
    );
}

#[test]
fn restore_rotates_the_stored_token() {
    let mock = Mock::start();
    let store = mem();
    sign_in(&client(&mock, store.clone(), Browser::Good));
    let before = store.load(&mock.issuer).unwrap().unwrap();

    let c = client(&mock, store.clone(), Browser::Idle);
    assert!(matches!(c.restore_blocking(), AuthState::SignedIn(u) if u.sub == "usr_1"));
    let after = store.load(&mock.issuer).unwrap().unwrap();
    assert_ne!(before, after);
    assert!(c.access_token().is_some());
    // and the rotated token restores again
    let c = client(&mock, store.clone(), Browser::Idle);
    assert!(matches!(c.restore_blocking(), AuthState::SignedIn(_)));
}

/// An OS keyring that cannot be reached, as in a Flatpak sandbox without
/// access to the Secret Service.
struct NoKeyring;

impl TokenStore for NoKeyring {
    fn load(&self, _: &str) -> dc_auth_client::Result<Option<String>> {
        Err(dc_auth_client::AuthError::new(
            AuthErrorCode::StorageUnavailable,
            "keyring open: no secret service",
        ))
    }
    fn save(&self, account: &str, _: &str) -> dc_auth_client::Result<()> {
        self.load(account).map(|_| ())
    }
    fn delete(&self, account: &str) -> dc_auth_client::Result<()> {
        self.load(account).map(|_| ())
    }
}

/// A unique scratch folder under the system temp dir, removed on drop.
struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        Self(std::env::temp_dir().join(format!("dc-auth-it-{name}-{}-{nanos}", std::process::id())))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The app's store: the keyring, then a private file in the config folder.
fn keyring_or_file(path: &std::path::Path) -> Arc<dyn TokenStore> {
    Arc::new(FallbackTokenStore::new(
        Arc::new(NoKeyring),
        Arc::new(FileTokenStore::new(path)),
    ))
}

#[test]
fn session_survives_a_restart_without_a_keyring() {
    let mock = Mock::start();
    mock.with(|m| m.signin_expires_in = 10); // the access token is stale before any restart

    // Keyring alone (the bug): the token is lost and the restart is signed out.
    let c = client(&mock, Arc::new(NoKeyring), Browser::Good);
    assert!(matches!(sign_in(&c), AuthState::SignedIn(_)));
    assert_eq!(c.storage_warning(), Some(AuthErrorCode::StorageUnavailable));
    drop(c);
    let c = client(&mock, Arc::new(NoKeyring), Browser::Idle);
    assert_eq!(c.restore_blocking(), AuthState::SignedOut);

    // Keyring then file: every "process" builds its own store over the same file.
    let tmp = Scratch::new("restart");
    let path = tmp.0.join("config").join("auth-tokens.json");
    let c = client(&mock, keyring_or_file(&path), Browser::Good);
    let AuthState::SignedIn(user) = sign_in(&c) else {
        panic!("not signed in: {:?}", c.state())
    };
    assert_eq!(c.storage_warning(), None);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "token file must be private, got {mode:o}");
    }
    drop(c);

    for _ in 0..2 {
        let refreshes = mock.with(|m| m.refresh_calls);
        let c = client(&mock, keyring_or_file(&path), Browser::Idle);
        assert_eq!(c.restore_blocking(), AuthState::SignedIn(user.clone()));
        assert!(c.access_token().is_some());
        assert_eq!(
            mock.with(|m| m.refresh_calls),
            refreshes + 1,
            "silent refresh"
        );
        // a later refresh in the session rotates again, and the file follows
        c.refresh_now().unwrap();
        assert_eq!(mock.with(|m| m.refresh_calls), refreshes + 2);
    }

    // Sign-out removes the file; the next start is signed out.
    let c = client(&mock, keyring_or_file(&path), Browser::Idle);
    assert!(matches!(c.restore_blocking(), AuthState::SignedIn(_)));
    let _ = c.sign_out_blocking(); // the keyring part reports StorageUnavailable
    assert!(!path.exists());
    let c = client(&mock, keyring_or_file(&path), Browser::Idle);
    assert_eq!(c.restore_blocking(), AuthState::SignedOut);
}

#[test]
fn restore_without_a_token_stays_signed_out() {
    let mock = Mock::start();
    let c = client(&mock, mem(), Browser::Idle);
    assert_eq!(c.restore_blocking(), AuthState::SignedOut);
    assert_eq!(mock.with(|m| m.refresh_calls), 0);
}

#[test]
fn restore_with_a_spent_token_is_signed_out_and_forgets_it() {
    let mock = Mock::start();
    let store = mem();
    sign_in(&client(&mock, store.clone(), Browser::Good));
    let c1 = client(&mock, store.clone(), Browser::Idle);
    let stale = store.load(&mock.issuer).unwrap().unwrap();
    assert!(matches!(c1.restore_blocking(), AuthState::SignedIn(_))); // rotates, `stale` is spent
    store.save(&mock.issuer, &stale).unwrap();
    let c2 = client(&mock, store.clone(), Browser::Idle);
    assert_eq!(c2.restore_blocking(), AuthState::SignedOut);
    assert_eq!(store.load(&mock.issuer).unwrap(), None);
}

#[test]
fn concurrent_ensure_access_token_refreshes_once() {
    let mock = Mock::start();
    mock.with(|m| {
        m.signin_expires_in = 10; // inside the 60 s refresh skew: stale at once
        m.refresh_delay = Duration::from_millis(150);
    });
    let c = client(&mock, mem(), Browser::Good);
    sign_in(&c);
    let tokens: Vec<_> = (0..8)
        .map(|_| {
            let c = c.clone();
            std::thread::spawn(move || c.ensure_access_token().unwrap())
        })
        .collect::<Vec<_>>()
        .into_iter()
        .map(|h| h.join().unwrap())
        .collect();
    assert_eq!(mock.with(|m| m.refresh_calls), 1, "single flight");
    assert!(tokens.iter().all(|t| t == &tokens[0]));
    assert!(matches!(c.state(), AuthState::SignedIn(_)));
}

#[test]
fn forced_refreshes_never_reuse_a_spent_token() {
    let mock = Mock::start();
    mock.with(|m| m.refresh_delay = Duration::from_millis(30));
    let store = mem();
    let c = client(&mock, store.clone(), Browser::Good);
    sign_in(&c);
    let hs: Vec<_> = (0..5)
        .map(|_| {
            let c = c.clone();
            std::thread::spawn(move || c.refresh_now())
        })
        .collect();
    for h in hs {
        h.join().unwrap().unwrap();
    }
    assert_eq!(mock.with(|m| m.refresh_calls), 5);
    assert!(matches!(c.state(), AuthState::SignedIn(_)));
    assert_eq!(
        store.load(&mock.issuer).unwrap().as_deref(),
        Some(format!("dcrt_{}", 2 + 5).as_str())
    );
}

#[test]
fn invalid_grant_on_refresh_means_signed_out() {
    let mock = Mock::start();
    let store = mem();
    let c = client(&mock, store.clone(), Browser::Good);
    sign_in(&c);
    mock.with(|m| m.valid_rt.clear()); // server-side revocation
    assert_eq!(
        c.refresh_now().unwrap_err().code,
        AuthErrorCode::InvalidGrant
    );
    assert_eq!(c.state(), AuthState::SignedOut);
    assert_eq!(c.access_token(), None);
    assert_eq!(store.load(&mock.issuer).unwrap(), None);
    assert_eq!(
        c.ensure_access_token().unwrap_err().code,
        AuthErrorCode::NotSignedIn
    );
}

#[test]
fn sign_out_revokes_and_forgets() {
    let mock = Mock::start();
    let store = mem();
    let c = client(&mock, store.clone(), Browser::Good);
    sign_in(&c);
    let rt = store.load(&mock.issuer).unwrap().unwrap();
    c.sign_out_blocking().unwrap();
    assert_eq!(c.state(), AuthState::SignedOut);
    assert_eq!(c.access_token(), None);
    assert_eq!(store.load(&mock.issuer).unwrap(), None);
    assert_eq!(mock.with(|m| m.revoked.clone()), vec![rt]);
    // idempotent, and nothing more to revoke
    c.sign_out_blocking().unwrap();
    assert_eq!(mock.with(|m| m.revoked.len()), 1);
}

#[test]
fn state_and_iss_mismatches_are_rejected() {
    let mock = Mock::start();
    let store = mem();
    expect_error(
        &client(&mock, store.clone(), Browser::BadIss),
        AuthErrorCode::CallbackInvalid,
    );
    expect_error(
        &client(&mock, store.clone(), Browser::Denied),
        AuthErrorCode::AccessDenied,
    );
    assert_eq!(store.load(&mock.issuer).unwrap(), None);
    assert_eq!(mock.with(|m| m.counter), 2, "no token was ever issued");
}

#[test]
fn a_forged_redirect_does_not_abort_the_sign_in() {
    let mock = Mock::start();
    let store = mem();
    // a redirect with another `state` is refused and the real one still lands
    let c = client(&mock, store.clone(), Browser::BadState);
    assert!(
        matches!(sign_in(&c), AuthState::SignedIn(_)),
        "{:?}",
        c.state()
    );
}

#[test]
fn a_token_rotated_by_another_instance_is_used_on_refresh() {
    let mock = Mock::start();
    let store = mem();
    let first = client(&mock, store.clone(), Browser::Good);
    sign_in(&first);
    // a second instance of the app starts and rotates the stored token
    let second = client(&mock, store.clone(), Browser::Idle);
    assert!(matches!(second.restore_blocking(), AuthState::SignedIn(_)));
    // the first one refreshes with the stored token, not its spent one
    first.refresh_now().unwrap();
    assert!(matches!(first.state(), AuthState::SignedIn(_)));
    second.refresh_now().unwrap();
    assert!(store.load(&mock.issuer).unwrap().is_some());
}

#[test]
fn id_token_with_a_wrong_nonce_is_rejected() {
    let mock = Mock::start();
    mock.with(|m| m.wrong_nonce = true);
    let store = mem();
    let c = client(&mock, store.clone(), Browser::Good);
    expect_error(&c, AuthErrorCode::TokenInvalid);
    assert_eq!(c.user(), None);
    assert_eq!(c.access_token(), None);
}

#[test]
fn a_clock_two_minutes_behind_still_signs_in() {
    let mock = Mock::start();
    mock.with(|m| m.clock_ahead = 120);
    let store = mem();
    let c = client(&mock, store.clone(), Browser::Good);
    assert!(
        matches!(sign_in(&c), AuthState::SignedIn(_)),
        "{:?}",
        c.state()
    );
    // and the session restores
    let c = client(&mock, store.clone(), Browser::Idle);
    assert!(matches!(c.restore_blocking(), AuthState::SignedIn(_)));
}

#[test]
fn a_clock_ten_minutes_behind_is_a_clock_error_that_keeps_the_session() {
    let mock = Mock::start();
    let store = mem();
    sign_in(&client(&mock, store.clone(), Browser::Good));
    mock.with(|m| m.clock_ahead = 600);

    let c = client(&mock, store.clone(), Browser::Idle);
    assert!(
        matches!(c.restore_blocking(), AuthState::Error(e) if e.code == AuthErrorCode::Clock),
        "{:?}",
        c.state()
    );
    let kept = store.load(&mock.issuer).unwrap();
    assert!(kept.is_some(), "the stored refresh token was wiped");
    // a fresh sign-in reports the clock too
    expect_error(&client(&mock, mem(), Browser::Good), AuthErrorCode::Clock);

    // once the clock is right again, the kept session restores
    mock.with(|m| m.clock_ahead = 0);
    let c = client(&mock, store.clone(), Browser::Idle);
    assert!(matches!(c.restore_blocking(), AuthState::SignedIn(_)));
}

#[test]
fn a_huge_expires_in_does_not_overflow() {
    let mock = Mock::start();
    mock.with(|m| m.signin_expires_in = u64::MAX);
    let c = client(&mock, mem(), Browser::Good);
    assert!(
        matches!(sign_in(&c), AuthState::SignedIn(_)),
        "{:?}",
        c.state()
    );
    assert!(c.access_token().is_some());
}

#[test]
fn cancel_closes_the_listener() {
    let mock = Mock::start();
    let c = client(&mock, mem(), Browser::Idle);
    c.begin_sign_in(Lang::It);
    let AuthState::WaitingForBrowser { url } = wait_for(&c, "browser step", |s| {
        matches!(s, AuthState::WaitingForBrowser { .. })
    }) else {
        unreachable!()
    };
    assert!(url.contains("ui_locales=it"));
    let redirect = url::Url::parse(&url)
        .unwrap()
        .query_pairs()
        .find(|(k, _)| k == "redirect_uri")
        .unwrap()
        .1
        .into_owned();
    let port = url::Url::parse(&redirect).unwrap().port().unwrap();
    c.cancel_sign_in();
    assert_eq!(c.state(), AuthState::SignedOut);
    let deadline = Instant::now() + Duration::from_secs(3);
    while TcpStream::connect(("127.0.0.1", port)).is_ok() {
        assert!(
            Instant::now() < deadline,
            "listener still open after cancel"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(c.state(), AuthState::SignedOut);
}

#[test]
fn unreachable_server_is_a_network_error() {
    let dead = TcpListener::bind("127.0.0.1:0").unwrap();
    let issuer = format!("http://127.0.0.1:{}", dead.local_addr().unwrap().port());
    drop(dead);
    let c = AuthClient::with_parts(&issuer, CLIENT, mem(), Arc::new(|_: &str| true)).unwrap();
    expect_error(&c, AuthErrorCode::Network);
    // offline restore keeps the stored token for later
    let store = mem();
    store.save(&issuer, "dcrt_x").unwrap();
    let c =
        AuthClient::with_parts(&issuer, CLIENT, store.clone(), Arc::new(|_: &str| true)).unwrap();
    assert!(
        matches!(c.restore_blocking(), AuthState::Error(e) if e.code == AuthErrorCode::Network)
    );
    assert_eq!(store.load(&issuer).unwrap().as_deref(), Some("dcrt_x"));
}

#[test]
fn plain_http_issuers_are_refused_off_loopback() {
    let e = AuthClient::new("http://auth.example.com", CLIENT).unwrap_err();
    assert_eq!(e.code, AuthErrorCode::InvalidIssuer);
}

#[test]
fn a_second_sign_in_supersedes_the_first() {
    let mock = Mock::start();
    let c = client(&mock, mem(), Browser::Idle);
    c.begin_sign_in(Lang::En);
    wait_for(&c, "first browser step", |s| {
        matches!(s, AuthState::WaitingForBrowser { .. })
    });
    let first = c.state();
    c.begin_sign_in(Lang::En);
    let second = wait_for(&c, "second browser step", |s| {
        matches!(s, AuthState::WaitingForBrowser { .. }) && *s != first
    });
    assert_ne!(first, second);
    c.cancel_sign_in();
    assert_eq!(c.state(), AuthState::SignedOut);
}
