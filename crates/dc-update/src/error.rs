//! What can go wrong, with the words the app shows for it.

use std::fmt;
use std::path::PathBuf;

/// An update that could not be checked or installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateError {
    /// The website could not be reached (no connection, name lookup, TLS,
    /// timeout); the detail is for the log.
    Offline(String),
    /// The website answered, but not with usable update information (an
    /// HTTP error, a file that cannot be read); the detail is for the log.
    Server(String),
    /// The update information is not signed with the DentalCraft release
    /// key: it is ignored and nothing is installed.
    NotGenuine,
    /// The downloaded file is not the one announced (other size or
    /// content): it is deleted and nothing is installed.
    Damaged,
    /// The user stopped the download.
    Cancelled,
    /// DentalCraft cannot write where the new version must go.
    NotWritable(PathBuf),
    /// No download fits this installation: the new version comes from the
    /// website.
    NoDownload,
    /// Unpacking, replacing the files or starting the installer failed; the
    /// installed version is unchanged. The detail is for the log.
    Install(String),
}

impl UpdateError {
    /// Every kind of error with an example value, for the key and wording
    /// tests.
    pub fn examples() -> Vec<UpdateError> {
        vec![
            Self::Offline(String::new()),
            Self::Server(String::new()),
            Self::NotGenuine,
            Self::Damaged,
            Self::Cancelled,
            Self::NotWritable(PathBuf::from("/opt/dentalcraft")),
            Self::NoDownload,
            Self::Install(String::new()),
        ]
    }

    /// i18n key of the message shown to the user (bundle `dc-update`).
    pub fn i18n_key(&self) -> &'static str {
        match self {
            Self::Offline(_) => "update-error-offline",
            Self::Server(_) => "update-error-server",
            Self::NotGenuine => "update-error-not-genuine",
            Self::Damaged => "update-error-damaged",
            Self::Cancelled => "update-error-cancelled",
            Self::NotWritable(_) => "update-error-not-writable",
            Self::NoDownload => "update-error-no-download",
            Self::Install(_) => "update-error-install",
        }
    }

    /// Stable code for the diagnostics reports (`update.offline`).
    pub fn code(&self) -> &'static str {
        match self {
            Self::Offline(_) => "update.offline",
            Self::Server(_) => "update.server",
            Self::NotGenuine => "update.not-genuine",
            Self::Damaged => "update.damaged",
            Self::Cancelled => "update.cancelled",
            Self::NotWritable(_) => "update.not-writable",
            Self::NoDownload => "update.no-download",
            Self::Install(_) => "update.install",
        }
    }

    /// The message for the user, in the UI language.
    pub fn message(&self) -> String {
        crate::loader();
        match self {
            Self::NotWritable(path) => dc_i18n::tr_args(
                self.i18n_key(),
                &[("path", path.display().to_string().as_str())],
            ),
            _ => dc_i18n::tr(self.i18n_key()),
        }
    }

    /// Technical detail worth showing under the message (an operating
    /// system error), if any.
    pub fn detail(&self) -> Option<&str> {
        match self {
            Self::Install(d) if !d.is_empty() => Some(d),
            _ => None,
        }
    }

    /// `true` when trying again may work (network trouble, a damaged
    /// download).
    pub fn retryable(&self) -> bool {
        matches!(
            self,
            Self::Offline(_) | Self::Server(_) | Self::Damaged | Self::Cancelled
        )
    }
}

impl fmt::Display for UpdateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Offline(d) => write!(f, "cannot reach the DentalCraft website: {d}"),
            Self::Server(d) => write!(f, "unusable update information: {d}"),
            Self::NotGenuine => {
                f.write_str("the update information is not signed with the DentalCraft release key")
            }
            Self::Damaged => {
                f.write_str("the download does not match the size and SHA-256 announced: deleted")
            }
            Self::Cancelled => f.write_str("the download was cancelled"),
            Self::NotWritable(p) => write!(f, "cannot write to {}", p.display()),
            Self::NoDownload => f.write_str("no download for this installation"),
            Self::Install(d) => write!(f, "cannot install the new version: {d}"),
        }
    }
}

impl std::error::Error for UpdateError {}
