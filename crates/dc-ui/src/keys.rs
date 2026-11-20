//! Names of keys in texts shown to the user.
//!
//! Shortcuts are written once, with the Windows and Linux names (`Ctrl Z`,
//! `Alt+click`). egui maps `Modifiers::COMMAND` to the Command key on macOS,
//! so there the same text must say `Cmd Z` and `Option+click`.

use std::borrow::Cow;

/// `text` with the key names of this platform: on macOS `Ctrl` becomes `Cmd`
/// and `Alt` becomes `Option`; elsewhere `text` itself.
pub fn platform_keys(text: &str) -> Cow<'_, str> {
    if cfg!(target_os = "macos") {
        macos_keys(text)
    } else {
        Cow::Borrowed(text)
    }
}

/// The macOS spelling of `text` (whole words only: `Control` stays).
pub fn macos_keys(text: &str) -> Cow<'_, str> {
    if !text.contains("Ctrl") && !text.contains("Alt") {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len() + 8);
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut String| {
        out.push_str(match word.as_str() {
            "Ctrl" => "Cmd",
            "Alt" => "Option",
            w => w,
        });
        word.clear();
    };
    for c in text.chars() {
        if c.is_alphanumeric() {
            word.push(c);
        } else {
            flush(&mut word, &mut out);
            out.push(c);
        }
    }
    flush(&mut word, &mut out);
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macos_says_cmd_and_option() {
        assert_eq!(macos_keys("Ctrl Shift Z"), "Cmd Shift Z");
        assert_eq!(macos_keys("Press Ctrl+Z to undo."), "Press Cmd+Z to undo.");
        assert_eq!(
            macos_keys("Shift: gingiva. Ctrl: fill. Alt+click on a tooth"),
            "Shift: gingiva. Cmd: fill. Option+click on a tooth"
        );
        assert_eq!(
            macos_keys("Control Altitude Ctrlx"),
            "Control Altitude Ctrlx"
        );
        assert!(matches!(macos_keys("Esc"), Cow::Borrowed(_)));
        // Italian text keeps its accents
        assert_eq!(
            macos_keys("Premi Ctrl+Z, è già così"),
            "Premi Cmd+Z, è già così"
        );
    }

    #[test]
    fn other_systems_keep_the_text() {
        if !cfg!(target_os = "macos") {
            assert_eq!(platform_keys("Ctrl K"), "Ctrl K");
        }
    }
}
