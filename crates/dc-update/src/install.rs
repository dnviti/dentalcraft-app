//! How this copy of DentalCraft was installed.
//!
//! Every package writes a marker file, [`MARKER`], holding one word and a
//! newline (`standalone`, `appimage`, `deb`, `rpm`, `flatpak`, `exe`, `msi`
//! or `macos-app`):
//!
//! - next to the program: the standalone archives (`.tar.gz`, `.zip`) and
//!   the Windows installers (setup `.exe`, `.msi`);
//! - in `../share/dentalcraft/` from the program's folder: the `.deb` and
//!   `.rpm` packages (`/usr/share/dentalcraft/`), the AppImage
//!   (`$APPDIR/usr/share/dentalcraft/`) and the Flatpak
//!   (`/app/share/dentalcraft/`);
//! - in `../Resources/` from the program's folder: the macOS `.app` bundle
//!   (`DentalCraft.app/Contents/Resources/`; the program is in
//!   `Contents/MacOS/`, where only code may live under a code signature).
//!
//! The environment wins over the marker: `$APPIMAGE` (set by the AppImage
//! runtime to the AppImage file) means an AppImage, `/.flatpak-info` (present
//! in every Flatpak sandbox) a Flatpak. Without either and without a marker
//! the installation is [`InstallKind::Unknown`] (a development build, or a
//! program copied by hand): the website is the way to update it.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Name of the marker file.
pub const MARKER: &str = "install-kind";

/// File name of the app program.
pub const APP_BIN: &str = if cfg!(windows) {
    "dentalcraft.exe"
} else {
    "dentalcraft"
};

/// How DentalCraft was installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InstallKind {
    /// A folder unpacked from the `.tar.gz` or `.zip` archive.
    Standalone,
    /// The AppImage file.
    AppImage,
    /// The `.deb` package (apt).
    Deb,
    /// The `.rpm` package (dnf).
    Rpm,
    /// The Flatpak.
    Flatpak,
    /// The Windows setup program (per user).
    Exe,
    /// The Windows `.msi` package (per machine).
    Msi,
    /// The macOS `DentalCraft.app` bundle from the universal `.zip`.
    MacApp,
    /// No marker: a development build or a copied program.
    Unknown,
}

impl InstallKind {
    /// Every kind.
    pub const ALL: [InstallKind; 9] = [
        InstallKind::Standalone,
        InstallKind::AppImage,
        InstallKind::Deb,
        InstallKind::Rpm,
        InstallKind::Flatpak,
        InstallKind::Exe,
        InstallKind::Msi,
        InstallKind::MacApp,
        InstallKind::Unknown,
    ];

    /// The word of the marker file (`unknown` for [`InstallKind::Unknown`]).
    pub fn name(self) -> &'static str {
        match self {
            InstallKind::Standalone => "standalone",
            InstallKind::AppImage => "appimage",
            InstallKind::Deb => "deb",
            InstallKind::Rpm => "rpm",
            InstallKind::Flatpak => "flatpak",
            InstallKind::Exe => "exe",
            InstallKind::Msi => "msi",
            InstallKind::MacApp => "macos-app",
            InstallKind::Unknown => "unknown",
        }
    }

    /// The kind a marker word names (surrounding blanks ignored).
    pub fn from_name(word: &str) -> Option<InstallKind> {
        let word = word.trim();
        InstallKind::ALL
            .into_iter()
            .find(|k| *k != InstallKind::Unknown && k.name() == word)
    }

    /// The command that updates this installation, when its package
    /// manager owns it (deb, rpm, Flatpak).
    pub fn package_command(self) -> Option<&'static str> {
        match self {
            InstallKind::Deb => Some("sudo apt update && sudo apt upgrade dentalcraft"),
            InstallKind::Rpm => Some("sudo dnf upgrade dentalcraft"),
            InstallKind::Flatpak => Some("flatpak update com.dentalcraft.DentalCraft"),
            _ => None,
        }
    }
}

/// The running installation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installation {
    /// How it was installed.
    pub kind: InstallKind,
    /// Folder of the running program; for a standalone installation, the
    /// folder that is updated (for a macOS bundle, `Contents/MacOS`: see
    /// [`Installation::bundle`]).
    pub dir: PathBuf,
    /// The AppImage file (`$APPIMAGE`) of an [`InstallKind::AppImage`].
    pub appimage: Option<PathBuf>,
}

impl Installation {
    /// The installation of the running program.
    pub fn detect() -> Self {
        let dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(Path::to_path_buf))
            .unwrap_or_default();
        let linux = cfg!(target_os = "linux");
        Self::detect_with(
            &dir,
            std::env::var_os("APPIMAGE").filter(|_| linux),
            linux && Path::new("/.flatpak-info").exists(),
        )
    }

    /// The installation of a program in `exe_dir`, with `$APPIMAGE` and the
    /// presence of `/.flatpak-info` given.
    pub fn detect_with(exe_dir: &Path, appimage: Option<OsString>, flatpak: bool) -> Self {
        let appimage = appimage.filter(|v| !v.is_empty()).map(PathBuf::from);
        let kind = if appimage.is_some() {
            InstallKind::AppImage
        } else if flatpak {
            InstallKind::Flatpak
        } else {
            read_marker(exe_dir).unwrap_or(InstallKind::Unknown)
        };
        Self {
            kind,
            dir: exe_dir.to_path_buf(),
            appimage,
        }
    }

    /// The `.app` bundle that holds the program: `dir` is its
    /// `Contents/MacOS` folder. `None` outside a bundle.
    pub fn bundle(&self) -> Option<PathBuf> {
        bundle_of(&self.dir)
    }

    /// An installation of `kind` in `dir` (tests, tools).
    pub fn new(kind: InstallKind, dir: impl Into<PathBuf>) -> Self {
        Self {
            kind,
            dir: dir.into(),
            appimage: None,
        }
    }
}

/// The `X.app` folder of a program in `X.app/Contents/MacOS`.
fn bundle_of(exe_dir: &Path) -> Option<PathBuf> {
    let contents = exe_dir.parent()?;
    let app = contents.parent()?;
    let is_bundle = exe_dir.file_name()? == "MacOS"
        && contents.file_name()? == "Contents"
        && app.extension()? == "app";
    is_bundle.then(|| app.to_path_buf())
}

/// The marker next to the program, else in `../share/dentalcraft/`, else in
/// `../Resources/` (a macOS bundle).
fn read_marker(exe_dir: &Path) -> Option<InstallKind> {
    let beside = exe_dir.join(MARKER);
    let resources = exe_dir.join("..").join("Resources").join(MARKER);
    let shared = exe_dir
        .join("..")
        .join("share")
        .join("dentalcraft")
        .join(MARKER);
    [beside, shared, resources].into_iter().find_map(|p| {
        let text = std::fs::read_to_string(&p).ok()?;
        let kind = InstallKind::from_name(&text);
        if kind.is_none() {
            tracing::warn!("{}: unknown installation kind {text:?}", p.display());
        }
        kind
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_words_round_trip() {
        for k in InstallKind::ALL {
            let parsed = InstallKind::from_name(&format!("{}\n", k.name()));
            if k == InstallKind::Unknown {
                assert_eq!(parsed, None);
            } else {
                assert_eq!(parsed, Some(k));
            }
        }
        assert_eq!(InstallKind::from_name("  rpm \r\n"), Some(InstallKind::Rpm));
        assert_eq!(InstallKind::from_name("snap"), None);
        assert_eq!(InstallKind::from_name(""), None);
    }

    #[test]
    fn only_package_managers_have_a_command() {
        for k in InstallKind::ALL {
            let managed = matches!(
                k,
                InstallKind::Deb | InstallKind::Rpm | InstallKind::Flatpak
            );
            assert_eq!(k.package_command().is_some(), managed, "{k:?}");
        }
        assert_eq!(
            InstallKind::Flatpak.package_command(),
            Some("flatpak update com.dentalcraft.DentalCraft")
        );
    }

    #[test]
    fn the_marker_next_to_the_program_says_the_kind() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("dentalcraft_1.0.0_linux_x86_64");
        std::fs::create_dir_all(&bin).unwrap();
        assert_eq!(
            Installation::detect_with(&bin, None, false).kind,
            InstallKind::Unknown
        );
        std::fs::write(bin.join(MARKER), "standalone\n").unwrap();
        let i = Installation::detect_with(&bin, None, false);
        assert_eq!(
            (i.kind, i.dir.as_path()),
            (InstallKind::Standalone, bin.as_path())
        );
        // a word the app does not know: unknown
        std::fs::write(bin.join(MARKER), "portable\n").unwrap();
        assert_eq!(
            Installation::detect_with(&bin, None, false).kind,
            InstallKind::Unknown
        );
    }

    #[test]
    fn packages_keep_the_marker_in_share() {
        let root = tempfile::tempdir().unwrap();
        let bin = root.path().join("usr").join("bin");
        let share = root.path().join("usr").join("share").join("dentalcraft");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(&share).unwrap();
        for (word, kind) in [("deb", InstallKind::Deb), ("rpm", InstallKind::Rpm)] {
            std::fs::write(share.join(MARKER), format!("{word}\n")).unwrap();
            assert_eq!(Installation::detect_with(&bin, None, false).kind, kind);
        }
    }

    #[test]
    fn a_macos_bundle_keeps_the_marker_in_resources() {
        let root = tempfile::tempdir().unwrap();
        let app = root.path().join("DentalCraft.app");
        let bin = app.join("Contents").join("MacOS");
        let resources = app.join("Contents").join("Resources");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(&resources).unwrap();
        assert_eq!(
            Installation::detect_with(&bin, None, false).kind,
            InstallKind::Unknown
        );
        std::fs::write(resources.join(MARKER), "macos-app\n").unwrap();
        let i = Installation::detect_with(&bin, None, false);
        assert_eq!(i.kind, InstallKind::MacApp);
        assert_eq!(i.bundle(), Some(app));
        // outside a bundle there is none
        assert_eq!(
            Installation::new(InstallKind::MacApp, bin.join("x")).bundle(),
            None
        );
        assert_eq!(
            Installation::new(InstallKind::Standalone, root.path()).bundle(),
            None
        );
        assert_eq!(
            Installation::new(InstallKind::MacApp, root.path().join("a.app").join("MacOS"))
                .bundle(),
            None
        );
    }

    #[test]
    fn the_environment_wins_over_the_marker() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(MARKER), "deb\n").unwrap();
        let file = OsString::from("/home/u/Apps/DentalCraft-1.0.0-x86_64.AppImage");
        let i = Installation::detect_with(dir.path(), Some(file.clone()), true);
        assert_eq!(i.kind, InstallKind::AppImage);
        assert_eq!(i.appimage, Some(PathBuf::from(file)));
        assert_eq!(
            Installation::detect_with(dir.path(), None, true).kind,
            InstallKind::Flatpak
        );
        // an empty $APPIMAGE is no AppImage
        assert_eq!(
            Installation::detect_with(dir.path(), Some(OsString::new()), false).kind,
            InstallKind::Deb
        );
    }
}
