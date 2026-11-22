//! The one-shot loopback server that receives the browser redirect.

use std::collections::HashMap;
use std::io::{ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use url::Url;

use crate::error::{AuthError, AuthErrorCode, Result};
use crate::oidc::Lang;
use crate::pkce::ct_eq;

const POLL: Duration = Duration::from_millis(40);
const CONN_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_HEAD: usize = 8 * 1024;

/// What the redirect carried, validated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallbackParams {
    /// The authorization code.
    pub code: String,
}

/// Checks `state` and `iss` (RFC 9207), then `error`, then `code`.
pub fn validate_callback(
    params: &HashMap<String, String>,
    state: &str,
    issuer: &str,
) -> Result<CallbackParams> {
    let bad = |why: &str| AuthError::new(AuthErrorCode::CallbackInvalid, why.to_owned());
    match params.get("state") {
        Some(s) if ct_eq(s, state) => {}
        _ => return Err(bad("state mismatch")),
    }
    match params.get("iss") {
        Some(i) if i.trim_end_matches('/') == issuer => {}
        _ => return Err(bad("iss mismatch")),
    }
    if let Some(e) = params.get("error") {
        let code = if e == "access_denied" {
            AuthErrorCode::AccessDenied
        } else {
            AuthErrorCode::ServerRejected
        };
        return Err(AuthError::new(code, format!("authorization error: {e}")));
    }
    match params.get("code") {
        Some(c) if !c.is_empty() => Ok(CallbackParams { code: c.clone() }),
        _ => Err(bad("no code")),
    }
}

/// How the request line was understood.
#[derive(Debug, PartialEq, Eq)]
enum Request {
    Callback(HashMap<String, String>),
    Other(u16),
}

fn parse_request(head: &str, port: u16) -> Request {
    let mut lines = head.split("\r\n");
    let mut first = lines.next().unwrap_or("").split(' ');
    let (method, target) = (first.next().unwrap_or(""), first.next().unwrap_or(""));
    let host = lines
        .filter_map(|l| l.split_once(':'))
        .find(|(k, _)| k.eq_ignore_ascii_case("host"))
        .map(|(_, v)| v.trim().to_ascii_lowercase());
    // DNS-rebinding guard: only the literal loopback origin we handed out.
    if host.as_deref() != Some(&format!("127.0.0.1:{port}")) {
        return Request::Other(403);
    }
    if method != "GET" {
        return Request::Other(405);
    }
    let Ok(url) = Url::parse(&format!("http://127.0.0.1:{port}{target}")) else {
        return Request::Other(400);
    };
    if url.path() != "/callback" {
        return Request::Other(404);
    }
    Request::Callback(url.query_pairs().into_owned().collect())
}

fn read_head(stream: &mut TcpStream) -> Option<String> {
    stream.set_nonblocking(false).ok()?;
    stream.set_read_timeout(Some(CONN_TIMEOUT)).ok()?;
    stream.set_write_timeout(Some(CONN_TIMEOUT)).ok()?;
    let mut buf = Vec::with_capacity(512);
    let mut chunk = [0u8; 512];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 || buf.len() + n > MAX_HEAD {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    String::from_utf8(buf).ok()
}

fn respond(stream: &mut TcpStream, status: u16, reason: &str, html: &str) {
    let resp = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nX-Content-Type-Options: nosniff\r\n\
         Content-Security-Policy: default-src 'none'; style-src 'unsafe-inline'\r\nConnection: close\r\n\r\n{html}",
        html.len()
    );
    let _ = stream.write_all(resp.as_bytes());
    let _ = stream.flush();
}

/// The "you can close this window" page.
pub fn success_page(lang: Lang) -> String {
    let (title, text) = match lang {
        Lang::En => (
            "DentalCraft",
            "You can close this window and return to DentalCraft.",
        ),
        Lang::It => (
            "DentalCraft",
            "Puoi chiudere questa finestra e tornare a DentalCraft.",
        ),
    };
    page(lang, title, text)
}

/// The page shown when the redirect was refused.
pub fn failure_page(lang: Lang) -> String {
    let (title, text) = match lang {
        Lang::En => (
            "DentalCraft",
            "Sign-in could not be completed. You can close this window and try again in DentalCraft.",
        ),
        Lang::It => (
            "DentalCraft",
            "Impossibile completare l'accesso. Puoi chiudere questa finestra e riprovare in DentalCraft.",
        ),
    };
    page(lang, title, text)
}

fn page(lang: Lang, title: &str, text: &str) -> String {
    format!(
        "<!doctype html><html lang=\"{}\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
         <title>{title}</title><style>body{{font:16px system-ui,sans-serif;display:grid;place-items:center;min-height:100vh;margin:0;\
         background:#f4f5f7;color:#1c1f24}}main{{max-width:28rem;padding:2rem;text-align:center}}h1{{font-size:1.25rem}}\
         @media(prefers-color-scheme:dark){{body{{background:#16181c;color:#e6e8eb}}}}</style></head>\
         <body><main><h1>{title}</h1><p>{text}</p></main></body></html>",
        lang.code()
    )
}

/// Waits for the redirect on `listener` (already bound to `127.0.0.1`), answers
/// it with a small page and returns the validated code. Other requests, and
/// redirects whose `state` is not ours (any web page or local program can
/// reach the port), get an error status and the wait goes on. Gives up when
/// `cancel` is set ([`AuthErrorCode::Cancelled`]) or `deadline` passes
/// ([`AuthErrorCode::Timeout`]).
pub fn wait_for_callback(
    listener: &TcpListener,
    lang: Lang,
    state: &str,
    issuer: &str,
    cancel: &AtomicBool,
    deadline: Instant,
) -> Result<CallbackParams> {
    let port = listener
        .local_addr()
        .map_err(|e| AuthError::new(AuthErrorCode::ListenerFailed, e.to_string()))?
        .port();
    listener
        .set_nonblocking(true)
        .map_err(|e| AuthError::new(AuthErrorCode::ListenerFailed, e.to_string()))?;
    loop {
        if cancel.load(Ordering::SeqCst) {
            return Err(AuthErrorCode::Cancelled.into());
        }
        if Instant::now() >= deadline {
            return Err(AuthErrorCode::Timeout.into());
        }
        let mut stream = match listener.accept() {
            Ok((s, _)) => s,
            Err(e) if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::Interrupted => {
                std::thread::sleep(POLL);
                continue;
            }
            Err(e) => return Err(AuthError::new(AuthErrorCode::ListenerFailed, e.to_string())),
        };
        let Some(head) = read_head(&mut stream) else {
            continue;
        };
        match parse_request(&head, port) {
            Request::Callback(params) if !params.get("state").is_some_and(|s| ct_eq(s, state)) => {
                // not the answer to our request: it cannot end the sign-in
                respond(&mut stream, 400, "Bad Request", &failure_page(lang));
            }
            Request::Callback(params) => {
                let result = validate_callback(&params, state, issuer);
                match &result {
                    Ok(_) => respond(&mut stream, 200, "OK", &success_page(lang)),
                    Err(_) => respond(&mut stream, 400, "Bad Request", &failure_page(lang)),
                }
                return result;
            }
            Request::Other(status) => {
                let reason = match status {
                    400 => "Bad Request",
                    403 => "Forbidden",
                    404 => "Not Found",
                    _ => "Method Not Allowed",
                };
                respond(&mut stream, status, reason, "");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::net::TcpStream;
    use std::sync::Arc;

    use super::*;

    const ISS: &str = "http://127.0.0.1:8787";

    fn map(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn accepts_matching_state_and_iss() {
        let p = validate_callback(
            &map(&[("code", "dcac_1"), ("state", "s"), ("iss", ISS)]),
            "s",
            ISS,
        )
        .unwrap();
        assert_eq!(p.code, "dcac_1");
        validate_callback(
            &map(&[
                ("code", "c"),
                ("state", "s"),
                ("iss", "http://127.0.0.1:8787/"),
            ]),
            "s",
            ISS,
        )
        .unwrap();
    }

    #[test]
    fn rejects_state_mismatch_or_absence() {
        for p in [
            map(&[("code", "c"), ("state", "other"), ("iss", ISS)]),
            map(&[("code", "c"), ("iss", ISS)]),
            map(&[("code", "c"), ("state", ""), ("iss", ISS)]),
        ] {
            assert_eq!(
                validate_callback(&p, "s", ISS).unwrap_err().code,
                AuthErrorCode::CallbackInvalid
            );
        }
    }

    #[test]
    fn rejects_iss_mismatch_or_absence() {
        for p in [
            map(&[
                ("code", "c"),
                ("state", "s"),
                ("iss", "https://evil.example"),
            ]),
            map(&[("code", "c"), ("state", "s")]),
        ] {
            assert_eq!(
                validate_callback(&p, "s", ISS).unwrap_err().code,
                AuthErrorCode::CallbackInvalid
            );
        }
    }

    #[test]
    fn maps_authorization_errors() {
        let denied = map(&[("error", "access_denied"), ("state", "s"), ("iss", ISS)]);
        assert_eq!(
            validate_callback(&denied, "s", ISS).unwrap_err().code,
            AuthErrorCode::AccessDenied
        );
        let other = map(&[("error", "server_error"), ("state", "s"), ("iss", ISS)]);
        assert_eq!(
            validate_callback(&other, "s", ISS).unwrap_err().code,
            AuthErrorCode::ServerRejected
        );
        // an error response with a foreign state is still a bad callback
        let forged = map(&[("error", "access_denied"), ("state", "x"), ("iss", ISS)]);
        assert_eq!(
            validate_callback(&forged, "s", ISS).unwrap_err().code,
            AuthErrorCode::CallbackInvalid
        );
    }

    #[test]
    fn rejects_missing_code() {
        let p = map(&[("state", "s"), ("iss", ISS)]);
        assert_eq!(
            validate_callback(&p, "s", ISS).unwrap_err().code,
            AuthErrorCode::CallbackInvalid
        );
    }

    #[test]
    fn request_parsing() {
        let ok = "GET /callback?code=a%20b&state=s&iss=http%3A%2F%2F127.0.0.1%3A8787 HTTP/1.1\r\nHost: 127.0.0.1:5000\r\n\r\n";
        let Request::Callback(p) = parse_request(ok, 5000) else {
            panic!("not a callback")
        };
        assert_eq!(p["code"], "a b");
        assert_eq!(p["iss"], "http://127.0.0.1:8787");
        assert_eq!(
            parse_request(
                "GET /favicon.ico HTTP/1.1\r\nHost: 127.0.0.1:5000\r\n\r\n",
                5000
            ),
            Request::Other(404)
        );
        assert_eq!(
            parse_request(
                "POST /callback HTTP/1.1\r\nHost: 127.0.0.1:5000\r\n\r\n",
                5000
            ),
            Request::Other(405)
        );
        assert_eq!(
            parse_request(
                "GET /callback?code=a HTTP/1.1\r\nHost: evil.example:5000\r\n\r\n",
                5000
            ),
            Request::Other(403)
        );
        assert_eq!(
            parse_request(
                "GET /callback?code=a HTTP/1.1\r\nHost: localhost:5000\r\n\r\n",
                5000
            ),
            Request::Other(403)
        );
        assert_eq!(
            parse_request("GET /callback?code=a HTTP/1.1\r\n\r\n", 5000),
            Request::Other(403)
        );
    }

    #[test]
    fn pages_are_localised() {
        assert!(success_page(Lang::En).contains("You can close this window"));
        assert!(success_page(Lang::It).contains("Puoi chiudere questa finestra"));
        assert!(success_page(Lang::It).contains("lang=\"it\""));
        assert!(failure_page(Lang::En).contains("could not be completed"));
        assert!(failure_page(Lang::It).contains("Impossibile"));
    }

    fn fetch(port: u16, request: &str) -> String {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.write_all(request.as_bytes()).unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).unwrap();
        out
    }

    #[test]
    fn serves_one_callback_and_ignores_strays() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let cancel = Arc::new(AtomicBool::new(false));
        let c2 = cancel.clone();
        let t = std::thread::spawn(move || {
            wait_for_callback(
                &listener,
                Lang::It,
                "s",
                ISS,
                &c2,
                Instant::now() + Duration::from_secs(10),
            )
        });
        let stray = fetch(
            port,
            &format!("GET /favicon.ico HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"),
        );
        assert!(stray.starts_with("HTTP/1.1 404"));
        let ok = fetch(
            port,
            &format!(
                "GET /callback?code=dcac_9&state=s&iss=http%3A%2F%2F127.0.0.1%3A8787 HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"
            ),
        );
        assert!(ok.starts_with("HTTP/1.1 200"));
        assert!(ok.contains("Puoi chiudere questa finestra"));
        assert_eq!(t.join().unwrap().unwrap().code, "dcac_9");
    }

    #[test]
    fn a_bad_state_gets_the_failure_page_and_the_wait_goes_on() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let cancel = AtomicBool::new(false);
        let t = std::thread::scope(|sc| {
            let h = sc.spawn(|| {
                wait_for_callback(
                    &listener,
                    Lang::En,
                    "s",
                    ISS,
                    &cancel,
                    Instant::now() + Duration::from_secs(10),
                )
            });
            // a forged redirect (another page, another program) cannot end it
            let resp = fetch(
                port,
                &format!(
                    "GET /callback?code=c&state=evil&iss=http%3A%2F%2F127.0.0.1%3A8787 HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"
                ),
            );
            assert!(resp.starts_with("HTTP/1.1 400"));
            let resp = fetch(
                port,
                &format!("GET /callback?code=c HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"),
            );
            assert!(resp.starts_with("HTTP/1.1 400"));
            assert!(!h.is_finished(), "still waiting for the real redirect");
            let ok = fetch(
                port,
                &format!(
                    "GET /callback?code=dcac_7&state=s&iss=http%3A%2F%2F127.0.0.1%3A8787 HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"
                ),
            );
            assert!(ok.starts_with("HTTP/1.1 200"));
            h.join().unwrap()
        });
        assert_eq!(t.unwrap().code, "dcac_7");
    }

    #[test]
    fn a_wrong_iss_with_our_state_ends_the_wait() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let cancel = AtomicBool::new(false);
        let t = std::thread::scope(|sc| {
            let h = sc.spawn(|| {
                wait_for_callback(
                    &listener,
                    Lang::En,
                    "s",
                    ISS,
                    &cancel,
                    Instant::now() + Duration::from_secs(10),
                )
            });
            let resp = fetch(
                port,
                &format!(
                    "GET /callback?code=c&state=s&iss=https%3A%2F%2Fevil.example HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"
                ),
            );
            assert!(resp.starts_with("HTTP/1.1 400"));
            h.join().unwrap()
        });
        assert_eq!(t.unwrap_err().code, AuthErrorCode::CallbackInvalid);
    }

    #[test]
    fn cancel_and_deadline_stop_the_wait() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let cancel = AtomicBool::new(true);
        let far = Instant::now() + Duration::from_secs(60);
        assert_eq!(
            wait_for_callback(&listener, Lang::En, "s", ISS, &cancel, far)
                .unwrap_err()
                .code,
            AuthErrorCode::Cancelled
        );
        let cancel = AtomicBool::new(false);
        let soon = Instant::now() + Duration::from_millis(120);
        assert_eq!(
            wait_for_callback(&listener, Lang::En, "s", ISS, &cancel, soon)
                .unwrap_err()
                .code,
            AuthErrorCode::Timeout
        );
    }
}
