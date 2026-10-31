//! Support ticket submission (`POST {issuer}/api/v1/support/tickets`).
//! The server does not implement it yet (501): the UI then offers to copy
//! the diagnostics or write an e-mail instead.

use std::time::Duration;

use serde::Serialize;

/// Support e-mail used when the ticket service is unavailable.
// TODO(release): confirm the support mailbox.
pub const SUPPORT_EMAIL: &str = "support@dentalcraft.app";

/// Ticket fields sent to the server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Ticket {
    /// One-line subject.
    pub subject: String,
    /// Category id (`aligners-staging`, ...).
    pub category: String,
    /// Description.
    pub description: String,
    /// Diagnostics text, when the user attached it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<String>,
    /// Application version.
    pub app_version: String,
}

/// What came back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TicketOutcome {
    /// Accepted; the server's ticket id when it sent one.
    Sent(Option<String>),
    /// The ticket service is not available yet (HTTP 404, 501 or 503).
    NotAvailable,
    /// Anything else (network error, rejected); detail for the log.
    Failed(String),
}

/// Ticket endpoint of `issuer`.
pub fn endpoint(issuer: &str) -> String {
    format!("{}/api/v1/support/tickets", issuer.trim_end_matches('/'))
}

/// Posts `ticket` (blocking: call it from a worker thread).
pub fn submit(issuer: &str, bearer: Option<&str>, ticket: &Ticket) -> TicketOutcome {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(20)))
        .http_status_as_error(false)
        .user_agent(concat!("DentalCraft/", env!("CARGO_PKG_VERSION")))
        .build()
        .into();
    let body = match serde_json::to_string(ticket) {
        Ok(b) => b,
        Err(e) => return TicketOutcome::Failed(e.to_string()),
    };
    let mut req = agent
        .post(endpoint(issuer))
        .header("Accept", "application/json")
        .content_type("application/json");
    if let Some(t) = bearer {
        req = req.header("Authorization", &format!("Bearer {t}"));
    }
    match req.send(body) {
        Ok(mut resp) => {
            let status = resp.status().as_u16();
            match status {
                200..=299 => {
                    let id = resp
                        .body_mut()
                        .read_to_string()
                        .ok()
                        .and_then(|b| serde_json::from_str::<serde_json::Value>(&b).ok())
                        .and_then(|v| v.get("id").and_then(|i| i.as_str()).map(str::to_owned));
                    TicketOutcome::Sent(id)
                }
                404 | 501 | 503 => TicketOutcome::NotAvailable,
                s => TicketOutcome::Failed(format!("HTTP {s}")),
            }
        }
        Err(e) => TicketOutcome::Failed(e.to_string()),
    }
}

/// `mailto:` link with the subject and the body prefilled.
pub fn mailto(ticket: &Ticket) -> String {
    let enc = |s: &str| -> String {
        s.bytes()
            .map(|b| match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    (b as char).to_string()
                }
                _ => format!("%{b:02X}"),
            })
            .collect()
    };
    let mut body = ticket.description.clone();
    if let Some(d) = &ticket.diagnostics {
        body.push_str("\n\n---\n");
        body.push_str(d);
    }
    format!(
        "mailto:{SUPPORT_EMAIL}?subject={}&body={}",
        enc(&format!("[{}] {}", ticket.category, ticket.subject)),
        enc(&body)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_and_mailto() {
        assert_eq!(
            endpoint("http://127.0.0.1:8787/"),
            "http://127.0.0.1:8787/api/v1/support/tickets"
        );
        let t = Ticket {
            subject: "Stage 9 & IPR".into(),
            category: "aligners-staging".into(),
            description: "a b".into(),
            diagnostics: None,
            app_version: "0.1.0".into(),
        };
        let m = mailto(&t);
        assert!(m.starts_with(
            "mailto:support@dentalcraft.app?subject=%5Baligners-staging%5D%20Stage%209%20%26%20IPR"
        ));
        assert!(m.ends_with("body=a%20b"));
    }

    #[test]
    fn unreachable_server_fails_quickly() {
        // port 9 (discard) on loopback is closed on CI and dev machines
        let t = Ticket {
            subject: "s".into(),
            category: "other".into(),
            description: "d".into(),
            diagnostics: None,
            app_version: "0".into(),
        };
        assert!(matches!(
            submit("http://127.0.0.1:9", None, &t),
            TicketOutcome::Failed(_)
        ));
    }
}
