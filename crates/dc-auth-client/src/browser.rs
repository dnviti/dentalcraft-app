//! Opening the system browser.

/// Opens a URL in the user's browser. Replaceable so that tests (and
/// headless environments) do not launch one.
pub trait BrowserOpener: Send + Sync {
    /// Opens `url`; returns false if no browser could be launched. The sign-in
    /// continues either way: the UI shows the URL for the user to open by hand.
    fn open(&self, url: &str) -> bool;
}

/// The default browser, through the `webbrowser` crate.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemBrowser;

impl BrowserOpener for SystemBrowser {
    fn open(&self, url: &str) -> bool {
        webbrowser::open(url).is_ok()
    }
}

impl<F: Fn(&str) -> bool + Send + Sync> BrowserOpener for F {
    fn open(&self, url: &str) -> bool {
        self(url)
    }
}
