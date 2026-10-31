//! OIDC protocol pieces: issuer rules, discovery, the authorization URL,
//! token-endpoint responses.

use serde::Deserialize;
use url::Url;

use crate::error::{AuthError, AuthErrorCode, Result};

/// The language of the sign-in pages (`ui_locales` and the callback page).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Lang {
    /// English (also the fallback).
    #[default]
    En,
    /// Italian.
    It,
}

impl Lang {
    /// The `ui_locales` value.
    pub fn code(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::It => "it",
        }
    }

    /// Parses a BCP 47 tag such as `it`, `it-IT`, `en_US`; unknown tags give English.
    pub fn from_tag(tag: &str) -> Self {
        let primary = tag.split(['-', '_']).next().unwrap_or("");
        if primary.eq_ignore_ascii_case("it") {
            Self::It
        } else {
            Self::En
        }
    }
}

/// Validates the configured issuer: `https`, or `http` on a loopback host only
/// (development). No query, no fragment; the trailing slash is dropped.
pub fn normalize_issuer(issuer: &str) -> Result<String> {
    let bad = |why: &str| AuthError::new(AuthErrorCode::InvalidIssuer, why.to_owned());
    let url = Url::parse(issuer.trim()).map_err(|_| bad("not a URL"))?;
    if url.query().is_some() || url.fragment().is_some() || url.cannot_be_a_base() {
        return Err(bad("query or fragment"));
    }
    let host = url.host_str().ok_or_else(|| bad("no host"))?;
    match url.scheme() {
        "https" => {}
        "http" if is_loopback(host) => {}
        _ => return Err(bad("plain http is only allowed on loopback")),
    }
    Ok(url.as_str().trim_end_matches('/').to_owned())
}

fn is_loopback(host: &str) -> bool {
    matches!(host, "127.0.0.1" | "localhost" | "[::1]" | "::1")
}

fn same_origin(a: &Url, b: &Url) -> bool {
    a.scheme() == b.scheme()
        && a.host_str() == b.host_str()
        && a.port_or_known_default() == b.port_or_known_default()
}

/// The endpoints of the server, from its discovery document.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Discovery {
    /// The issuer, equal to the configured one.
    pub issuer: String,
    /// Where the browser is sent.
    pub authorization_endpoint: String,
    /// Code exchange and refresh.
    pub token_endpoint: String,
    /// RFC 7009 revocation.
    pub revocation_endpoint: String,
    /// Public signing keys.
    pub jwks_uri: String,
}

impl Discovery {
    /// Parses and checks a discovery document: the issuer must match, and
    /// every endpoint must live on the issuer's origin.
    pub fn parse(json: &str, issuer: &str) -> Result<Self> {
        let bad = |why: &str| AuthError::new(AuthErrorCode::DiscoveryFailed, why.to_owned());
        let d: Discovery =
            serde_json::from_str(json).map_err(|e| bad(&format!("bad discovery document: {e}")))?;
        if d.issuer.trim_end_matches('/') != issuer {
            return Err(bad("issuer mismatch"));
        }
        let base = Url::parse(issuer).map_err(|_| bad("issuer"))?;
        for ep in [
            &d.authorization_endpoint,
            &d.token_endpoint,
            &d.revocation_endpoint,
            &d.jwks_uri,
        ] {
            let u = Url::parse(ep).map_err(|_| bad("endpoint is not a URL"))?;
            if !same_origin(&base, &u) {
                return Err(bad("endpoint on a foreign origin"));
            }
        }
        Ok(d)
    }
}

/// Everything that goes into the authorization request.
#[derive(Debug, Clone)]
pub struct AuthorizeParams<'a> {
    /// `authorization_endpoint`.
    pub endpoint: &'a str,
    /// Public client id.
    pub client_id: &'a str,
    /// `http://127.0.0.1:{port}/callback`.
    pub redirect_uri: &'a str,
    /// Space-separated scopes.
    pub scope: &'a str,
    /// CSRF `state`.
    pub state: &'a str,
    /// ID token `nonce`.
    pub nonce: &'a str,
    /// S256 PKCE challenge.
    pub code_challenge: &'a str,
    /// Language of the server's pages.
    pub lang: Lang,
}

/// Builds the URL to open in the browser.
pub fn authorize_url(p: &AuthorizeParams<'_>) -> Result<String> {
    let mut url = Url::parse(p.endpoint)
        .map_err(|_| AuthError::new(AuthErrorCode::DiscoveryFailed, "authorization endpoint"))?;
    url.query_pairs_mut()
        .append_pair("client_id", p.client_id)
        .append_pair("redirect_uri", p.redirect_uri)
        .append_pair("response_type", "code")
        .append_pair("scope", p.scope)
        .append_pair("state", p.state)
        .append_pair("nonce", p.nonce)
        .append_pair("code_challenge", p.code_challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("ui_locales", p.lang.code());
    Ok(url.into())
}

/// The loopback redirect URI for a port. `localhost` is rejected by the server.
pub fn redirect_uri(port: u16) -> String {
    format!("http://127.0.0.1:{port}/callback")
}

/// Answer of `/oauth/token`.
#[derive(Clone, Deserialize)]
pub(crate) struct TokenResponse {
    pub access_token: String,
    #[serde(default)]
    pub expires_in: Option<u64>,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub id_token: Option<String>,
}

impl std::fmt::Debug for TokenResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenResponse")
            .field("expires_in", &self.expires_in)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issuer_rules() {
        assert_eq!(
            normalize_issuer("http://127.0.0.1:8787").unwrap(),
            "http://127.0.0.1:8787"
        );
        assert_eq!(
            normalize_issuer("https://auth.example.com/").unwrap(),
            "https://auth.example.com"
        );
        assert_eq!(
            normalize_issuer("  https://auth.example.com/base/ ").unwrap(),
            "https://auth.example.com/base"
        );
        for bad in [
            "http://auth.example.com",
            "ftp://x",
            "not a url",
            "https://a.example/?x=1",
            "https://a.example/#f",
            "mailto:a@b",
        ] {
            assert_eq!(
                normalize_issuer(bad).unwrap_err().code,
                AuthErrorCode::InvalidIssuer,
                "{bad}"
            );
        }
    }

    #[test]
    fn language_tags() {
        assert_eq!(Lang::from_tag("it"), Lang::It);
        assert_eq!(Lang::from_tag("it-IT"), Lang::It);
        assert_eq!(Lang::from_tag("IT_it"), Lang::It);
        assert_eq!(Lang::from_tag("en-GB"), Lang::En);
        assert_eq!(Lang::from_tag("de"), Lang::En);
        assert_eq!(Lang::from_tag(""), Lang::En);
    }

    #[test]
    fn authorize_url_carries_every_parameter() {
        let p = AuthorizeParams {
            endpoint: "http://127.0.0.1:8787/oauth/authorize",
            client_id: "dentalcraft-desktop",
            redirect_uri: &redirect_uri(53123),
            scope: "openid profile email offline_access",
            state: "st",
            nonce: "no",
            code_challenge: "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM",
            lang: Lang::It,
        };
        let u = Url::parse(&authorize_url(&p).unwrap()).unwrap();
        assert_eq!(u.path(), "/oauth/authorize");
        let q: std::collections::HashMap<_, _> = u.query_pairs().into_owned().collect();
        assert_eq!(q["client_id"], "dentalcraft-desktop");
        assert_eq!(q["redirect_uri"], "http://127.0.0.1:53123/callback");
        assert_eq!(q["response_type"], "code");
        assert_eq!(q["scope"], "openid profile email offline_access");
        assert_eq!(q["state"], "st");
        assert_eq!(q["nonce"], "no");
        assert_eq!(q["code_challenge"], p.code_challenge);
        assert_eq!(q["code_challenge_method"], "S256");
        assert_eq!(q["ui_locales"], "it");
        assert_eq!(q.len(), 9);
        assert!(!q.contains_key("client_secret"));
    }

    fn doc(issuer: &str, authz: &str) -> String {
        format!(
            r#"{{"issuer":"{issuer}","authorization_endpoint":"{authz}","token_endpoint":"{issuer}/oauth/token","revocation_endpoint":"{issuer}/oauth/revoke","jwks_uri":"{issuer}/oauth/jwks.json"}}"#
        )
    }

    #[test]
    fn discovery_checks_issuer_and_origin() {
        let iss = "http://127.0.0.1:8787";
        let d = Discovery::parse(&doc(iss, &format!("{iss}/oauth/authorize")), iss).unwrap();
        assert_eq!(d.token_endpoint, format!("{iss}/oauth/token"));
        assert!(
            Discovery::parse(
                &doc("http://127.0.0.1:9999", "http://127.0.0.1:9999/a"),
                iss
            )
            .is_err()
        );
        assert!(Discovery::parse(&doc(iss, "http://evil.example/oauth/authorize"), iss).is_err());
        assert!(Discovery::parse("{}", iss).is_err());
    }

    #[test]
    fn redirect_uri_is_exact() {
        assert_eq!(redirect_uri(1234), "http://127.0.0.1:1234/callback");
    }
}
