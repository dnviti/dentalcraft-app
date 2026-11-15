//! Persisted settings and the recent projects list (JSON files in the
//! platform config directory, e.g. `~/.config/dentalcraft/`).

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Default issuer in debug builds (`dentalcraft-server dev`).
pub const DEV_ISSUER: &str = "http://127.0.0.1:8787";
/// Default issuer in release builds: the production sign-in server
/// (`dentalcraft-server`). `DC_AUTH_ISSUER` or the saved setting override it.
pub const RELEASE_ISSUER: &str = "https://auth.dentalcraft.app";
/// Environment variable overriding the issuer.
pub const ISSUER_ENV: &str = "DC_AUTH_ISSUER";
/// How many recent projects are kept.
pub const MAX_RECENT: usize = 24;

/// Platform directories of the application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dirs {
    /// Settings and recent projects.
    pub config: PathBuf,
    /// Logs and crash reports.
    pub data: PathBuf,
    /// Default folder of new projects (`~/DentalCraft/Projects`).
    pub projects: PathBuf,
}

impl Dirs {
    /// The standard locations (`directories` crate), or `./dentalcraft-data`
    /// when no home directory is known.
    pub fn standard() -> Self {
        let pd = directories::ProjectDirs::from("app", "DentalCraft", "dentalcraft");
        let home = directories::UserDirs::new().map(|u| u.home_dir().to_path_buf());
        let fallback = PathBuf::from("dentalcraft-data");
        Self {
            config: pd
                .as_ref()
                .map_or(fallback.join("config"), |p| p.config_dir().to_path_buf()),
            data: pd
                .as_ref()
                .map_or(fallback.join("data"), |p| p.data_dir().to_path_buf()),
            projects: home.map_or(fallback.join("Projects"), |h| {
                h.join("DentalCraft").join("Projects")
            }),
        }
    }

    /// All directories under `root` (tests).
    pub fn under(root: &Path) -> Self {
        Self {
            config: root.join("config"),
            data: root.join("data"),
            projects: root.join("Projects"),
        }
    }

    /// Log folder.
    pub fn logs(&self) -> PathBuf {
        self.data.join("logs")
    }
}

/// Theme preference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemePref {
    /// Graphite (default of the design).
    #[default]
    Dark,
    /// Light.
    Light,
    /// Follow the operating system.
    System,
}

impl ThemePref {
    /// The egui preference.
    pub fn egui(self) -> egui::ThemePreference {
        match self {
            ThemePref::Dark => egui::ThemePreference::Dark,
            ThemePref::Light => egui::ThemePreference::Light,
            ThemePref::System => egui::ThemePreference::System,
        }
    }
}

/// User settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Theme.
    pub theme: ThemePref,
    /// Language code (`en`, `it`); system language when absent.
    pub language: Option<String>,
    /// Sign-in server; [`ISSUER_ENV`] wins, then this, then the build default.
    pub auth_issuer: Option<String>,
    /// Look for a newer version at startup (on by default).
    pub check_updates: bool,
    /// A version the user chose to skip: the check at startup does not
    /// offer it again.
    pub skipped_update: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: ThemePref::default(),
            language: None,
            auth_issuer: None,
            check_updates: true,
            skipped_update: None,
        }
    }
}

impl Settings {
    /// The issuer to use: `DC_AUTH_ISSUER`, the setting, the build default.
    pub fn issuer(&self) -> String {
        std::env::var(ISSUER_ENV)
            .ok()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| self.auth_issuer.clone())
            .unwrap_or_else(|| default_issuer().to_owned())
    }

    /// The UI language.
    pub fn lang(&self) -> dc_i18n::Lang {
        self.language
            .as_deref()
            .and_then(dc_i18n::Lang::from_code)
            .unwrap_or_else(dc_i18n::Lang::from_system)
    }
}

/// Build default issuer.
pub fn default_issuer() -> &'static str {
    if cfg!(debug_assertions) {
        DEV_ISSUER
    } else {
        RELEASE_ISSUER
    }
}

/// A recently opened or created project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecentProject {
    /// Path of the `.dcprj`.
    pub path: PathBuf,
    /// Module id (`aligners`).
    #[serde(default)]
    pub module: String,
    /// Last opened or saved, Unix seconds.
    pub touched: u64,
}

impl RecentProject {
    /// File name shown on the card.
    pub fn name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.display().to_string())
    }
}

/// Most recent first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RecentList {
    /// Entries, newest first.
    pub items: Vec<RecentProject>,
}

impl RecentList {
    /// Moves `path` to the front (adding it), keeping [`MAX_RECENT`] entries.
    pub fn touch(&mut self, path: &Path, module: &str) {
        self.items.retain(|r| r.path != path);
        self.items.insert(
            0,
            RecentProject {
                path: path.to_path_buf(),
                module: module.to_owned(),
                touched: now_unix(),
            },
        );
        self.items.truncate(MAX_RECENT);
    }

    /// Removes `path`.
    pub fn remove(&mut self, path: &Path) {
        self.items.retain(|r| r.path != path);
    }
}

/// Current Unix time in seconds.
pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn read_json<T: for<'de> Deserialize<'de> + Default>(path: &Path) -> T {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            tracing::warn!("ignoring unreadable {}: {e}", path.display());
            T::default()
        }),
        Err(_) => T::default(),
    }
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(value)?)?;
    std::fs::rename(tmp, path)
}

/// Settings and recent list stored under [`Dirs::config`].
#[derive(Debug, Clone)]
pub struct Store {
    dirs: Dirs,
}

impl Store {
    /// Store in `dirs`.
    pub fn new(dirs: Dirs) -> Self {
        Self { dirs }
    }

    /// The directories.
    pub fn dirs(&self) -> &Dirs {
        &self.dirs
    }

    fn settings_path(&self) -> PathBuf {
        self.dirs.config.join("settings.json")
    }

    fn recent_path(&self) -> PathBuf {
        self.dirs.config.join("recent-projects.json")
    }

    /// Loads the settings (defaults when missing or unreadable).
    pub fn load_settings(&self) -> Settings {
        read_json(&self.settings_path())
    }

    /// Saves the settings; failures are logged.
    pub fn save_settings(&self, s: &Settings) {
        if let Err(e) = write_json(&self.settings_path(), s) {
            tracing::warn!("cannot save settings: {e}");
        }
    }

    /// Loads the recent projects list.
    pub fn load_recent(&self) -> RecentList {
        read_json(&self.recent_path())
    }

    /// Saves the recent projects list; failures are logged.
    pub fn save_recent(&self, r: &RecentList) {
        if let Err(e) = write_json(&self.recent_path(), r) {
            tracing::warn!("cannot save recent projects: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_and_recent_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(Dirs::under(dir.path()));
        assert_eq!(store.load_settings(), Settings::default());
        let s = Settings {
            theme: ThemePref::Light,
            language: Some("it".into()),
            check_updates: false,
            skipped_update: Some("0.3.0".into()),
            ..Settings::default()
        };
        store.save_settings(&s);
        assert_eq!(store.load_settings(), s);
        assert_eq!(s.lang(), dc_i18n::Lang::It);

        let mut r = RecentList::default();
        for i in 0..30 {
            r.touch(Path::new(&format!("/p/{i}.dcprj")), "aligners");
        }
        r.touch(Path::new("/p/3.dcprj"), "aligners");
        assert_eq!(r.items.len(), MAX_RECENT);
        assert_eq!(r.items[0].path, Path::new("/p/3.dcprj"));
        assert_eq!(r.items[0].name(), "3.dcprj");
        store.save_recent(&r);
        assert_eq!(store.load_recent(), r);
    }

    #[test]
    fn the_update_check_is_on_unless_turned_off() {
        assert!(Settings::default().check_updates);
        // settings saved before the setting existed
        let old: Settings = serde_json::from_str(r#"{"theme": "light"}"#).unwrap();
        assert!(old.check_updates);
        assert_eq!(old.skipped_update, None);
        let off: Settings = serde_json::from_str(r#"{"check_updates": false}"#).unwrap();
        assert!(!off.check_updates);
    }

    #[test]
    fn issuer_default() {
        let s = Settings {
            auth_issuer: Some("https://x.example".into()),
            ..Settings::default()
        };
        if std::env::var(ISSUER_ENV).is_err() {
            assert_eq!(s.issuer(), "https://x.example");
            assert_eq!(Settings::default().issuer(), default_issuer());
        }
    }

    #[test]
    fn issuer_constants() {
        assert_eq!(RELEASE_ISSUER, "https://auth.dentalcraft.app");
        assert_eq!(DEV_ISSUER, "http://127.0.0.1:8787");
        assert!(RELEASE_ISSUER.starts_with("https://"));
        assert_eq!(
            default_issuer(),
            if cfg!(debug_assertions) {
                DEV_ISSUER
            } else {
                RELEASE_ISSUER
            }
        );
    }
}
