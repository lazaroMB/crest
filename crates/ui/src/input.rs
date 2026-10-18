//! Modal prompt input.
//!
//! This first part converts GPUI keystrokes into the keymap's own [`Key`]
//! vocabulary, so the [`application::keymap::KeySequencer`] can resolve
//! them.

use application::keymap::{Key, KeyCode, Modifiers};

/// Converts a GPUI keystroke to a keymap [`Key`]. Returns `None` for a bare
/// modifier press.
///
/// Printable keys use the character the platform produced (so `shift-g` is
/// `G` and `shift-;` is `:`) unless Ctrl, Alt or Cmd is held.
pub fn key_from_keystroke(keystroke: &gpui::Keystroke) -> Option<Key> {
    let m = &keystroke.modifiers;
    let named = match keystroke.key.as_str() {
        "shift" | "control" | "alt" | "platform" | "function" | "" => return None,
        "enter" => Some(KeyCode::Enter),
        "escape" => Some(KeyCode::Esc),
        "tab" => Some(KeyCode::Tab),
        "backspace" => Some(KeyCode::Backspace),
        "delete" => Some(KeyCode::Delete),
        "up" => Some(KeyCode::Up),
        "down" => Some(KeyCode::Down),
        "left" => Some(KeyCode::Left),
        "right" => Some(KeyCode::Right),
        "home" => Some(KeyCode::Home),
        "end" => Some(KeyCode::End),
        "pageup" => Some(KeyCode::PageUp),
        "pagedown" => Some(KeyCode::PageDown),
        "space" => Some(KeyCode::Char(' ')),
        key => key
            .strip_prefix('f')
            .and_then(|n| n.parse::<u8>().ok())
            .filter(|n| (1..=24).contains(n))
            .map(KeyCode::F),
    };
    let chord = m.control || m.alt || m.platform;
    let mut mods = Modifiers {
        ctrl: m.control,
        alt: m.alt,
        shift: m.shift,
        cmd: m.platform,
    };
    let code = match named {
        Some(code) => code,
        None => {
            let typed = keystroke.key_char.as_deref().filter(|_| !chord);
            let c = match typed.and_then(single_char) {
                Some(c) => {
                    // The character already carries the shift.
                    mods.shift = false;
                    c
                }
                None => single_char(&keystroke.key)?,
            };
            KeyCode::Char(c)
        }
    };
    Some(Key::new(code, mods))
}

fn single_char(s: &str) -> Option<char> {
    let mut chars = s.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => Some(c),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::Keystroke;

    fn key(notation: &str) -> Option<Key> {
        let keystroke = Keystroke::parse(notation)
            .unwrap_or_else(|err| panic!("{err}"))
            .with_simulated_ime();
        key_from_keystroke(&keystroke)
    }

    fn parsed(notation: &str) -> Option<Key> {
        application::keymap::parse_keys(notation, " ")
            .ok()
            .and_then(|keys| keys.first().copied())
    }

    #[test]
    fn printable_keys_use_the_typed_character() {
        assert_eq!(key("g"), parsed("g"));
        assert_eq!(key("shift-g"), parsed("G"));
        assert_eq!(key(":"), parsed(":"));
        assert_eq!(key("space"), parsed("<Space>"));
    }

    #[test]
    fn named_and_chorded_keys_keep_modifiers() {
        assert_eq!(key("enter"), parsed("<CR>"));
        assert_eq!(key("shift-enter"), parsed("<S-CR>"));
        assert_eq!(key("escape"), parsed("<Esc>"));
        assert_eq!(key("ctrl-c"), parsed("<C-c>"));
        assert_eq!(key("f5"), parsed("<F5>"));
    }

    #[test]
    fn bare_modifiers_are_not_keys() {
        assert_eq!(key("shift"), None);
    }
}
