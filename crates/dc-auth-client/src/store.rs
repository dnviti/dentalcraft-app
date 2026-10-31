//! Where the refresh token lives between runs.
//!
//! The access token never leaves memory. The refresh token goes to the OS
//! credential store (service `dentalcraft`, account = issuer URL). The Linux
//! backend is the Secret Service through `zbus`: pure Rust, no libdbus.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::error::{AuthError, AuthErrorCode, Result};

/// Service name under which the refresh token is stored in the OS keyring.
pub const KEYRING_SERVICE: &str = "dentalcraft";

/// Persistent storage of one refresh token per account (the issuer URL).
pub trait TokenStore: Send + Sync {
    /// The stored refresh token, if any.
    fn load(&self, account: &str) -> Result<Option<String>>;
    /// Replaces the stored refresh token.
    fn save(&self, account: &str, token: &str) -> Result<()>;
    /// Removes the stored refresh token; absent is not an error.
    fn delete(&self, account: &str) -> Result<()>;
}

/// The OS credential store: Secret Service (Linux), Keychain (macOS),
/// Credential Manager (Windows).
#[derive(Debug, Default, Clone, Copy)]
pub struct KeyringTokenStore;

fn store_err(op: &str, e: &keyring::Error) -> AuthError {
    AuthError::new(
        AuthErrorCode::StorageUnavailable,
        format!("keyring {op}: {e}"),
    )
}

impl KeyringTokenStore {
    fn entry(account: &str) -> Result<keyring::Entry> {
        keyring::Entry::new(KEYRING_SERVICE, account).map_err(|e| store_err("open", &e))
    }
}

impl TokenStore for KeyringTokenStore {
    fn load(&self, account: &str) -> Result<Option<String>> {
        match Self::entry(account)?.get_password() {
            Ok(t) => Ok(Some(t)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(store_err("read", &e)),
        }
    }

    fn save(&self, account: &str, token: &str) -> Result<()> {
        Self::entry(account)?
            .set_password(token)
            .map_err(|e| store_err("write", &e))
    }

    fn delete(&self, account: &str) -> Result<()> {
        match Self::entry(account)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(store_err("delete", &e)),
        }
    }
}

/// A process-local store: for tests, and for running without a keyring.
#[derive(Debug, Default)]
pub struct MemoryTokenStore {
    tokens: Mutex<HashMap<String, String>>,
}

impl MemoryTokenStore {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }
}

impl TokenStore for MemoryTokenStore {
    fn load(&self, account: &str) -> Result<Option<String>> {
        Ok(self
            .tokens
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(account)
            .cloned())
    }

    fn save(&self, account: &str, token: &str) -> Result<()> {
        self.tokens
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(account.to_owned(), token.to_owned());
        Ok(())
    }

    fn delete(&self, account: &str) -> Result<()> {
        self.tokens
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(account);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_store_round_trip() {
        let s = MemoryTokenStore::new();
        assert_eq!(s.load("a").unwrap(), None);
        s.save("a", "t1").unwrap();
        s.save("b", "t2").unwrap();
        s.save("a", "t3").unwrap();
        assert_eq!(s.load("a").unwrap().as_deref(), Some("t3"));
        assert_eq!(s.load("b").unwrap().as_deref(), Some("t2"));
        s.delete("a").unwrap();
        s.delete("a").unwrap();
        assert_eq!(s.load("a").unwrap(), None);
    }
}
