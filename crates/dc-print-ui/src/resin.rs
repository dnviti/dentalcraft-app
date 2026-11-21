//! The user's resin: the values its maker gives for this printer, entered
//! once and kept in `print-resin.json` in the app's settings folder (the
//! folder of `settings.json`). Nothing is guessed: without a saved resin
//! the popup cannot prepare a print.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use dc_print::goo::JobSettings;
use serde::{Deserialize, Serialize};

/// Name of the file in the settings folder.
pub const RESIN_FILE: &str = "print-resin.json";

/// The resin values of the maker, for this printer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Resin {
    /// Resin name, written into the print files (1–31 plain ASCII
    /// characters).
    pub name: String,
    /// Layer height, µm.
    pub layer_um: u32,
    /// Exposure of the normal layers, s.
    pub exposure_s: f32,
    /// Exposure of the first (bottom) layers, s.
    pub bottom_exposure_s: f32,
    /// Number of bottom layers.
    pub bottom_layers: u32,
    /// Rest before each exposure, s.
    pub rest_s: f32,
    /// Density, g/ml.
    pub density_g_ml: f32,
}

impl Resin {
    /// The print settings of a job with this resin (volume, weight and time
    /// are filled in per plate by the slicer).
    pub fn job_settings(&self) -> JobSettings {
        JobSettings {
            layer_um: self.layer_um,
            exposure_s: self.exposure_s,
            bottom_exposure_s: self.bottom_exposure_s,
            bottom_layers: self.bottom_layers,
            rest_before_exposure_s: self.rest_s,
            pwm: 255,
            resin_name: self.name.clone(),
            volume_mm3: 0.0,
            weight_g: 0.0,
            print_time_s: 0,
        }
    }
}

/// Where the resin is kept: a file, or nowhere (tests, headless hosts: the
/// values live as long as the popup).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResinStore {
    path: Option<PathBuf>,
}

impl ResinStore {
    /// The file `path`.
    pub fn file(path: impl Into<PathBuf>) -> Self {
        Self {
            path: Some(path.into()),
        }
    }

    /// Nothing is read or written.
    pub fn memory() -> Self {
        Self { path: None }
    }

    /// `print-resin.json` in the app's settings folder (the one of
    /// `settings.json`: `~/.config/dentalcraft`, `%APPDATA%\DentalCraft\
    /// dentalcraft\config`, `~/Library/Application Support/app.DentalCraft.
    /// dentalcraft`); in memory when the platform has no such folder.
    pub fn standard() -> Self {
        directories::ProjectDirs::from("app", "DentalCraft", "dentalcraft")
            .map_or_else(Self::memory, |d| {
                Self::file(d.config_dir().join(RESIN_FILE))
            })
    }

    /// The file, if any.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// The saved resin; `None` when there is none or the file cannot be
    /// read (logged).
    pub fn load(&self) -> Option<Resin> {
        let path = self.path.as_ref()?;
        let text = match fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return None,
            Err(e) => {
                tracing::warn!(code = "print-resin-read", "{}: {e}", path.display());
                return None;
            }
        };
        match serde_json::from_str(&text) {
            Ok(r) => Some(r),
            Err(e) => {
                tracing::warn!(code = "print-resin-read", "{}: {e}", path.display());
                None
            }
        }
    }

    /// Saves `resin` atomically (a `.tmp` file, synced, then renamed).
    ///
    /// # Errors
    /// The folder or the file cannot be written.
    pub fn save(&self, resin: &Resin) -> io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_vec_pretty(resin).map_err(io::Error::other)?;
        let tmp = path.with_extension("json.tmp");
        {
            let mut f = fs::File::create(&tmp)?;
            f.write_all(&json)?;
            f.sync_all()?;
        }
        fs::rename(&tmp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resin() -> Resin {
        Resin {
            name: "Clear aligner".into(),
            layer_um: 100,
            exposure_s: 5.7,
            bottom_exposure_s: 35.0,
            bottom_layers: 4,
            rest_s: 1.0,
            density_g_ml: 1.1,
        }
    }

    #[test]
    fn a_saved_resin_is_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let store = ResinStore::file(dir.path().join("sub").join(RESIN_FILE));
        assert_eq!(store.load(), None);
        store.save(&resin()).unwrap();
        assert_eq!(store.load(), Some(resin()));
        assert!(!dir.path().join("sub/print-resin.json.tmp").exists());
    }

    #[test]
    fn a_memory_store_keeps_nothing() {
        let store = ResinStore::memory();
        store.save(&resin()).unwrap();
        assert_eq!(store.load(), None);
    }

    #[test]
    fn the_job_settings_carry_the_values() {
        let j = resin().job_settings();
        assert_eq!(j.layer_um, 100);
        assert_eq!(j.bottom_layers, 4);
        assert_eq!(j.rest_before_exposure_s, 1.0);
        assert_eq!(j.resin_name, "Clear aligner");
    }
}
