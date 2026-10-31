//! Integration test against the real `dentalcraft-server dev` (Go).
//!
//! Off by default (`#[ignore]`, and a no-op unless `DC_AUTH_IT=1`):
//!
//! ```sh
//! DC_AUTH_IT=1 cargo test -p dc-auth-client --test server_it -- --ignored --nocapture
//! ```
//!
//! Environment: `DC_SERVER_DIR` (default: the sibling `dentalcraft-server`
//! checkout), `DC_SERVER_BIN` (a prebuilt binary; otherwise `go build` into a
//! temp dir), `DC_AUTH_IT_TMP` (temp dir root; default the system temp dir).
//! The server is stopped and the temp dir removed at the end, also on failure.
//! The browser step is simulated with a [`Browser`] that keeps its cookies
//! like a real one: GET the authorize URL, read the CSRF token and then either
//! POST the demo credentials (`/login`) or, when the browser already has a
//! session and the server shows its "Continue as ..." page, POST `/consent`;
//! finally follow the redirect to the loopback listener.

use std::collections::BTreeMap;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dc_auth_client::{AuthClient, AuthState, Lang, MemoryTokenStore, TokenStore};

const ISSUER: &str = "http://127.0.0.1:8787";
const CLIENT: &str = "dentalcraft-desktop";

struct Server {
    child: Child,
    tmp: PathBuf,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.tmp);
    }
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(20)))
        .http_status_as_error(false)
        .max_redirects(0)
        .build()
        .into()
}

fn start_server() -> Server {
    assert!(
        TcpStream::connect("127.0.0.1:8787").is_err(),
        "port 8787 is already in use; refusing to test against a foreign server"
    );
    let root = std::env::var_os("DC_AUTH_IT_TMP")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let tmp = root.join(format!("dc-auth-it-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    let bin = match std::env::var_os("DC_SERVER_BIN") {
        Some(b) => PathBuf::from(b),
        None => {
            let dir = std::env::var_os("DC_SERVER_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../dentalcraft-server")
                });
            let out = tmp.join("dentalcraft-server");
            let status = Command::new("go")
                .args(["build", "-o"])
                .arg(&out)
                .arg("./cmd/dentalcraft-server")
                .current_dir(&dir)
                .status()
                .expect("go is not installed");
            assert!(status.success(), "go build failed in {}", dir.display());
            out
        }
    };
    let child = Command::new(bin)
        .arg("dev")
        .current_dir(&tmp)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("cannot start the server");
    let server = Server { child, tmp };
    let a = agent();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if a.get(&format!("{ISSUER}/healthz"))
            .call()
            .is_ok_and(|r| r.status() == 200)
        {
            return server;
        }
        assert!(Instant::now() < deadline, "server did not become healthy");
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn unescape(s: &str) -> String {
    s.replace("&#34;", "\"")
        .replace("&#39;", "'")
        .replace("&#43;", "+")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// `name -> value` of every `<input type="hidden" ...>` of the login page.
fn hidden_fields(html: &str) -> Vec<(String, String)> {
    const PREFIX: &str = "<input type=\"hidden\" name=\"";
    html.match_indices(PREFIX)
        .map(|(i, _)| {
            let rest = &html[i + PREFIX.len()..];
            let (name, rest) = rest
                .split_once("\" value=\"")
                .expect("hidden input without value");
            let (value, _) = rest.split_once('"').expect("unterminated value");
            (unescape(name), unescape(value))
        })
        .collect()
}

/// A simulated browser: one cookie jar for all the sign-ins made with it.
#[derive(Default)]
struct Browser {
    jar: Mutex<BTreeMap<String, String>>,
    /// Which page each sign-in went through: `login` or `consent`.
    steps: Mutex<Vec<&'static str>>,
    /// Add `prompt=login` to the authorize URL (what "use another account" does).
    force_login: AtomicBool,
}

impl Browser {
    fn cookie_header(&self) -> String {
        self.jar
            .lock()
            .unwrap()
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("; ")
    }

    /// Stores the `Set-Cookie`s of a response (an empty value deletes).
    fn absorb(&self, resp: &ureq::http::Response<ureq::Body>) {
        let mut jar = self.jar.lock().unwrap();
        for c in resp
            .headers()
            .get_all("set-cookie")
            .iter()
            .filter_map(|v| v.to_str().ok())
            .filter_map(|c| c.split(';').next())
        {
            if let Some((k, v)) = c.split_once('=') {
                if v.is_empty() {
                    jar.remove(k);
                } else {
                    jar.insert(k.to_owned(), v.to_owned());
                }
            }
        }
    }

    fn has_cookie(&self, name: &str) -> bool {
        self.jar.lock().unwrap().contains_key(name)
    }

    fn steps(&self) -> Vec<&'static str> {
        self.steps.lock().unwrap().clone()
    }

    /// What a person does in the browser; returns the loopback page they end on.
    fn sign_in(&self, auth_url: &str, email: &str, password: &str) -> String {
        let a = agent();
        let mut auth_url = auth_url.to_owned();
        if self.force_login.load(Ordering::SeqCst) {
            auth_url.push_str("&prompt=login");
        }
        let mut resp = a
            .get(&auth_url)
            .header("Cookie", &self.cookie_header())
            .call()
            .expect("authorize");
        assert_eq!(resp.status(), 200, "authorize page");
        self.absorb(&resp);
        assert!(self.has_cookie("dc_csrf"), "csrf cookie");
        let html = resp.body_mut().read_to_string().unwrap();
        let lang = url::Url::parse(&auth_url)
            .unwrap()
            .query_pairs()
            .find(|(k, _)| k == "ui_locales")
            .map(|(_, v)| v.into_owned());
        if let Some(l) = lang {
            assert!(html.contains(&format!("lang=\"{l}\"")), "page language {l}");
        }
        let mut fields = hidden_fields(&html);
        assert!(fields.iter().any(|(k, _)| k == "csrf_token"));
        let (path, step) = if html.contains("action=\"/consent\"") {
            ("/consent", "consent")
        } else {
            fields.push(("email".into(), email.into()));
            fields.push(("password".into(), password.into()));
            ("/login", "login")
        };
        self.steps.lock().unwrap().push(step);
        let body = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(&fields)
            .finish();
        let mut resp = a
            .post(&format!("{ISSUER}{path}"))
            .header("Cookie", &self.cookie_header())
            .content_type("application/x-www-form-urlencoded")
            .send(body)
            .unwrap_or_else(|e| panic!("{path}: {e}"));
        self.absorb(&resp);
        let status = resp.status().as_u16();
        assert_eq!(
            status,
            303,
            "{path} should redirect, got {status}: {}",
            resp.body_mut().read_to_string().unwrap_or_default()
        );
        let location = resp
            .headers()
            .get("location")
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        assert!(
            location.starts_with("http://127.0.0.1:") && location.contains("/callback?"),
            "{location}"
        );
        let mut cb = a.get(&location).call().expect("callback");
        assert_eq!(cb.status(), 200);
        cb.body_mut().read_to_string().unwrap()
    }
}

fn wait_for(c: &AuthClient, what: &str, pred: impl Fn(&AuthState) -> bool) -> AuthState {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let s = c.state();
        if pred(&s) {
            return s;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}; state: {s:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn browser_client(
    store: Arc<MemoryTokenStore>,
    pages: Arc<Mutex<Vec<String>>>,
    browser: Arc<Browser>,
) -> AuthClient {
    let opener = move |url: &str| {
        let (url, pages, browser) = (url.to_owned(), pages.clone(), browser.clone());
        // a browser is a separate process: never block the auth worker on it
        std::thread::spawn(move || {
            let page = browser.sign_in(&url, "demo@dentalcraft.local", "dentalcraft-demo");
            pages.lock().unwrap().push(page);
        });
        true
    };
    AuthClient::with_parts(ISSUER, CLIENT, store, Arc::new(opener)).unwrap()
}

fn me_status(access_token: &str) -> u16 {
    agent()
        .get(&format!("{ISSUER}/api/v1/me"))
        .header("Authorization", &format!("Bearer {access_token}"))
        .call()
        .unwrap()
        .status()
        .as_u16()
}

fn refresh_grant_status(refresh_token: &str) -> u16 {
    let body = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs([
            ("grant_type", "refresh_token"),
            ("client_id", CLIENT),
            ("refresh_token", refresh_token),
        ])
        .finish();
    agent()
        .post(&format!("{ISSUER}/oauth/token"))
        .content_type("application/x-www-form-urlencoded")
        .send(body)
        .unwrap()
        .status()
        .as_u16()
}

#[test]
#[ignore = "needs Go and port 8787; run with DC_AUTH_IT=1 cargo test -p dc-auth-client --test server_it -- --ignored"]
fn against_dentalcraft_server_dev() {
    if std::env::var("DC_AUTH_IT").as_deref() != Ok("1") {
        eprintln!("DC_AUTH_IT is not 1: skipping");
        return;
    }
    let _server = start_server();
    let store = Arc::new(MemoryTokenStore::new());
    let pages = Arc::new(Mutex::new(Vec::new()));
    // one browser for the whole test: its session cookie outlives the sign-ins
    let browser = Arc::new(Browser::default());

    // --- sign in through the (simulated) browser, Italian pages ---
    let a = browser_client(store.clone(), pages.clone(), browser.clone());
    a.begin_sign_in(Lang::It);
    let AuthState::SignedIn(user) = wait_for(&a, "sign-in", |s| {
        matches!(s, AuthState::SignedIn(_) | AuthState::Error(_))
    }) else {
        panic!("sign-in failed: {:?}", a.state())
    };
    assert_eq!(
        browser.steps(),
        ["login"],
        "no session yet: the sign-in form"
    );
    assert!(user.sub.starts_with("usr_"), "{user:?}");
    assert_eq!(user.email, "demo@dentalcraft.local");
    assert!(user.entitlements.has_module("aligners"), "{user:?}");
    let first_access = a.access_token().expect("access token");
    assert_eq!(me_status(&first_access), 200);
    assert!(
        pages.lock().unwrap()[0].contains("Puoi chiudere questa finestra"),
        "Italian callback page"
    );
    let rt1 = store.load(ISSUER).unwrap().expect("refresh token stored");
    assert!(rt1.starts_with("dcrt_"));
    assert_eq!(a.storage_warning(), None);

    // --- refresh rotates; concurrent callers are serialized, never a reuse ---
    a.refresh_now().unwrap();
    let rt2 = store.load(ISSUER).unwrap().unwrap();
    assert_ne!(rt1, rt2);
    assert_ne!(a.access_token().unwrap(), first_access);
    let hs: Vec<_> = (0..4)
        .map(|_| {
            let a = a.clone();
            std::thread::spawn(move || a.refresh_now())
        })
        .collect();
    for h in hs {
        h.join().unwrap().expect("concurrent refresh");
    }
    assert!(matches!(a.state(), AuthState::SignedIn(_)));
    assert_ne!(store.load(ISSUER).unwrap().unwrap(), rt2);
    assert_eq!(a.ensure_access_token().map(|t| me_status(&t)).unwrap(), 200);

    // --- silent restore at startup (rotates the stored token again) ---
    let stale = store.load(ISSUER).unwrap().unwrap();
    let b = browser_client(store.clone(), pages.clone(), browser.clone());
    assert!(
        matches!(b.restore_blocking(), AuthState::SignedIn(u) if u.email == "demo@dentalcraft.local")
    );
    assert_ne!(store.load(ISSUER).unwrap().unwrap(), stale);

    // --- a spent refresh token is a leak: signed out, token forgotten ---
    // (the server tolerates a retry within 10 s while the successor is unused:
    // use the successor first so that the replay of `stale` counts as reuse)
    b.refresh_now().unwrap();
    store.save(ISSUER, &stale).unwrap();
    let c = browser_client(store.clone(), pages.clone(), browser.clone());
    assert_eq!(c.restore_blocking(), AuthState::SignedOut);
    assert_eq!(store.load(ISSUER).unwrap(), None);

    // --- the browser now has a session, so this sign-in goes through the
    // "Continue as ..." page (POST /consent); then sign out revokes the
    // token family and clears the store ---
    let d = browser_client(store.clone(), pages.clone(), browser.clone());
    d.begin_sign_in(Lang::En);
    wait_for(&d, "second sign-in", |s| {
        matches!(s, AuthState::SignedIn(_) | AuthState::Error(_))
    });
    assert!(
        matches!(d.state(), AuthState::SignedIn(_)),
        "{:?}",
        d.state()
    );
    assert_eq!(
        browser.steps(),
        ["login", "consent"],
        "signed-in browser: the consent page, not the form"
    );
    assert!(
        pages
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .contains("You can close this window")
    );
    let access = d.access_token().unwrap();
    let rt = store.load(ISSUER).unwrap().unwrap();
    d.sign_out_blocking().unwrap();
    assert_eq!(d.state(), AuthState::SignedOut);
    assert_eq!(d.access_token(), None);
    assert_eq!(store.load(ISSUER).unwrap(), None);
    assert_eq!(
        me_status(&access),
        401,
        "access token revoked with its family"
    );
    assert_eq!(refresh_grant_status(&rt), 400, "refresh token revoked");
    assert_eq!(d.restore_blocking(), AuthState::SignedOut);

    // --- "use another account" (prompt=login) forces the form again ---
    browser.force_login.store(true, Ordering::SeqCst);
    let f = browser_client(store.clone(), pages.clone(), browser.clone());
    f.begin_sign_in(Lang::En);
    wait_for(&f, "forced sign-in", |s| {
        matches!(s, AuthState::SignedIn(_) | AuthState::Error(_))
    });
    assert!(
        matches!(f.state(), AuthState::SignedIn(_)),
        "{:?}",
        f.state()
    );
    assert_eq!(browser.steps(), ["login", "consent", "login"]);
    browser.force_login.store(false, Ordering::SeqCst);

    // --- cancelling a sign-in ---
    let e =
        AuthClient::with_parts(ISSUER, CLIENT, store.clone(), Arc::new(|_: &str| true)).unwrap();
    e.begin_sign_in(Lang::En);
    wait_for(&e, "browser step", |s| {
        matches!(s, AuthState::WaitingForBrowser { .. })
    });
    e.cancel_sign_in();
    assert_eq!(e.state(), AuthState::SignedOut);
}
