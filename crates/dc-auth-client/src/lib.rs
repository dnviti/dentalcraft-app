//! Desktop sign-in for DentalCraft.
//!
//! OpenID Connect authorization code flow with PKCE (S256) over a loopback
//! redirect (RFC 8252), against `dentalcraft-server`:
//!
//! 1. [`AuthClient::begin_sign_in`] opens the system browser on the server's
//!    sign-in page and listens on `127.0.0.1:<random port>/callback`.
//! 2. The redirect is checked (`state`, `iss` per RFC 9207) and the code is
//!    exchanged; the id_token is verified (EdDSA against the cached JWKS,
//!    `iss`, `aud`, `exp`, `nonce`) and `/api/v1/me` gives the profile and the
//!    entitlements.
//! 3. The refresh token goes to the OS keyring (service `dentalcraft`, account
//!    = issuer) through a [`TokenStore`]; the access token stays in memory.
//!    Refresh tokens rotate, so refreshes are strictly single-flight.
//!
//! The API is blocking-free for the UI thread: poll [`AuthClient::poll`] each
//! frame (or register [`AuthClient::on_change`] to request a repaint) and map
//! [`AuthState::Error`] through [`AuthErrorCode::i18n_key`].
//!
//! ```no_run
//! use dc_auth_client::{AuthClient, AuthState, Lang, DEFAULT_CLIENT_ID};
//!
//! let auth = AuthClient::new("http://127.0.0.1:8787", DEFAULT_CLIENT_ID).unwrap();
//! auth.try_restore(); // at startup
//! auth.begin_sign_in(Lang::It); // on "Sign in"
//! if let Some(AuthState::SignedIn(user)) = auth.poll() {
//!     println!("{} may open aligners: {}", user.email, user.entitlements.has_module("aligners"));
//! }
//! ```

mod browser;
pub mod callback;
mod client;
mod error;
mod http;
pub mod jwt;
pub mod oidc;
pub mod pkce;
mod store;

pub use browser::{BrowserOpener, SystemBrowser};
pub use client::{
    AuthClient, AuthState, DEFAULT_CLIENT_ID, Entitlements, SCOPES, SIGN_IN_TIMEOUT, UserInfo,
};
pub use error::{AuthError, AuthErrorCode, Result};
pub use oidc::Lang;
pub use store::{KEYRING_SERVICE, KeyringTokenStore, MemoryTokenStore, TokenStore};
