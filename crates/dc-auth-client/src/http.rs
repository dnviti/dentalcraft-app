//! Blocking HTTP with `ureq` (rustls + ring). Every request has a timeout.

use std::time::Duration;

use serde::de::DeserializeOwned;

use crate::error::{AuthError, AuthErrorCode, Result};

/// Whole-request timeout.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_BODY: u64 = 1 << 20;

/// A status code and the (size-limited) text body.
#[derive(Debug)]
pub(crate) struct Reply {
    pub status: u16,
    pub body: String,
}

impl Reply {
    pub fn json<T: DeserializeOwned>(&self) -> Result<T> {
        serde_json::from_str(&self.body).map_err(|e| {
            AuthError::new(
                AuthErrorCode::ServerRejected,
                format!("bad JSON (HTTP {}): {e}", self.status),
            )
        })
    }

    /// The RFC 6749 `error` member of an error body, if any.
    pub fn oauth_error(&self) -> Option<String> {
        serde_json::from_str::<serde_json::Value>(&self.body)
            .ok()?
            .get("error")?
            .as_str()
            .map(str::to_owned)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Http {
    agent: ureq::Agent,
}

fn map_err(e: ureq::Error) -> AuthError {
    match e {
        ureq::Error::Timeout(_) => AuthError::new(AuthErrorCode::Timeout, "request timed out"),
        other => AuthError::new(AuthErrorCode::Network, other.to_string()),
    }
}

impl Http {
    pub fn new() -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(REQUEST_TIMEOUT))
            .timeout_connect(Some(CONNECT_TIMEOUT))
            .http_status_as_error(false)
            .max_redirects(0)
            .user_agent(concat!("DentalCraft/", env!("CARGO_PKG_VERSION")))
            .build()
            .into();
        Self { agent }
    }

    fn read(mut resp: ureq::http::Response<ureq::Body>) -> Result<Reply> {
        let status = resp.status().as_u16();
        let body = resp
            .body_mut()
            .with_config()
            .limit(MAX_BODY)
            .read_to_string()
            .map_err(map_err)?;
        Ok(Reply { status, body })
    }

    pub fn get(&self, url: &str, bearer: Option<&str>) -> Result<Reply> {
        let mut req = self.agent.get(url).header("Accept", "application/json");
        if let Some(t) = bearer {
            req = req.header("Authorization", &format!("Bearer {t}"));
        }
        Self::read(req.call().map_err(map_err)?)
    }

    /// `application/x-www-form-urlencoded` POST. Never sends credentials in a header:
    /// the client is public.
    pub fn post_form(&self, url: &str, fields: &[(&str, &str)]) -> Result<Reply> {
        let body = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(fields)
            .finish();
        let req = self
            .agent
            .post(url)
            .header("Accept", "application/json")
            .content_type("application/x-www-form-urlencoded");
        Self::read(req.send(body).map_err(map_err)?)
    }
}
