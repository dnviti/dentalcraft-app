//! Updates of DentalCraft, for the app and `dentalcraft-cli`.
//!
//! 1. **Check** ([`check`]): the website publishes the newest release in
//!    [`MANIFEST_URL`] (`update.json`, see [`Manifest`]) and its minisign
//!    signature next to it (`update.json.minisig`). Nothing in the file is
//!    used before the signature is checked against [`PUBLIC_KEY`], the
//!    release key built into the program. A release is newer by SemVer
//!    precedence ([`is_newer`]).
//! 2. **Plan** ([`plan`]): how the running copy was installed
//!    ([`Installation::detect`]: the `install-kind` marker, `$APPIMAGE`,
//!    `/.flatpak-info`) decides the [`Action`]: DentalCraft installs the
//!    standalone archives, the AppImage and the Windows installers itself;
//!    the package manager updates deb, rpm and Flatpak installations (the
//!    user gets the command); anything else is updated from the website.
//! 3. **Prepare** ([`prepare`]): the download is checked against the size
//!    and SHA-256 the signed manifest announces, then put in place next to
//!    the installation; the installation itself does not change yet.
//! 4. **Finish** ([`Prepared::finish`]): when the program closes, the new
//!    version takes the place of the old one, and starts if asked.
//!
//! The commands [`commands::CHECK`] (`app.update.check`) and
//! [`commands::APPLY`] (`app.update.apply`) expose this to the CLI, the MCP
//! server and the app bridge. Messages for the user are in the `dc-update`
//! Fluent bundle ([`UpdateError::message`]).

mod apply;
mod check;
pub mod commands;
mod download;
mod error;
mod install;
mod manifest;

use std::path::PathBuf;

pub use apply::{Prepared, UPDATE_DIR, cleanup, prepare};
pub use check::{Action, Check, Os, Source, check, plan};
pub use error::UpdateError;
pub use install::{APP_BIN, InstallKind, Installation, MARKER};
pub use manifest::{Asset, AssetKey, Manifest, is_newer, parse_version};

dc_i18n::bundle!(pub fn loader);

/// The running version (the version of the workspace).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Where the website publishes the newest release.
pub const MANIFEST_URL: &str = "https://dentalcraft.app/update/stable.json";

/// Environment variable with another address for the manifest (a test
/// site). The signature is still checked with [`PUBLIC_KEY`].
pub const MANIFEST_URL_ENV: &str = "DC_UPDATE_URL";

/// Minisign public key of the DentalCraft releases (key id
/// `B155D07CF2A36DB0`, `keys/dentalcraft.minisign.pub` on the website).
pub const PUBLIC_KEY: &str = "RWSwbaPyfNBVsS+9xJIPCGLveeo88q6t1ABYCLqowwtBBnlF5am1MsN6";

/// The download page of the website.
pub const DOWNLOAD_PAGE: &str = "https://dentalcraft.app/en/download/";

/// The download page in the language `lang` (`en`, `it`).
pub fn download_page(lang: &str) -> String {
    match lang {
        "it" => "https://dentalcraft.app/it/download/".to_owned(),
        _ => DOWNLOAD_PAGE.to_owned(),
    }
}

/// Where the downloads wait before they are installed: `updates/` in the
/// data folder of DentalCraft (the app's, e.g.
/// `~/.local/share/dentalcraft/updates`), else in the temporary folder.
pub fn work_dir() -> PathBuf {
    directories::ProjectDirs::from("app", "DentalCraft", "dentalcraft").map_or_else(
        || std::env::temp_dir().join("dentalcraft-updates"),
        |p| p.data_dir().join("updates"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translations_are_complete() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("i18n");
        dc_i18n::parity::check_dir(&dir).unwrap();
    }

    /// Words the user is never shown: the names of the tools and checks
    /// behind the update.
    const JARGON: [&str; 14] = [
        "minisign",
        "sha256",
        "sha-256",
        "hash",
        "checksum",
        "signature",
        "firma",
        "manifest",
        "json",
        "ed25519",
        "msiexec",
        "staging",
        "http",
        "powershell",
    ];

    #[test]
    fn every_message_is_translated_in_plain_words() {
        loader();
        for lang in dc_i18n::Lang::ALL {
            let ftl = std::fs::read_to_string(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("i18n")
                    .join(lang.code())
                    .join("dc-update.ftl"),
            )
            .unwrap();
            for line in ftl.lines().filter(|l| !l.starts_with('#')) {
                let text = line.to_lowercase();
                for word in JARGON {
                    assert!(!text.contains(word), "{lang}: {word:?} in {line:?}");
                }
            }
            let ids = dc_i18n::parity::ids(&ftl).unwrap();
            for e in UpdateError::examples() {
                assert!(ids.contains(e.i18n_key()), "{lang}: {}", e.i18n_key());
            }
        }
        // the path is in the message
        let e = UpdateError::NotWritable(PathBuf::from("/opt/dentalcraft"));
        assert!(e.message().contains("/opt/dentalcraft"), "{}", e.message());
    }

    #[test]
    fn the_work_folder_belongs_to_dentalcraft() {
        let w = work_dir();
        assert!(w.ends_with("updates") || w.ends_with("dentalcraft-updates"));
        assert_eq!(download_page("it"), "https://dentalcraft.app/it/download/");
        assert_eq!(download_page("en"), DOWNLOAD_PAGE);
    }
}
