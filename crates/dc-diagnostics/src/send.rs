//! One `POST` of a report (blocking: the worker thread calls it).

use std::time::Duration;

/// What the server answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendOutcome {
    /// Taken (`202`): how many events were new and how many it had already.
    Accepted {
        /// New events.
        accepted: u64,
        /// Events the server already had.
        duplicates: u64,
    },
    /// Refused as invalid (`400`) or too large (`413`): sending it again
    /// would not help, the events are dropped.
    Rejected(u16),
    /// Not taken now (`429`, a server error, no connection, a server
    /// without this service): the events stay and go later, not before
    /// `after` when the server said so.
    RetryLater {
        /// The server's `Retry-After`.
        after: Option<Duration>,
        /// Why, for the log.
        reason: String,
    },
}

/// Posts the report `body` to `endpoint`. No account data is attached:
/// the reports are anonymous.
pub fn post(endpoint: &str, body: &[u8]) -> SendOutcome {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(20)))
        // a redirect could take the report elsewhere (or to plain http)
        .max_redirects(0)
        .http_status_as_error(false)
        .user_agent(concat!("DentalCraft/", env!("CARGO_PKG_VERSION")))
        .build()
        .into();
    let sent = agent
        .post(endpoint)
        .header("Accept", "application/json")
        .content_type("application/json")
        .send(body);
    let mut resp = match sent {
        Ok(r) => r,
        Err(e) => {
            return SendOutcome::RetryLater {
                after: None,
                reason: e.to_string(),
            };
        }
    };
    let status = resp.status().as_u16();
    match status {
        200..=299 => {
            let v = resp
                .body_mut()
                .read_to_string()
                .ok()
                .and_then(|b| serde_json::from_str::<serde_json::Value>(&b).ok())
                .unwrap_or_default();
            SendOutcome::Accepted {
                accepted: v["accepted"].as_u64().unwrap_or(0),
                duplicates: v["duplicates"].as_u64().unwrap_or(0),
            }
        }
        400 | 413 => SendOutcome::Rejected(status),
        s => SendOutcome::RetryLater {
            after: resp
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse::<u64>().ok())
                .map(Duration::from_secs),
            reason: format!("HTTP {s}"),
        },
    }
}

#[cfg(test)]
pub(crate) mod test_server {
    //! A tiny HTTP server on loopback that answers each request with the
    //! next canned response and keeps what it received.

    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    /// A request the server received.
    #[derive(Debug, Clone)]
    pub struct Received {
        pub head: String,
        pub body: String,
    }

    /// Serves `responses` (status, extra header lines, body) in order, then
    /// stops. Returns the base URL and what it received.
    pub fn serve(
        responses: Vec<(u16, &'static str, String)>,
    ) -> (String, Arc<Mutex<Vec<Received>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let got = Arc::new(Mutex::new(Vec::new()));
        let keep = Arc::clone(&got);
        std::thread::spawn(move || {
            for (status, headers, body) in responses {
                let Ok((mut stream, _)) = listener.accept() else {
                    return;
                };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut head = String::new();
                let mut length = 0usize;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = v.trim().parse().unwrap_or(0);
                    }
                    head.push_str(&line);
                }
                let mut buf = vec![0; length];
                let _ = reader.read_exact(&mut buf);
                keep.lock().unwrap().push(Received {
                    head,
                    body: String::from_utf8_lossy(&buf).into_owned(),
                });
                let reply = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(reply.as_bytes());
            }
        });
        (base, got)
    }
}

#[cfg(test)]
mod tests {
    use super::test_server::serve;
    use super::*;

    #[test]
    fn the_answers_of_the_server() {
        let (base, got) = serve(vec![
            (202, "", r#"{"accepted":2,"duplicates":1}"#.into()),
            (429, "Retry-After: 120\r\n", "{}".into()),
            (400, "", r#"{"error":"invalid"}"#.into()),
            (413, "", String::new()),
            (503, "", String::new()),
        ]);
        let url = crate::endpoint(&base);
        assert_eq!(
            post(&url, b"{}"),
            SendOutcome::Accepted {
                accepted: 2,
                duplicates: 1
            }
        );
        assert_eq!(
            post(&url, b"{}"),
            SendOutcome::RetryLater {
                after: Some(Duration::from_secs(120)),
                reason: "HTTP 429".into()
            }
        );
        assert_eq!(post(&url, b"{}"), SendOutcome::Rejected(400));
        assert_eq!(post(&url, b"{}"), SendOutcome::Rejected(413));
        assert!(matches!(
            post(&url, b"{}"),
            SendOutcome::RetryLater { after: None, .. }
        ));
        let got = got.lock().unwrap();
        let head = got[0].head.to_ascii_lowercase();
        assert!(
            head.starts_with("post /api/v1/diagnostics/reports "),
            "{head}"
        );
        assert!(head.contains("user-agent: dentalcraft/"), "{head}");
        assert!(head.contains("content-type: application/json"), "{head}");
        assert!(!head.contains("authorization"), "{head}");
        // nothing listens on port 9: later
        assert!(matches!(
            post("http://127.0.0.1:9/x", b"{}"),
            SendOutcome::RetryLater { .. }
        ));
    }
}
