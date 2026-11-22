//! Error codes of the sign-in flow.
//!
//! The UI never shows a raw error string: it maps [`AuthErrorCode`] to a
//! translated message through [`AuthErrorCode::i18n_key`]. The free-text
//! `detail` of an [`AuthError`] is for logs only and never contains tokens.

use std::fmt;

/// What went wrong, in terms the UI can translate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AuthErrorCode {
    /// The configured issuer URL is not usable (not a URL, or plain http on a non-loopback host).
    InvalidIssuer,
    /// The server could not be reached.
    Network,
    /// The server or the user took too long.
    Timeout,
    /// The discovery document or the signing keys are missing or inconsistent.
    DiscoveryFailed,
    /// The loopback listener for the browser redirect could not be opened.
    ListenerFailed,
    /// The browser redirect was malformed, or its `state` or `iss` did not match.
    CallbackInvalid,
    /// The user declined the sign-in in the browser.
    AccessDenied,
    /// The server refused a request (HTTP 4xx other than an invalid grant).
    ServerRejected,
    /// The refresh token or authorization code is no longer valid: sign in again.
    InvalidGrant,
    /// The id_token failed verification (signature, algorithm, issuer, audience or nonce).
    TokenInvalid,
    /// The id_token is valid except for its times (`exp` in the past, `iat` in
    /// the future): the computer clock is wrong. Never forgets the session.
    Clock,
    /// The sign-in was cancelled by the user.
    Cancelled,
    /// The credential store (OS keyring) is not available.
    StorageUnavailable,
    /// An operation that needs a signed-in user was called while signed out.
    NotSignedIn,
    /// Unexpected internal failure (random source, thread, ...).
    Internal,
}

impl AuthErrorCode {
    /// Stable machine-readable name, e.g. `"callback-invalid"`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InvalidIssuer => "invalid-issuer",
            Self::Network => "network",
            Self::Timeout => "timeout",
            Self::DiscoveryFailed => "discovery-failed",
            Self::ListenerFailed => "listener-failed",
            Self::CallbackInvalid => "callback-invalid",
            Self::AccessDenied => "access-denied",
            Self::ServerRejected => "server-rejected",
            Self::InvalidGrant => "invalid-grant",
            Self::TokenInvalid => "token-invalid",
            Self::Clock => "clock",
            Self::Cancelled => "cancelled",
            Self::StorageUnavailable => "storage-unavailable",
            Self::NotSignedIn => "not-signed-in",
            Self::Internal => "internal",
        }
    }

    /// Fluent message id for this error, e.g. `"auth-error-callback-invalid"`.
    pub fn i18n_key(self) -> &'static str {
        match self {
            Self::InvalidIssuer => "auth-error-invalid-issuer",
            Self::Network => "auth-error-network",
            Self::Timeout => "auth-error-timeout",
            Self::DiscoveryFailed => "auth-error-discovery-failed",
            Self::ListenerFailed => "auth-error-listener-failed",
            Self::CallbackInvalid => "auth-error-callback-invalid",
            Self::AccessDenied => "auth-error-access-denied",
            Self::ServerRejected => "auth-error-server-rejected",
            Self::InvalidGrant => "auth-error-invalid-grant",
            Self::TokenInvalid => "auth-error-token-invalid",
            Self::Clock => "auth-error-clock",
            Self::Cancelled => "auth-error-cancelled",
            Self::StorageUnavailable => "auth-error-storage-unavailable",
            Self::NotSignedIn => "auth-error-not-signed-in",
            Self::Internal => "auth-error-internal",
        }
    }

    /// Every code, for the en/it key-parity test of the application.
    pub const ALL: [AuthErrorCode; 15] = [
        Self::InvalidIssuer,
        Self::Network,
        Self::Timeout,
        Self::DiscoveryFailed,
        Self::ListenerFailed,
        Self::CallbackInvalid,
        Self::AccessDenied,
        Self::ServerRejected,
        Self::InvalidGrant,
        Self::TokenInvalid,
        Self::Clock,
        Self::Cancelled,
        Self::StorageUnavailable,
        Self::NotSignedIn,
        Self::Internal,
    ];
}

impl fmt::Display for AuthErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// An error code plus a developer-facing detail (for logs, never for the user).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthError {
    /// The translatable reason.
    pub code: AuthErrorCode,
    /// Free text for logs. Never contains tokens or secrets.
    pub detail: String,
}

impl AuthError {
    /// Builds an error from a code and a log detail.
    pub fn new(code: AuthErrorCode, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }
}

impl From<AuthErrorCode> for AuthError {
    fn from(code: AuthErrorCode) -> Self {
        Self {
            code,
            detail: String::new(),
        }
    }
}

impl fmt::Display for AuthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.detail.is_empty() {
            write!(f, "{}", self.code)
        } else {
            write!(f, "{}: {}", self.code, self.detail)
        }
    }
}

impl std::error::Error for AuthError {}

/// Result alias of this crate.
pub type Result<T> = std::result::Result<T, AuthError>;
