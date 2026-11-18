//! Workstation information and the diagnostics bundle (zip with app
//! version, OS, GPU adapter and the last log lines; never scans or patient
//! aliases).

use std::io::Write;
use std::path::{Path, PathBuf};

/// GPU adapter as reported by wgpu.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GpuInfo {
    /// Backend (`Vulkan`, `Metal`, `Dx12`, `Gl`).
    pub backend: String,
    /// Adapter name.
    pub name: String,
    /// Driver name and version.
    pub driver: String,
}

impl GpuInfo {
    /// From an eframe wgpu render state.
    pub fn from_render_state(rs: &egui_wgpu::RenderState) -> Self {
        let i = rs.adapter.get_info();
        let driver = [i.driver.trim(), i.driver_info.trim()]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        Self {
            backend: format!("{:?}", i.backend),
            name: i.name,
            driver,
        }
    }

    /// `wgpu Vulkan, NVIDIA RTX A2000` for the status bar.
    pub fn short(&self) -> String {
        format!("wgpu {}, {}", self.backend, self.name)
    }

    /// The adapter as the anonymous diagnostics report it.
    pub fn to_report(&self) -> dc_diagnostics::Gpu {
        dc_diagnostics::Gpu {
            backend: self.backend.clone(),
            name: self.name.clone(),
            driver: self.driver.clone(),
        }
    }
}

/// What the support page and the diagnostics bundle show.
#[derive(Debug, Clone, PartialEq)]
pub struct SystemInfo {
    /// Application version.
    pub version: String,
    /// Build commit.
    pub commit: String,
    /// Operating system (pretty name) and architecture.
    pub os: String,
    /// Renderer, when a GPU is in use.
    pub gpu: Option<GpuInfo>,
    /// Total and available memory in GiB.
    pub memory: Option<(f64, f64)>,
    /// Log folder.
    pub log_dir: PathBuf,
}

impl SystemInfo {
    /// Collects the information (cheap: reads two small files on Linux).
    pub fn collect(gpu: Option<GpuInfo>, log_dir: &Path) -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            commit: option_env!("DC_GIT_COMMIT").unwrap_or("dev").to_owned(),
            os: format!(
                "{}, {}",
                dc_diagnostics::system::os_name(),
                std::env::consts::ARCH
            ),
            gpu,
            memory: dc_diagnostics::system::memory_gib(),
            log_dir: log_dir.to_path_buf(),
        }
    }

    /// Key / value rows (keys are i18n keys of dc-app).
    pub fn rows(&self) -> Vec<(&'static str, String)> {
        let gpu = self.gpu.as_ref();
        vec![
            (
                "support-sys-version",
                format!("{} [{}]", self.version, self.commit),
            ),
            ("support-sys-system", self.os.clone()),
            (
                "support-sys-renderer",
                gpu.map_or_else(|| "-".to_owned(), |g| format!("wgpu, {}", g.backend)),
            ),
            (
                "support-sys-gpu",
                gpu.map_or_else(
                    || "-".to_owned(),
                    |g| {
                        if g.driver.is_empty() {
                            g.name.clone()
                        } else {
                            format!("{}, {}", g.name, g.driver)
                        }
                    },
                ),
            ),
            (
                "support-sys-memory",
                self.memory.map_or_else(
                    || "-".to_owned(),
                    |(t, a)| format!("{t:.0} GB, {a:.1} GB free"),
                ),
            ),
            ("support-sys-logs", tilde(&self.log_dir)),
        ]
    }

    /// Plain-text report (English keys, for support staff).
    pub fn to_text(&self) -> String {
        let mut s = String::new();
        for (k, v) in self.rows() {
            let k = k.trim_start_matches("support-sys-");
            s.push_str(&format!("{k:10} {v}\n"));
        }
        s
    }
}

/// `$HOME/x` shown as `~/x`.
pub fn tilde(p: &Path) -> String {
    if let Some(home) = directories::UserDirs::new().map(|u| u.home_dir().to_path_buf())
        && let Ok(rest) = p.strip_prefix(&home)
    {
        return format!("~/{}", rest.display());
    }
    p.display().to_string()
}

/// Writes the diagnostics zip: `system.txt` and `log-tail.txt` (last 500
/// lines of the newest log).
pub fn export_zip(path: &Path, info: &SystemInfo) -> std::io::Result<()> {
    let file = std::fs::File::create(path)?;
    let mut zip = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default();
    zip.start_file("system.txt", opts)?;
    zip.write_all(info.to_text().as_bytes())?;
    zip.start_file("log-tail.txt", opts)?;
    for line in dc_diagnostics::logs::last_lines(&info.log_dir, 500) {
        zip.write_all(line.as_bytes())?;
        zip.write_all(b"\n")?;
    }
    zip.start_file("README.txt", opts)?;
    zip.write_all(
        b"DentalCraft diagnostics bundle. Contains the application version, \
          operating system, GPU and the last log lines. Patient aliases and scan \
          geometry are never included.\n",
    )?;
    zip.finish()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zip_has_the_expected_entries() {
        let dir = tempfile::tempdir().unwrap();
        let info = SystemInfo::collect(
            Some(GpuInfo {
                backend: "Vulkan".into(),
                name: "Test GPU".into(),
                driver: String::new(),
            }),
            dir.path(),
        );
        assert!(info.to_text().contains("Test GPU"));
        let out = dir.path().join("diag.zip");
        export_zip(&out, &info).unwrap();
        let zip = zip::ZipArchive::new(std::fs::File::open(&out).unwrap()).unwrap();
        let names: Vec<String> = zip
            .file_names()
            .filter_map(Result::ok)
            .map(|n| n.into_owned())
            .collect();
        assert!(
            names.iter().any(|n| n == "system.txt") && names.iter().any(|n| n == "log-tail.txt")
        );
    }
}
