//! id_token verification: EdDSA (Ed25519) signature against a JWKS, then the
//! OIDC claim checks (`iss`, `aud`/`azp`, `exp`, `iat`, `nonce`).
//!
//! Only `alg: EdDSA` is accepted: `none`, HMAC and RSA/ECDSA headers are
//! rejected before any key is looked at.

use std::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ed25519_dalek::{Signature, VerifyingKey};
use serde::Deserialize;
use serde_json::Value;

/// Why a token was rejected. Details are for logs; the UI sees
/// [`AuthErrorCode::TokenInvalid`](crate::AuthErrorCode::TokenInvalid).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JwtError {
    /// Not three base64url segments of valid JSON.
    Malformed,
    /// The header `alg` is not `EdDSA` (this includes `none`).
    Algorithm,
    /// The header has a `crit` member, which this verifier does not understand.
    Critical,
    /// No key in the JWKS matches the header `kid`.
    UnknownKey,
    /// The signature does not verify.
    Signature,
    /// `iss` differs from the expected issuer.
    Issuer,
    /// `aud` does not contain the client id, or `azp` differs from it.
    Audience,
    /// `exp` is in the past (beyond the leeway).
    Expired,
    /// `iat` is in the future (beyond the leeway).
    NotYetValid,
    /// A nonce was expected and the token's differs or is missing.
    Nonce,
    /// A required claim (`sub`, `exp`) is missing or has the wrong type.
    MissingClaim,
}

impl fmt::Display for JwtError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Malformed => "malformed token",
            Self::Algorithm => "unsupported alg",
            Self::Critical => "unsupported crit header",
            Self::UnknownKey => "unknown signing key",
            Self::Signature => "bad signature",
            Self::Issuer => "issuer mismatch",
            Self::Audience => "audience mismatch",
            Self::Expired => "token expired",
            Self::NotYetValid => "token issued in the future",
            Self::Nonce => "nonce mismatch",
            Self::MissingClaim => "missing claim",
        })
    }
}

impl std::error::Error for JwtError {}

#[derive(Debug, Clone, Deserialize)]
struct Jwk {
    kty: String,
    #[serde(default)]
    crv: String,
    #[serde(default)]
    kid: Option<String>,
    #[serde(default)]
    x: String,
    #[serde(default, rename = "use")]
    use_: Option<String>,
}

/// A JSON Web Key Set (only OKP/Ed25519 signing keys are kept usable).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Jwks {
    keys: Vec<Jwk>,
}

impl Jwks {
    /// Parses a JWKS document.
    pub fn parse(json: &str) -> Result<Self, JwtError> {
        serde_json::from_str(json).map_err(|_| JwtError::Malformed)
    }

    /// True if a usable key with this `kid` is present.
    pub fn contains(&self, kid: &str) -> bool {
        self.usable().any(|k| k.kid.as_deref() == Some(kid))
    }

    fn usable(&self) -> impl Iterator<Item = &Jwk> {
        self.keys.iter().filter(|k| {
            k.kty == "OKP" && k.crv == "Ed25519" && k.use_.as_deref().is_none_or(|u| u == "sig")
        })
    }

    fn select(&self, kid: Option<&str>) -> Result<VerifyingKey, JwtError> {
        let key = match kid {
            Some(kid) => self.usable().find(|k| k.kid.as_deref() == Some(kid)),
            None => {
                let mut it = self.usable();
                match (it.next(), it.next()) {
                    (Some(k), None) => Some(k),
                    _ => None,
                }
            }
        }
        .ok_or(JwtError::UnknownKey)?;
        let raw = URL_SAFE_NO_PAD
            .decode(&key.x)
            .map_err(|_| JwtError::UnknownKey)?;
        let raw: [u8; 32] = raw.try_into().map_err(|_| JwtError::UnknownKey)?;
        VerifyingKey::from_bytes(&raw).map_err(|_| JwtError::UnknownKey)
    }
}

/// What the verified token must satisfy.
#[derive(Debug, Clone, Copy)]
pub struct Expect<'a> {
    /// Required `iss`.
    pub issuer: &'a str,
    /// Required audience (`aud`, and `azp` if present).
    pub client_id: &'a str,
    /// Required `nonce`; `None` for refresh responses, which carry none.
    pub nonce: Option<&'a str>,
    /// Current time, seconds since the Unix epoch.
    pub now: u64,
    /// Clock-skew tolerance in seconds.
    pub leeway: u64,
}

/// The claims this client uses, after verification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdClaims {
    /// Stable user id.
    pub sub: String,
    /// Expiry, seconds since the Unix epoch.
    pub exp: u64,
}

fn split(token: &str) -> Result<(&str, &str, &str), JwtError> {
    let mut it = token.split('.');
    match (it.next(), it.next(), it.next(), it.next()) {
        (Some(h), Some(p), Some(s), None) if !h.is_empty() && !p.is_empty() => Ok((h, p, s)),
        _ => Err(JwtError::Malformed),
    }
}

fn json_segment(seg: &str) -> Result<Value, JwtError> {
    let bytes = URL_SAFE_NO_PAD
        .decode(seg)
        .map_err(|_| JwtError::Malformed)?;
    serde_json::from_slice(&bytes).map_err(|_| JwtError::Malformed)
}

/// The `kid` of the token header (without verifying anything), so that the
/// caller can fetch the right JWKS first.
pub fn peek_kid(token: &str) -> Result<Option<String>, JwtError> {
    let (h, _, _) = split(token)?;
    let header = json_segment(h)?;
    Ok(header.get("kid").and_then(Value::as_str).map(str::to_owned))
}

/// Verifies signature and claims of an id_token.
pub fn verify_id_token(
    token: &str,
    jwks: &Jwks,
    expect: &Expect<'_>,
) -> Result<IdClaims, JwtError> {
    let (h, p, s) = split(token)?;
    let header = json_segment(h)?;
    if header.get("alg").and_then(Value::as_str) != Some("EdDSA") {
        return Err(JwtError::Algorithm);
    }
    if header.get("crit").is_some() {
        return Err(JwtError::Critical);
    }
    let key = jwks.select(header.get("kid").and_then(Value::as_str))?;
    let sig = URL_SAFE_NO_PAD.decode(s).map_err(|_| JwtError::Malformed)?;
    let sig: [u8; 64] = sig.try_into().map_err(|_| JwtError::Signature)?;
    let signing_input = &token[..h.len() + 1 + p.len()];
    key.verify_strict(signing_input.as_bytes(), &Signature::from_bytes(&sig))
        .map_err(|_| JwtError::Signature)?;

    let claims = json_segment(p)?;
    check_claims(&claims, expect)
}

fn num(v: Option<&Value>) -> Option<f64> {
    v.and_then(Value::as_f64)
        .filter(|n| n.is_finite() && *n >= 0.0)
}

fn check_claims(c: &Value, e: &Expect<'_>) -> Result<IdClaims, JwtError> {
    if c.get("iss").and_then(Value::as_str) != Some(e.issuer) {
        return Err(JwtError::Issuer);
    }
    let aud_ok = match c.get("aud") {
        Some(Value::String(a)) => a == e.client_id,
        Some(Value::Array(a)) => a.iter().any(|v| v.as_str() == Some(e.client_id)),
        _ => false,
    };
    let azp_ok = match c.get("azp") {
        None => true,
        Some(v) => v.as_str() == Some(e.client_id),
    };
    if !aud_ok || !azp_ok {
        return Err(JwtError::Audience);
    }
    let exp = num(c.get("exp")).ok_or(JwtError::MissingClaim)?;
    if exp + (e.leeway as f64) <= e.now as f64 {
        return Err(JwtError::Expired);
    }
    if let Some(iat) = num(c.get("iat"))
        && iat > (e.now + e.leeway) as f64
    {
        return Err(JwtError::NotYetValid);
    }
    if let Some(expected) = e.nonce
        && c.get("nonce").and_then(Value::as_str) != Some(expected)
    {
        return Err(JwtError::Nonce);
    }
    let sub = c
        .get("sub")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or(JwtError::MissingClaim)?;
    Ok(IdClaims {
        sub: sub.to_owned(),
        exp: exp as u64,
    })
}

#[cfg(test)]
mod tests {
    use ed25519_dalek::{Signer, SigningKey};
    use serde_json::json;

    use super::*;

    const ISS: &str = "http://127.0.0.1:8787";
    const CLIENT: &str = "dentalcraft-desktop";
    const NOW: u64 = 1_800_000_000;

    fn key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    fn jwks_for(k: &SigningKey, kid: &str) -> Jwks {
        let x = URL_SAFE_NO_PAD.encode(k.verifying_key().as_bytes());
        Jwks::parse(&json!({"keys": [{"kty": "OKP", "crv": "Ed25519", "kid": kid, "use": "sig", "alg": "EdDSA", "x": x}]}).to_string())
            .unwrap()
    }

    fn b64(v: &Value) -> String {
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(v).unwrap())
    }

    fn sign_with(k: &SigningKey, header: Value, claims: Value) -> String {
        let input = format!("{}.{}", b64(&header), b64(&claims));
        let sig = k.sign(input.as_bytes());
        format!("{input}.{}", URL_SAFE_NO_PAD.encode(sig.to_bytes()))
    }

    fn good_claims() -> Value {
        json!({"iss": ISS, "sub": "usr_1", "aud": CLIENT, "azp": CLIENT, "iat": NOW - 5, "exp": NOW + 3600, "nonce": "n-1"})
    }

    fn sign(k: &SigningKey, claims: Value) -> String {
        sign_with(
            k,
            json!({"alg": "EdDSA", "typ": "JWT", "kid": "k1"}),
            claims,
        )
    }

    fn expect(nonce: Option<&str>) -> Expect<'_> {
        Expect {
            issuer: ISS,
            client_id: CLIENT,
            nonce,
            now: NOW,
            leeway: 60,
        }
    }

    #[test]
    fn accepts_a_valid_token() {
        let k = key(7);
        let c = verify_id_token(
            &sign(&k, good_claims()),
            &jwks_for(&k, "k1"),
            &expect(Some("n-1")),
        )
        .unwrap();
        assert_eq!(
            c,
            IdClaims {
                sub: "usr_1".into(),
                exp: NOW + 3600
            }
        );
    }

    #[test]
    fn refresh_tokens_have_no_nonce() {
        let k = key(7);
        let mut c = good_claims();
        c.as_object_mut().unwrap().remove("nonce");
        verify_id_token(&sign(&k, c.clone()), &jwks_for(&k, "k1"), &expect(None)).unwrap();
        assert_eq!(
            verify_id_token(&sign(&k, c), &jwks_for(&k, "k1"), &expect(Some("n-1"))),
            Err(JwtError::Nonce)
        );
    }

    #[test]
    fn rejects_alg_none() {
        let k = key(7);
        let input = format!(
            "{}.{}",
            b64(&json!({"alg": "none", "kid": "k1"})),
            b64(&good_claims())
        );
        for tok in [format!("{input}."), format!("{input}.AAAA")] {
            assert_eq!(
                verify_id_token(&tok, &jwks_for(&k, "k1"), &expect(Some("n-1"))),
                Err(JwtError::Algorithm)
            );
        }
    }

    #[test]
    fn rejects_other_algorithms() {
        let k = key(7);
        for alg in ["HS256", "RS256", "ES256", "eddsa", "Ed25519"] {
            let t = sign_with(&k, json!({"alg": alg, "kid": "k1"}), good_claims());
            assert_eq!(
                verify_id_token(&t, &jwks_for(&k, "k1"), &expect(Some("n-1"))),
                Err(JwtError::Algorithm),
                "{alg}"
            );
        }
    }

    #[test]
    fn rejects_wrong_audience() {
        let k = key(7);
        let mut c = good_claims();
        c["aud"] = json!("someone-else");
        assert_eq!(
            verify_id_token(&sign(&k, c), &jwks_for(&k, "k1"), &expect(Some("n-1"))),
            Err(JwtError::Audience)
        );
        let mut c = good_claims();
        c["azp"] = json!("someone-else");
        assert_eq!(
            verify_id_token(&sign(&k, c), &jwks_for(&k, "k1"), &expect(Some("n-1"))),
            Err(JwtError::Audience)
        );
        let mut c = good_claims();
        c["aud"] = json!(["other", CLIENT]);
        verify_id_token(&sign(&k, c), &jwks_for(&k, "k1"), &expect(Some("n-1"))).unwrap();
    }

    #[test]
    fn rejects_wrong_issuer() {
        let k = key(7);
        let mut c = good_claims();
        c["iss"] = json!("https://evil.example");
        assert_eq!(
            verify_id_token(&sign(&k, c), &jwks_for(&k, "k1"), &expect(Some("n-1"))),
            Err(JwtError::Issuer)
        );
    }

    #[test]
    fn expiry_honours_the_leeway() {
        let k = key(7);
        let jwks = jwks_for(&k, "k1");
        let mut c = good_claims();
        c["exp"] = json!(NOW - 30);
        verify_id_token(&sign(&k, c), &jwks, &expect(Some("n-1"))).unwrap();
        let mut c = good_claims();
        c["exp"] = json!(NOW - 61);
        assert_eq!(
            verify_id_token(&sign(&k, c), &jwks, &expect(Some("n-1"))),
            Err(JwtError::Expired)
        );
        let mut c = good_claims();
        c.as_object_mut().unwrap().remove("exp");
        assert_eq!(
            verify_id_token(&sign(&k, c), &jwks, &expect(Some("n-1"))),
            Err(JwtError::MissingClaim)
        );
    }

    #[test]
    fn rejects_token_from_the_future() {
        let k = key(7);
        let mut c = good_claims();
        c["iat"] = json!(NOW + 3600);
        assert_eq!(
            verify_id_token(&sign(&k, c), &jwks_for(&k, "k1"), &expect(Some("n-1"))),
            Err(JwtError::NotYetValid)
        );
    }

    #[test]
    fn rejects_wrong_or_missing_nonce() {
        let k = key(7);
        let jwks = jwks_for(&k, "k1");
        assert_eq!(
            verify_id_token(&sign(&k, good_claims()), &jwks, &expect(Some("n-2"))),
            Err(JwtError::Nonce)
        );
        let mut c = good_claims();
        c.as_object_mut().unwrap().remove("nonce");
        assert_eq!(
            verify_id_token(&sign(&k, c), &jwks, &expect(Some("n-1"))),
            Err(JwtError::Nonce)
        );
    }

    #[test]
    fn rejects_bad_signatures_and_keys() {
        let (k, other) = (key(7), key(8));
        let tok = sign(&other, good_claims());
        assert_eq!(
            verify_id_token(&tok, &jwks_for(&k, "k1"), &expect(Some("n-1"))),
            Err(JwtError::Signature)
        );
        // payload tampered after signing
        let good = sign(&k, good_claims());
        let parts: Vec<&str> = good.split('.').collect();
        let mut c = good_claims();
        c["sub"] = json!("usr_2");
        let forged = format!("{}.{}.{}", parts[0], b64(&c), parts[2]);
        assert_eq!(
            verify_id_token(&forged, &jwks_for(&k, "k1"), &expect(Some("n-1"))),
            Err(JwtError::Signature)
        );
        // unknown kid
        assert_eq!(
            verify_id_token(&good, &jwks_for(&k, "k2"), &expect(Some("n-1"))),
            Err(JwtError::UnknownKey)
        );
    }

    #[test]
    fn rejects_garbage() {
        let k = key(7);
        let jwks = jwks_for(&k, "k1");
        for t in ["", "a.b", "a.b.c.d", "!!.!!.!!", "e30.e30.e30"] {
            assert!(verify_id_token(t, &jwks, &expect(None)).is_err(), "{t}");
        }
    }

    #[test]
    fn rejects_crit_header() {
        let k = key(7);
        let t = sign_with(
            &k,
            json!({"alg": "EdDSA", "kid": "k1", "crit": ["exp"]}),
            good_claims(),
        );
        assert_eq!(
            verify_id_token(&t, &jwks_for(&k, "k1"), &expect(Some("n-1"))),
            Err(JwtError::Critical)
        );
    }

    #[test]
    fn non_ed25519_keys_are_ignored() {
        let jwks = Jwks::parse(r#"{"keys":[{"kty":"RSA","kid":"k1","n":"AQAB","e":"AQAB"},{"kty":"OKP","crv":"X25519","kid":"k2","x":"AA"}]}"#)
            .unwrap();
        assert!(!jwks.contains("k1"));
        assert!(!jwks.contains("k2"));
    }

    #[test]
    fn peek_kid_reads_the_header() {
        let k = key(7);
        assert_eq!(
            peek_kid(&sign(&k, good_claims())).unwrap().as_deref(),
            Some("k1")
        );
        assert!(peek_kid("nope").is_err());
    }

    #[test]
    fn missing_kid_is_allowed_with_a_single_key() {
        let k = key(7);
        let t = sign_with(&k, json!({"alg": "EdDSA"}), good_claims());
        verify_id_token(&t, &jwks_for(&k, "k1"), &expect(Some("n-1"))).unwrap();
    }
}
