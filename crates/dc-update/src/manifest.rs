//! The update manifest (`update.json`), its signature, and versions.
//!
//! ```json
//! {"version": "0.3.0", "tag": "v0.3.0", "date": "2026-11-20",
//!  "notes_url": "https://dentalcraft.app/en/download/",
//!  "assets": {"appimage": {"url": "https://dentalcraft.app/download/DentalCraft-0.3.0-x86_64.AppImage",
//!                          "sha256": "<64 hex>", "size": 123}, ...}}
//! ```
//!
//! The release job signs the exact bytes with minisign (prehashed, the
//! default); [`Manifest::verified`] checks that signature against the key
//! built into the program before anything in the file is used.

use std::collections::BTreeMap;

use minisign_verify::{PublicKey, Signature};
use semver::Version;
use serde::Deserialize;

use crate::UpdateError;

/// One downloadable file of a release.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Asset {
    /// Where it is downloaded from (`https://dentalcraft.app/download/<file>`).
    pub url: String,
    /// SHA-256 of the file, hexadecimal.
    pub sha256: String,
    /// Size in bytes.
    pub size: u64,
}

impl Asset {
    /// File name: the last segment of the URL path.
    pub fn file_name(&self) -> &str {
        let path = self.url.split(['?', '#']).next().unwrap_or_default();
        path.rsplit('/').next().unwrap_or_default()
    }
}

/// The downloads in `assets`, one per kind of installation DentalCraft
/// updates by itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AssetKey {
    /// `dentalcraft_V_linux_x86_64.tar.gz`.
    LinuxStandalone,
    /// `DentalCraft-V-x86_64.AppImage`.
    AppImage,
    /// `dentalcraft_V_windows_x86_64.zip`.
    WindowsStandalone,
    /// `DentalCraft-V-setup.exe`.
    WindowsExe,
    /// `DentalCraft-V-x86_64.msi`.
    WindowsMsi,
}

impl AssetKey {
    /// Every key.
    pub const ALL: [AssetKey; 5] = [
        AssetKey::LinuxStandalone,
        AssetKey::AppImage,
        AssetKey::WindowsStandalone,
        AssetKey::WindowsExe,
        AssetKey::WindowsMsi,
    ];

    /// The key in `update.json`.
    pub fn name(self) -> &'static str {
        match self {
            AssetKey::LinuxStandalone => "linux-standalone",
            AssetKey::AppImage => "appimage",
            AssetKey::WindowsStandalone => "windows-standalone",
            AssetKey::WindowsExe => "windows-exe",
            AssetKey::WindowsMsi => "windows-msi",
        }
    }
}

/// `update.json`: the newest release on the website.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Manifest {
    /// Version, SemVer without a leading `v` (`0.3.0`, `0.3.0-alpha.1`).
    pub version: String,
    /// Git tag (`v0.3.0`).
    #[serde(default)]
    pub tag: String,
    /// Release date (`YYYY-MM-DD`).
    #[serde(default)]
    pub date: String,
    /// Page that says what is new.
    #[serde(default)]
    pub notes_url: String,
    /// Downloads by key ([`AssetKey::name`]); any may be missing.
    #[serde(default)]
    pub assets: BTreeMap<String, Asset>,
}

impl Manifest {
    /// Reads `update.json` and checks its fields: a valid version, and for
    /// every asset an `https` URL (plain `http` only to this computer, for
    /// tests), a 64-digit hexadecimal SHA-256 and a size.
    pub fn parse(bytes: &[u8]) -> Result<Self, UpdateError> {
        let m: Manifest = serde_json::from_slice(bytes)
            .map_err(|e| UpdateError::Server(format!("update.json: {e}")))?;
        parse_version(&m.version)
            .map_err(|e| UpdateError::Server(format!("version {:?}: {e}", m.version)))?;
        for (key, a) in &m.assets {
            let bad = |what: String| UpdateError::Server(format!("asset {key}: {what}"));
            check_url(&a.url).map_err(bad)?;
            if a.sha256.len() != 64 || !a.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(bad(format!("sha256 {:?} is not 64 hex digits", a.sha256)));
            }
            if a.size == 0 {
                return Err(bad("size 0".into()));
            }
            if a.file_name().is_empty() {
                return Err(bad(format!("no file name in {}", a.url)));
            }
        }
        Ok(m)
    }

    /// Checks that `signature` (the `.minisig` file) signs exactly `bytes`
    /// with `public_key` (base64, the second line of a minisign `.pub`),
    /// then reads the manifest. Only prehashed signatures are accepted.
    pub fn verified(bytes: &[u8], signature: &str, public_key: &str) -> Result<Self, UpdateError> {
        let key = PublicKey::from_base64(public_key.trim())
            .map_err(|e| UpdateError::Server(format!("public key: {e}")))?;
        let sig = Signature::decode(signature).map_err(|e| {
            tracing::warn!("update signature unreadable: {e}");
            UpdateError::NotGenuine
        })?;
        key.verify(bytes, &sig, false).map_err(|e| {
            tracing::warn!("update signature refused: {e}");
            UpdateError::NotGenuine
        })?;
        Self::parse(bytes)
    }

    /// The download for `key`, when the release has one.
    pub fn asset(&self, key: AssetKey) -> Option<&Asset> {
        self.assets.get(key.name())
    }
}

/// Reads a version: SemVer, a leading `v` allowed (`v0.3.0-alpha.1`).
pub fn parse_version(s: &str) -> Result<Version, semver::Error> {
    Version::parse(s.trim().trim_start_matches('v'))
}

/// `true` when `latest` comes after `current` in SemVer order: a
/// pre-release comes before its release (`0.3.0-alpha.2` < `0.3.0`), and
/// pre-release fields compare numerically when they are numbers
/// (`alpha.9` < `alpha.10`). An unreadable version is never newer.
pub fn is_newer(current: &str, latest: &str) -> bool {
    match (parse_version(current), parse_version(latest)) {
        (Ok(c), Ok(l)) => l.cmp_precedence(&c).is_gt(),
        _ => false,
    }
}

/// An `https` URL, or plain `http` to this computer (tests).
pub(crate) fn check_url(url: &str) -> Result<(), String> {
    let uri: ureq::http::Uri = url.parse().map_err(|e| format!("URL {url:?}: {e}"))?;
    let host = uri.host().unwrap_or_default();
    match uri.scheme_str() {
        Some("https") if !host.is_empty() => Ok(()),
        Some("http") if matches!(host, "127.0.0.1" | "localhost" | "[::1]") => Ok(()),
        _ => Err(format!("URL {url:?} is not https")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) const TEST_KEY: &str = include_str!("../tests/fixtures/test-key.pub");
    const NEWER: &[u8] = include_bytes!("../tests/fixtures/newer.json");
    const NEWER_SIG: &str = include_str!("../tests/fixtures/newer.json.minisig");
    const NEWER_LEGACY_SIG: &str = include_str!("../tests/fixtures/newer.json.legacy.minisig");
    const OLDER: &[u8] = include_bytes!("../tests/fixtures/older.json");
    const OLDER_SIG: &str = include_str!("../tests/fixtures/older.json.minisig");

    fn test_key() -> &'static str {
        TEST_KEY.lines().nth(1).unwrap()
    }

    #[test]
    fn versions_follow_semver_precedence() {
        assert!(is_newer("0.2.0-alpha.3", "0.2.0-alpha.4"));
        assert!(
            is_newer("0.2.0-alpha.9", "0.2.0-alpha.10"),
            "numeric fields"
        );
        assert!(
            is_newer("0.2.0-alpha.4", "0.2.0"),
            "a release follows its pre-releases"
        );
        assert!(is_newer("0.2.0-alpha.1", "0.2.0-beta.1"));
        assert!(is_newer("0.2.0", "0.2.1-alpha.1"));
        assert!(is_newer("0.9.9", "0.10.0"));
        assert!(is_newer("v0.2.0", "v0.3.0"), "a leading v is allowed");
        assert!(!is_newer("0.2.0", "0.2.0"));
        assert!(
            !is_newer("0.2.0", "0.2.0-alpha.9"),
            "never back to a pre-release"
        );
        assert!(!is_newer("0.3.0", "0.2.9"));
        assert!(
            !is_newer("0.2.0+build.1", "0.2.0+build.2"),
            "build metadata is ignored"
        );
        assert!(!is_newer("0.2.0", "latest"));
        assert!(!is_newer("garbage", "0.3.0"));
    }

    #[test]
    fn the_manifest_is_read_with_every_asset() {
        let m = Manifest::parse(NEWER).unwrap();
        assert_eq!(m.version, "999.0.0");
        assert_eq!(m.tag, "v999.0.0");
        assert_eq!(m.notes_url, "https://dentalcraft.app/en/download/");
        for key in AssetKey::ALL {
            let a = m.asset(key).unwrap_or_else(|| panic!("{}", key.name()));
            assert!(a.url.starts_with("https://dentalcraft.app/download/"));
            assert!(a.size > 0);
        }
        assert_eq!(
            m.asset(AssetKey::AppImage).unwrap().file_name(),
            "DentalCraft-999.0.0-x86_64.AppImage"
        );
    }

    #[test]
    fn unknown_fields_and_missing_assets_are_fine() {
        let m = Manifest::parse(br#"{"version": "1.2.3", "channel": "stable"}"#).unwrap();
        assert!(m.assets.is_empty() && m.notes_url.is_empty());
        assert_eq!(m.asset(AssetKey::WindowsMsi), None);
    }

    #[test]
    fn a_bad_manifest_is_refused() {
        let asset = |url: &str, sha: &str, size: u64| {
            format!(
                r#"{{"version": "1.0.0", "assets": {{"appimage": {{"url": "{url}", "sha256": "{sha}", "size": {size}}}}}}}"#
            )
        };
        let sha = "ab".repeat(32);
        let ok = "https://dentalcraft.app/download/DentalCraft-1.0.0-x86_64.AppImage";
        assert!(Manifest::parse(asset(ok, &sha, 10).as_bytes()).is_ok());
        assert!(
            Manifest::parse(asset("http://127.0.0.1:9/a.AppImage", &sha, 10).as_bytes()).is_ok()
        );
        for bad in [
            asset("http://dentalcraft.app/download/a.AppImage", &sha, 10),
            asset("ftp://dentalcraft.app/a", &sha, 10),
            asset("https://dentalcraft.app/download/", &sha, 10),
            asset(ok, "abc", 10),
            asset(ok, &"zz".repeat(32), 10),
            asset(ok, &sha, 0),
            r#"{"version": "one"}"#.to_owned(),
            r#"{"tag": "v1.0.0"}"#.to_owned(),
            "not json".to_owned(),
        ] {
            assert!(
                matches!(Manifest::parse(bad.as_bytes()), Err(UpdateError::Server(_))),
                "{bad}"
            );
        }
    }

    #[test]
    fn a_signed_manifest_is_accepted() {
        let m = Manifest::verified(NEWER, NEWER_SIG, test_key()).unwrap();
        assert_eq!(m.version, "999.0.0");
        let m = Manifest::verified(OLDER, OLDER_SIG, test_key()).unwrap();
        assert_eq!(m.version, "0.0.1");
    }

    #[test]
    fn a_wrong_signature_is_refused() {
        // one byte changed
        let mut tampered = NEWER.to_vec();
        let at = tampered.iter().position(|&b| b == b'9').unwrap();
        tampered[at] = b'8';
        assert_eq!(
            Manifest::verified(&tampered, NEWER_SIG, test_key()),
            Err(UpdateError::NotGenuine)
        );
        // the signature of another file
        assert_eq!(
            Manifest::verified(NEWER, OLDER_SIG, test_key()),
            Err(UpdateError::NotGenuine)
        );
        // a legacy (not prehashed) signature
        assert_eq!(
            Manifest::verified(NEWER, NEWER_LEGACY_SIG, test_key()),
            Err(UpdateError::NotGenuine)
        );
        // the release key did not sign the test files
        assert_eq!(
            Manifest::verified(NEWER, NEWER_SIG, crate::PUBLIC_KEY),
            Err(UpdateError::NotGenuine)
        );
        // no signature, or garbage
        for sig in ["", "untrusted comment: x\nnot base64\n"] {
            assert_eq!(
                Manifest::verified(NEWER, sig, test_key()),
                Err(UpdateError::NotGenuine)
            );
        }
        // a changed trusted comment breaks the global signature
        let edited = NEWER_SIG.replace("dentalcraft 999.0.0", "dentalcraft 999.0.1");
        assert_eq!(
            Manifest::verified(NEWER, &edited, test_key()),
            Err(UpdateError::NotGenuine)
        );
    }

    #[test]
    fn the_release_key_is_well_formed() {
        assert!(PublicKey::from_base64(crate::PUBLIC_KEY).is_ok());
    }

    /// The signature the release job makes verifies with the key built into
    /// the program. Point `DC_UPDATE_SAMPLE` at a folder with `update.json`
    /// and `update.json.minisig` signed with the release key (for example the
    /// files of a release, or `/update/` of the website), then:
    /// `cargo test -p dc-update -- --ignored release_signature`.
    #[test]
    #[ignore = "needs DC_UPDATE_SAMPLE: a manifest signed with the release key"]
    fn release_signature_verifies_with_the_built_in_key() {
        let dir = std::path::PathBuf::from(std::env::var("DC_UPDATE_SAMPLE").unwrap());
        let json = std::fs::read(dir.join("update.json")).unwrap();
        let sig = std::fs::read_to_string(dir.join("update.json.minisig")).unwrap();
        let m = Manifest::verified(&json, &sig, crate::PUBLIC_KEY).unwrap();
        assert!(parse_version(&m.version).is_ok());
    }
}
