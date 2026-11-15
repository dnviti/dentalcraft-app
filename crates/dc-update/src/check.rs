//! Is there a newer version, and how is this installation updated?

use serde_json::{Value, json};

use crate::download::get_small;
use crate::{Asset, AssetKey, InstallKind, Installation, Manifest, UpdateError, manifest};

/// Where the update information is read, and the key that must sign it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    /// URL of `update.json`; its signature is the same URL plus `.minisig`.
    pub manifest_url: String,
    /// Minisign public key (base64) that must have signed it.
    pub public_key: String,
}

impl Source {
    /// The DentalCraft website ([`crate::MANIFEST_URL`], or
    /// [`crate::MANIFEST_URL_ENV`] when set) and the release key built into
    /// the program ([`crate::PUBLIC_KEY`]): the address can change, the key
    /// cannot.
    pub fn standard() -> Self {
        let url = std::env::var(crate::MANIFEST_URL_ENV)
            .ok()
            .filter(|u| !u.trim().is_empty())
            .unwrap_or_else(|| crate::MANIFEST_URL.to_owned());
        Self {
            manifest_url: url,
            public_key: crate::PUBLIC_KEY.to_owned(),
        }
    }

    /// URL of the signature.
    pub fn signature_url(&self) -> String {
        format!("{}.minisig", self.manifest_url)
    }

    /// Downloads the manifest and its signature and checks the signature.
    pub fn fetch(&self) -> Result<Manifest, UpdateError> {
        let json = get_small(&self.manifest_url)?;
        let sig = get_small(&self.signature_url())?;
        Manifest::verified(&json, &String::from_utf8_lossy(&sig), &self.public_key)
    }
}

/// The operating systems with downloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    /// Linux (x86_64).
    Linux,
    /// Windows (x86_64).
    Windows,
    /// Anything else: updated from the website.
    Other,
}

impl Os {
    /// The system this program was built for.
    pub const CURRENT: Os = if cfg!(target_os = "linux") {
        Os::Linux
    } else if cfg!(windows) {
        Os::Windows
    } else {
        Os::Other
    };
}

/// What updating this installation means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// This is the newest version: nothing to do.
    None,
    /// DentalCraft downloads `asset` and installs it.
    Install {
        /// Which download.
        key: AssetKey,
        /// The file.
        asset: Asset,
    },
    /// The package manager owns the installation: the user runs `command`.
    PackageManager {
        /// The command (`sudo dnf upgrade dentalcraft`, ...).
        command: &'static str,
    },
    /// The new version is downloaded from the website by hand.
    Manual,
}

impl Action {
    /// `none`, `install`, `package-manager` or `manual`.
    pub fn name(&self) -> &'static str {
        match self {
            Action::None => "none",
            Action::Install { .. } => "install",
            Action::PackageManager { .. } => "package-manager",
            Action::Manual => "manual",
        }
    }
}

/// How an installation of `kind` on `os` takes the release of `manifest`
/// (whether or not it is newer).
pub fn plan(kind: InstallKind, os: Os, manifest: &Manifest) -> Action {
    if let Some(command) = kind.package_command() {
        return Action::PackageManager { command };
    }
    let key = match (kind, os) {
        (InstallKind::Standalone, Os::Linux) => AssetKey::LinuxStandalone,
        (InstallKind::Standalone, Os::Windows) => AssetKey::WindowsStandalone,
        (InstallKind::AppImage, Os::Linux) => AssetKey::AppImage,
        (InstallKind::Exe, Os::Windows) => AssetKey::WindowsExe,
        (InstallKind::Msi, Os::Windows) => AssetKey::WindowsMsi,
        _ => return Action::Manual,
    };
    match manifest.asset(key) {
        Some(asset) => Action::Install {
            key,
            asset: asset.clone(),
        },
        None => Action::Manual,
    }
}

/// The answer of the website for this installation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    /// The running version.
    pub current: String,
    /// The newest release (signature checked).
    pub manifest: Manifest,
    /// This installation.
    pub installation: Installation,
    /// What to do: [`Action::None`] unless the release is newer.
    pub action: Action,
}

impl Check {
    /// The check of `installation`, running `current`, against `manifest`.
    pub fn new(current: &str, manifest: Manifest, installation: Installation) -> Self {
        let action = if manifest::is_newer(current, &manifest.version) {
            plan(installation.kind, Os::CURRENT, &manifest)
        } else {
            Action::None
        };
        Self {
            current: current.to_owned(),
            manifest,
            installation,
            action,
        }
    }

    /// The newest version.
    pub fn latest(&self) -> &str {
        &self.manifest.version
    }

    /// `true` when the release is newer than the running version.
    pub fn update_available(&self) -> bool {
        self.action != Action::None
    }

    /// The page that says what is new: the manifest's, else the download
    /// page.
    pub fn notes_url(&self) -> &str {
        if self.manifest.notes_url.is_empty() {
            crate::DOWNLOAD_PAGE
        } else {
            &self.manifest.notes_url
        }
    }

    /// The output of `app.update.check`: `current`, `latest`,
    /// `update_available`, `kind`, `action`, `notes_url`, plus `command` for
    /// a package manager and `download` (`url`, `size`) for an install.
    pub fn to_json(&self) -> Value {
        let mut out = json!({
            "current": self.current,
            "latest": self.latest(),
            "update_available": self.update_available(),
            "kind": self.installation.kind.name(),
            "action": self.action.name(),
            "notes_url": self.notes_url(),
        });
        match &self.action {
            Action::PackageManager { command } => out["command"] = json!(command),
            Action::Install { asset, .. } => {
                out["download"] = json!({ "url": asset.url, "size": asset.size });
            }
            Action::Manual => out["download_page"] = json!(crate::DOWNLOAD_PAGE),
            Action::None => {}
        }
        out
    }
}

/// Asks `source` for the newest release and works out what it means for
/// `installation` (blocking: call it from a worker thread).
pub fn check(source: &Source, installation: Installation) -> Result<Check, UpdateError> {
    let manifest = source.fetch()?;
    Ok(Check::new(crate::VERSION, manifest, installation))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::download::tests::serve;

    const NEWER: &[u8] = include_bytes!("../tests/fixtures/newer.json");
    const NEWER_SIG: &[u8] = include_bytes!("../tests/fixtures/newer.json.minisig");
    const OLDER: &[u8] = include_bytes!("../tests/fixtures/older.json");
    const OLDER_SIG: &[u8] = include_bytes!("../tests/fixtures/older.json.minisig");
    const TEST_KEY: &str = include_str!("../tests/fixtures/test-key.pub");

    fn newer() -> Manifest {
        Manifest::parse(NEWER).unwrap()
    }

    #[test]
    fn every_installation_gets_its_action() {
        let m = newer();
        let install = |kind, os| match plan(kind, os, &m) {
            Action::Install { key, .. } => Some(key),
            _ => None,
        };
        use AssetKey as K;
        use InstallKind as I;
        assert_eq!(install(I::Standalone, Os::Linux), Some(K::LinuxStandalone));
        assert_eq!(
            install(I::Standalone, Os::Windows),
            Some(K::WindowsStandalone)
        );
        assert_eq!(install(I::AppImage, Os::Linux), Some(K::AppImage));
        assert_eq!(install(I::Exe, Os::Windows), Some(K::WindowsExe));
        assert_eq!(install(I::Msi, Os::Windows), Some(K::WindowsMsi));
        for (kind, command) in [
            (I::Deb, "sudo apt update && sudo apt upgrade dentalcraft"),
            (I::Rpm, "sudo dnf upgrade dentalcraft"),
            (I::Flatpak, "flatpak update com.dentalcraft.DentalCraft"),
        ] {
            assert_eq!(
                plan(kind, Os::Linux, &m),
                Action::PackageManager { command }
            );
        }
        // the website: no marker, no download for this system
        for (kind, os) in [
            (I::Unknown, Os::Linux),
            (I::Unknown, Os::Windows),
            (I::Standalone, Os::Other),
            (I::AppImage, Os::Windows),
            (I::Exe, Os::Linux),
        ] {
            assert_eq!(plan(kind, os, &m), Action::Manual, "{kind:?} {os:?}");
        }
        // a release without that download
        let mut partial = m.clone();
        partial.assets.remove("appimage");
        assert_eq!(plan(I::AppImage, Os::Linux, &partial), Action::Manual);
    }

    #[test]
    fn nothing_to_do_without_a_newer_version() {
        let inst = Installation::new(InstallKind::Standalone, "/opt/dc");
        let c = Check::new("999.0.0", newer(), inst.clone());
        assert_eq!(c.action, Action::None);
        assert!(!c.update_available());
        let v = c.to_json();
        assert_eq!(v["action"], "none");
        assert_eq!(v["update_available"], false);
        // a pre-release of the same version is older
        let c = Check::new("999.0.0-alpha.1", newer(), inst);
        assert!(c.update_available());
    }

    #[test]
    fn the_check_output_says_what_to_do() {
        let deb = Check::new(
            "0.2.0-alpha.3",
            newer(),
            Installation::new(InstallKind::Deb, "/usr/bin"),
        );
        let v = deb.to_json();
        assert_eq!(v["current"], "0.2.0-alpha.3");
        assert_eq!(v["latest"], "999.0.0");
        assert_eq!(v["update_available"], true);
        assert_eq!(v["kind"], "deb");
        assert_eq!(v["action"], "package-manager");
        assert_eq!(
            v["command"],
            "sudo apt update && sudo apt upgrade dentalcraft"
        );
        assert_eq!(v["notes_url"], "https://dentalcraft.app/en/download/");
        let unknown = Check::new(
            "0.2.0-alpha.3",
            newer(),
            Installation::new(InstallKind::Unknown, "/tmp"),
        );
        assert_eq!(unknown.to_json()["action"], "manual");
        assert_eq!(unknown.to_json()["download_page"], crate::DOWNLOAD_PAGE);
        if Os::CURRENT == Os::Linux {
            let appimage = Check::new(
                "0.2.0-alpha.3",
                newer(),
                Installation::new(InstallKind::AppImage, "/tmp"),
            );
            let v = appimage.to_json();
            assert_eq!(v["action"], "install");
            assert!(
                v["download"]["url"]
                    .as_str()
                    .unwrap()
                    .ends_with(".AppImage")
            );
        }
    }

    #[test]
    fn the_check_reads_the_signed_manifest_over_http() {
        let files: HashMap<String, Vec<u8>> = [
            ("newer.json", NEWER),
            ("newer.json.minisig", NEWER_SIG),
            ("older.json", OLDER),
            ("older.json.minisig", OLDER_SIG),
            ("unsigned.json", NEWER),
            ("forged.json", OLDER),
            ("forged.json.minisig", NEWER_SIG),
        ]
        .into_iter()
        .map(|(n, b)| (n.to_owned(), b.to_vec()))
        .collect();
        let base = serve(files);
        let source = |name: &str| Source {
            manifest_url: format!("{base}/{name}"),
            public_key: TEST_KEY.lines().nth(1).unwrap().to_owned(),
        };
        let inst = Installation::new(InstallKind::Rpm, "/usr/bin");
        let c = check(&source("newer.json"), inst.clone()).unwrap();
        assert!(c.update_available());
        assert_eq!(c.latest(), "999.0.0");
        let c = check(&source("older.json"), inst.clone()).unwrap();
        assert!(!c.update_available());
        // no signature at all, or the signature of another file: refused
        assert!(matches!(
            check(&source("unsigned.json"), inst.clone()),
            Err(UpdateError::Server(_))
        ));
        assert_eq!(
            check(&source("forged.json"), inst.clone()),
            Err(UpdateError::NotGenuine)
        );
        // signed with the test key, checked with the release key: refused
        let mut release = source("newer.json");
        release.public_key = crate::PUBLIC_KEY.to_owned();
        assert_eq!(check(&release, inst), Err(UpdateError::NotGenuine));
    }

    #[test]
    fn the_standard_source_is_the_website_and_the_release_key() {
        let s = Source::standard();
        if std::env::var(crate::MANIFEST_URL_ENV).is_err() {
            assert_eq!(s.manifest_url, "https://dentalcraft.app/update/stable.json");
        }
        assert_eq!(s.public_key, crate::PUBLIC_KEY);
        assert!(s.signature_url().ends_with(".json.minisig"));
    }
}
