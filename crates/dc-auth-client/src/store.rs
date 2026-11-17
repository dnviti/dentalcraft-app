//! Where the refresh token lives between runs.
//!
//! The access token never leaves memory. The refresh token goes to the OS
//! credential store (service `dentalcraft`, account = issuer URL). The Linux
//! backend is the Secret Service through `zbus`: pure Rust, no libdbus.
//!
//! Where the OS store cannot be reached (no Secret Service running, a Flatpak
//! sandbox without access to it), [`FallbackTokenStore`] keeps the token in a
//! [`FileTokenStore`] instead: a file only the user can read (mode 0600 on
//! Unix), so the session still survives a restart.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

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

/// Refresh tokens by account in a JSON file that only the user can read
/// (mode 0600 on Unix; on Windows the per-user profile folder protects it).
/// Writes are atomic: temporary file, fsync, rename.
#[derive(Debug)]
pub struct FileTokenStore {
    path: PathBuf,
    lock: Mutex<()>,
}

fn file_err(op: &str, path: &Path, e: &dyn std::fmt::Display) -> AuthError {
    AuthError::new(
        AuthErrorCode::StorageUnavailable,
        format!("token file {op} {}: {e}", path.display()),
    )
}

impl FileTokenStore {
    /// A store kept in the file at `path` (created on the first save).
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            lock: Mutex::new(()),
        }
    }

    /// The file holding the tokens.
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn read(&self) -> Result<BTreeMap<String, String>> {
        match fs::read(&self.path) {
            Ok(bytes) => {
                serde_json::from_slice(&bytes).map_err(|e| file_err("parse", &self.path, &e))
            }
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(BTreeMap::new()),
            Err(e) => Err(file_err("read", &self.path, &e)),
        }
    }

    fn write(&self, tokens: &BTreeMap<String, String>) -> Result<()> {
        if tokens.is_empty() {
            return match fs::remove_file(&self.path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
                Err(e) => Err(file_err("remove", &self.path, &e)),
            };
        }
        let io = |op: &str| {
            let path = self.path.clone();
            let op = op.to_owned();
            move |e: std::io::Error| file_err(&op, &path, &e)
        };
        if let Some(dir) = self.path.parent().filter(|d| !d.as_os_str().is_empty()) {
            fs::create_dir_all(dir).map_err(io("create folder for"))?;
        }
        let body =
            serde_json::to_vec_pretty(tokens).map_err(|e| file_err("encode", &self.path, &e))?;
        let mut tmp = self.path.clone().into_os_string();
        tmp.push(".tmp");
        let tmp = PathBuf::from(tmp);
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp).map_err(io("write"))?;
        // `mode` only applies when the file is created: a leftover temporary file keeps its own.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            f.set_permissions(fs::Permissions::from_mode(0o600))
                .map_err(io("protect"))?;
        }
        f.write_all(&body).map_err(io("write"))?;
        f.sync_all().map_err(io("sync"))?;
        drop(f);
        fs::rename(&tmp, &self.path).map_err(io("replace"))
    }
}

impl TokenStore for FileTokenStore {
    fn load(&self, account: &str) -> Result<Option<String>> {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        Ok(self.read()?.remove(account))
    }

    fn save(&self, account: &str, token: &str) -> Result<()> {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        // An unreadable (corrupt) file is replaced rather than blocking every sign-in.
        let mut tokens = self.read().unwrap_or_default();
        tokens.insert(account.to_owned(), token.to_owned());
        self.write(&tokens)
    }

    fn delete(&self, account: &str) -> Result<()> {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut tokens = self.read()?;
        if tokens.remove(account).is_none() {
            return Ok(());
        }
        self.write(&tokens)
    }
}

type Reporter = Arc<dyn Fn(&AuthError) + Send + Sync>;

/// The `primary` store (the OS keyring), and the `fallback` one (a
/// [`FileTokenStore`]) whenever the primary cannot be reached.
///
/// A token saved to the primary removes any copy in the fallback, so the file
/// only holds a token while the keyring is unavailable. Loading prefers the
/// primary and falls back when it fails or holds nothing (a token saved while
/// it was unavailable). Deleting clears both.
pub struct FallbackTokenStore {
    primary: Arc<dyn TokenStore>,
    fallback: Arc<dyn TokenStore>,
    on_primary_error: Option<Reporter>,
}

impl std::fmt::Debug for FallbackTokenStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FallbackTokenStore").finish_non_exhaustive()
    }
}

impl FallbackTokenStore {
    /// `primary` first, `fallback` when it fails.
    pub fn new(primary: Arc<dyn TokenStore>, fallback: Arc<dyn TokenStore>) -> Self {
        Self {
            primary,
            fallback,
            on_primary_error: None,
        }
    }

    /// The OS keyring, then a [`FileTokenStore`] at `path`.
    pub fn keyring_or_file(path: impl Into<PathBuf>) -> Self {
        Self::new(
            Arc::new(KeyringTokenStore),
            Arc::new(FileTokenStore::new(path)),
        )
    }

    /// Calls `f` each time the primary store fails and the fallback is used
    /// (e.g. to log it; the error never contains tokens).
    #[must_use]
    pub fn on_primary_error(mut self, f: impl Fn(&AuthError) + Send + Sync + 'static) -> Self {
        self.on_primary_error = Some(Arc::new(f));
        self
    }

    fn report(&self, e: &AuthError) {
        if let Some(f) = &self.on_primary_error {
            f(e);
        }
    }
}

impl TokenStore for FallbackTokenStore {
    fn load(&self, account: &str) -> Result<Option<String>> {
        match self.primary.load(account) {
            Ok(Some(t)) => Ok(Some(t)),
            Ok(None) => self.fallback.load(account),
            Err(e) => {
                self.report(&e);
                self.fallback.load(account)
            }
        }
    }

    fn save(&self, account: &str, token: &str) -> Result<()> {
        match self.primary.save(account, token) {
            Ok(()) => {
                // The keyring holds it now: no copy stays on disk. A stale copy
                // is harmless (the keyring wins on load), so a failure is not fatal.
                let _ = self.fallback.delete(account);
                Ok(())
            }
            Err(e) => {
                self.report(&e);
                self.fallback.save(account, token)
            }
        }
    }

    fn delete(&self, account: &str) -> Result<()> {
        let primary = self.primary.delete(account);
        if let Err(e) = &primary {
            self.report(e);
        }
        self.fallback.delete(account).and(primary)
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
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    /// A unique scratch folder under the system temp dir, removed on drop.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let mut rnd = [0u8; 8];
            getrandom::fill(&mut rnd).unwrap();
            let dir = std::env::temp_dir().join(format!(
                "dc-auth-store-{name}-{}",
                rnd.iter().map(|b| format!("{b:02x}")).collect::<String>()
            ));
            Self(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// A keyring that can be switched off.
    #[derive(Default)]
    struct Flaky {
        down: AtomicBool,
        inner: MemoryTokenStore,
    }

    impl Flaky {
        fn check(&self) -> Result<()> {
            if self.down.load(Ordering::SeqCst) {
                Err(AuthError::new(AuthErrorCode::StorageUnavailable, "down"))
            } else {
                Ok(())
            }
        }
    }

    impl TokenStore for Flaky {
        fn load(&self, account: &str) -> Result<Option<String>> {
            self.check()?;
            self.inner.load(account)
        }
        fn save(&self, account: &str, token: &str) -> Result<()> {
            self.check()?;
            self.inner.save(account, token)
        }
        fn delete(&self, account: &str) -> Result<()> {
            self.check()?;
            self.inner.delete(account)
        }
    }

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

    #[test]
    fn file_store_round_trip_survives_a_new_instance() {
        let tmp = Scratch::new("file");
        let path = tmp.0.join("nested").join("auth-tokens.json");
        let s = FileTokenStore::new(&path);
        assert_eq!(s.load("a").unwrap(), None);
        s.save("a", "t1").unwrap();
        s.save("b", "t2").unwrap();
        s.save("a", "t3").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "token file must be private, got {mode:o}");
        }
        // a new process reads what the old one wrote
        let s = FileTokenStore::new(&path);
        assert_eq!(s.load("a").unwrap().as_deref(), Some("t3"));
        assert_eq!(s.load("b").unwrap().as_deref(), Some("t2"));
        s.delete("a").unwrap();
        s.delete("a").unwrap();
        assert_eq!(s.load("a").unwrap(), None);
        assert!(path.exists());
        s.delete("b").unwrap();
        assert!(
            !path.exists(),
            "the last token removed takes the file with it"
        );
    }

    #[test]
    fn file_store_replaces_a_corrupt_file() {
        let tmp = Scratch::new("corrupt");
        fs::create_dir_all(&tmp.0).unwrap();
        let path = tmp.0.join("auth-tokens.json");
        fs::write(&path, b"{not json").unwrap();
        let s = FileTokenStore::new(&path);
        assert_eq!(
            s.load("a").unwrap_err().code,
            AuthErrorCode::StorageUnavailable
        );
        s.save("a", "t1").unwrap();
        assert_eq!(s.load("a").unwrap().as_deref(), Some("t1"));
    }

    #[test]
    fn fallback_store_uses_the_file_only_while_the_keyring_is_down() {
        let tmp = Scratch::new("fallback");
        let path = tmp.0.join("auth-tokens.json");
        let keyring = Arc::new(Flaky::default());
        let reported = Arc::new(AtomicUsize::new(0));
        let r = reported.clone();
        let s = FallbackTokenStore::new(keyring.clone(), Arc::new(FileTokenStore::new(&path)))
            .on_primary_error(move |_| {
                r.fetch_add(1, Ordering::SeqCst);
            });

        // keyring up: nothing on disk
        s.save("a", "t1").unwrap();
        assert!(!path.exists());
        assert_eq!(s.load("a").unwrap().as_deref(), Some("t1"));
        assert_eq!(reported.load(Ordering::SeqCst), 0);

        // keyring down: the file takes over, and a new instance finds the token
        keyring.down.store(true, Ordering::SeqCst);
        s.save("a", "t2").unwrap();
        assert!(path.exists());
        let again = FallbackTokenStore::new(keyring.clone(), Arc::new(FileTokenStore::new(&path)));
        assert_eq!(again.load("a").unwrap().as_deref(), Some("t2"));
        assert!(reported.load(Ordering::SeqCst) >= 1);

        // keyring back: the next save moves the token there and empties the file
        keyring.down.store(false, Ordering::SeqCst);
        keyring.inner.delete("a").unwrap();
        assert_eq!(s.load("a").unwrap().as_deref(), Some("t2"), "from the file");
        s.save("a", "t3").unwrap();
        assert!(!path.exists());
        assert_eq!(keyring.inner.load("a").unwrap().as_deref(), Some("t3"));

        // delete clears both
        keyring.down.store(true, Ordering::SeqCst);
        s.save("a", "t4").unwrap();
        keyring.down.store(false, Ordering::SeqCst);
        s.delete("a").unwrap();
        assert_eq!(s.load("a").unwrap(), None);
        assert!(!path.exists());
    }
}
