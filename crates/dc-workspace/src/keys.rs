//! Parsing of module shortcut strings (egui notation, e.g. `S`,
//! `Ctrl+Shift+S`).

use egui::{Key, KeyboardShortcut, Modifiers};

/// Parses `Ctrl+Shift+S`-style text; `Ctrl` and `Cmd` both mean the
/// platform command key. `None` for unknown keys.
pub fn parse(text: &str) -> Option<KeyboardShortcut> {
    let mut mods = Modifiers::NONE;
    let mut key = None;
    for part in text.split('+').map(str::trim) {
        match part.to_ascii_lowercase().as_str() {
            "ctrl" | "cmd" | "command" => mods |= Modifiers::COMMAND,
            "shift" => mods |= Modifiers::SHIFT,
            "alt" | "option" => mods |= Modifiers::ALT,
            "" => return None,
            _ => key = Some(Key::from_name(part)?),
        }
    }
    key.map(|k| KeyboardShortcut::new(mods, k))
}

/// `true` when the shortcut has no modifier (a single key such as `S`):
/// such shortcuts must not fire while a text field has the keyboard.
pub fn is_plain(s: &KeyboardShortcut) -> bool {
    s.modifiers.is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_shortcuts() {
        assert_eq!(
            parse("S"),
            Some(KeyboardShortcut::new(Modifiers::NONE, Key::S))
        );
        assert_eq!(
            parse("Ctrl+Shift+S"),
            Some(KeyboardShortcut::new(
                Modifiers::COMMAND | Modifiers::SHIFT,
                Key::S
            ))
        );
        assert_eq!(parse("F5").map(|s| s.logical_key), Some(Key::F5));
        assert_eq!(parse("Ctrl+"), None);
        assert_eq!(parse("Nope"), None);
        assert!(is_plain(&parse("A").unwrap()));
    }
}
