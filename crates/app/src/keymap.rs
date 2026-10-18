//! Neovim-style key sequences: notation parser, per-mode keymaps and a
//! sequencer that resolves pending prefixes with `timeoutlen`.
//!
//! This module is pure logic: no async runtime and no UI toolkit. Time is
//! passed in as an [`Instant`](std::time::Instant) so tests never sleep.
//!
//! The types here (`Mode`, [`Key`], [`KeySequence`], `Action`) are the
//! keymap's own vocabulary. The Lua host (`app.keymap.set`) and the UI layer
//! convert their own representations into them.

use std::fmt;

/// A key without modifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyCode {
    /// A printable character. Space is `Char(' ')`; shifted letters are
    /// stored as their uppercase form without the shift modifier.
    Char(char),
    Enter,
    Esc,
    Tab,
    Backspace,
    Delete,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    /// Function key `F1`..`F24`.
    F(u8),
}

/// Modifier keys held together with a [`KeyCode`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub cmd: bool,
}

/// One key press.
///
/// Keys are normalized so that a parsed mapping and an incoming event compare
/// equal: shift on a letter becomes the uppercase letter (`<S-a>` == `A`) and
/// a letter with ctrl, alt or cmd is lowercased (`<C-C>` == `<C-c>`). Hosts
/// should build incoming keys with [`Key::new`], which applies the same rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Key {
    pub code: KeyCode,
    pub mods: Modifiers,
}

impl Key {
    /// Builds a normalized key.
    pub fn new(code: KeyCode, mods: Modifiers) -> Self {
        let mut key = Self { code, mods };
        if let KeyCode::Char(c) = key.code {
            if key.mods.shift && c.is_alphabetic() {
                key.code = KeyCode::Char(c.to_uppercase().next().unwrap_or(c));
                key.mods.shift = false;
            }
            if (key.mods.ctrl || key.mods.alt || key.mods.cmd) && c.is_alphabetic() {
                key.code = KeyCode::Char(c.to_lowercase().next().unwrap_or(c));
            }
        }
        key
    }

    /// A plain key with no modifiers.
    pub fn plain(code: KeyCode) -> Self {
        Self::new(code, Modifiers::default())
    }

    /// A plain character key.
    pub fn char(c: char) -> Self {
        Self::plain(KeyCode::Char(c))
    }
}

/// A sequence of keys, e.g. `gg` or `<leader>ff`.
pub type KeySequence = Vec<Key>;

/// Errors produced while parsing key notation.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KeyParseError {
    #[error("empty key sequence")]
    Empty,
    #[error("unclosed `<` in key notation")]
    Unclosed,
    #[error("empty `<>` in key notation")]
    EmptyBracket,
    #[error("unknown key name `{0}`")]
    UnknownKey(String),
    #[error("unknown modifier `{0}`")]
    UnknownModifier(String),
    #[error("`<leader>` cannot be used inside the leader itself")]
    LeaderInLeader,
}

/// Parses Neovim key notation (`"gg"`, `"<leader>ff"`, `"<C-c>"`,
/// `"<S-Enter>"`, `"<lt>"`...). `<leader>` expands to `leader`, which is
/// itself parsed as notation (so both `" "` and `"<Space>"` work).
pub fn parse_keys(notation: &str, leader: &str) -> Result<KeySequence, KeyParseError> {
    let leader_keys = parse_inner(leader, None)?;
    parse_inner(notation, Some(&leader_keys))
}

fn parse_inner(notation: &str, leader: Option<&[Key]>) -> Result<KeySequence, KeyParseError> {
    let mut keys = Vec::new();
    let mut rest = notation;
    while let Some(c) = rest.chars().next() {
        if c != '<' {
            keys.push(Key::char(c));
            rest = &rest[c.len_utf8()..];
            continue;
        }
        let close = rest.find('>').ok_or(KeyParseError::Unclosed)?;
        let inner = &rest[1..close];
        rest = &rest[close + 1..];
        if inner.is_empty() {
            return Err(KeyParseError::EmptyBracket);
        }
        if inner.eq_ignore_ascii_case("leader") {
            let leader = leader.ok_or(KeyParseError::LeaderInLeader)?;
            keys.extend_from_slice(leader);
            continue;
        }
        keys.push(parse_bracket(inner)?);
    }
    if keys.is_empty() {
        return Err(KeyParseError::Empty);
    }
    Ok(keys)
}

/// Parses the inside of `<...>`: optional `X-` modifiers then a key name.
fn parse_bracket(inner: &str) -> Result<Key, KeyParseError> {
    let mut mods = Modifiers::default();
    let mut name = inner;
    // A modifier is a single ASCII letter followed by `-`, and something must
    // follow it (so `<C-->` is ctrl + `-`).
    while name.len() > 2 && name.as_bytes()[1] == b'-' && name.as_bytes()[0].is_ascii() {
        let m = &name[..1];
        match m.to_ascii_uppercase().as_str() {
            "C" => mods.ctrl = true,
            "A" | "M" => mods.alt = true,
            "S" => mods.shift = true,
            "D" => mods.cmd = true,
            _ => return Err(KeyParseError::UnknownModifier(m.to_owned())),
        }
        name = &name[2..];
    }
    let code = key_name(name).ok_or_else(|| KeyParseError::UnknownKey(name.to_owned()))?;
    if mods == Modifiers::default() && matches!(code, KeyCode::Char(_)) && name.chars().count() == 1
    {
        // `<x>` without modifiers is not valid Neovim notation.
        return Err(KeyParseError::UnknownKey(name.to_owned()));
    }
    Ok(Key::new(code, mods))
}

fn key_name(name: &str) -> Option<KeyCode> {
    let mut chars = name.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        return Some(KeyCode::Char(c));
    }
    let code = match name.to_ascii_lowercase().as_str() {
        "cr" | "enter" | "return" => KeyCode::Enter,
        "esc" => KeyCode::Esc,
        "tab" => KeyCode::Tab,
        "bs" | "backspace" => KeyCode::Backspace,
        "del" | "delete" => KeyCode::Delete,
        "space" => KeyCode::Char(' '),
        "lt" => KeyCode::Char('<'),
        "gt" => KeyCode::Char('>'),
        "bar" => KeyCode::Char('|'),
        "bslash" => KeyCode::Char('\\'),
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pageup" => KeyCode::PageUp,
        "pagedown" => KeyCode::PageDown,
        other => {
            let n: u8 = other.strip_prefix('f')?.parse().ok()?;
            if !(1..=24).contains(&n) {
                return None;
            }
            KeyCode::F(n)
        }
    };
    Some(code)
}

impl fmt::Display for Key {
    /// Writes the key back in Neovim notation.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self.code {
            KeyCode::Char(' ') => "Space".to_owned(),
            KeyCode::Char('<') => "lt".to_owned(),
            KeyCode::Char(c) => c.to_string(),
            KeyCode::Enter => "CR".to_owned(),
            KeyCode::Esc => "Esc".to_owned(),
            KeyCode::Tab => "Tab".to_owned(),
            KeyCode::Backspace => "BS".to_owned(),
            KeyCode::Delete => "Del".to_owned(),
            KeyCode::Up => "Up".to_owned(),
            KeyCode::Down => "Down".to_owned(),
            KeyCode::Left => "Left".to_owned(),
            KeyCode::Right => "Right".to_owned(),
            KeyCode::Home => "Home".to_owned(),
            KeyCode::End => "End".to_owned(),
            KeyCode::PageUp => "PageUp".to_owned(),
            KeyCode::PageDown => "PageDown".to_owned(),
            KeyCode::F(n) => format!("F{n}"),
        };
        let m = self.mods;
        let bare = matches!(self.code, KeyCode::Char(c) if c != ' ' && c != '<');
        if m == Modifiers::default() && bare {
            return f.write_str(&name);
        }
        f.write_str("<")?;
        for (on, p) in [
            (m.ctrl, "C-"),
            (m.alt, "A-"),
            (m.shift, "S-"),
            (m.cmd, "D-"),
        ] {
            if on {
                f.write_str(p)?;
            }
        }
        write!(f, "{name}>")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctrl(c: char) -> Key {
        Key::new(
            KeyCode::Char(c),
            Modifiers {
                ctrl: true,
                ..Modifiers::default()
            },
        )
    }

    #[test]
    fn parses_plain_chars() {
        assert_eq!(
            parse_keys("gg", " "),
            Ok(vec![Key::char('g'), Key::char('g')])
        );
        assert_eq!(parse_keys("G", " "), Ok(vec![Key::char('G')]));
    }

    #[test]
    fn expands_leader() {
        let ff = vec![Key::char(' '), Key::char('f'), Key::char('f')];
        assert_eq!(parse_keys("<leader>ff", " "), Ok(ff.clone()));
        assert_eq!(parse_keys("<Leader>ff", "<Space>"), Ok(ff));
        assert_eq!(
            parse_keys("<leader>x", ","),
            Ok(vec![Key::char(','), Key::char('x')])
        );
    }

    #[test]
    fn parses_named_and_modified_keys() {
        let shift = Modifiers {
            shift: true,
            ..Modifiers::default()
        };
        assert_eq!(parse_keys("<C-c>", " "), Ok(vec![ctrl('c')]));
        assert_eq!(parse_keys("<C-C>", " "), Ok(vec![ctrl('c')]));
        assert_eq!(
            parse_keys("<S-Enter>", " "),
            Ok(vec![Key::new(KeyCode::Enter, shift)])
        );
        assert_eq!(parse_keys("<S-a>", " "), Ok(vec![Key::char('A')]));
        assert_eq!(parse_keys("<Esc>", " "), Ok(vec![Key::plain(KeyCode::Esc)]));
        assert_eq!(
            parse_keys("<CR>", " "),
            Ok(vec![Key::plain(KeyCode::Enter)])
        );
        assert_eq!(parse_keys("<Space>", " "), Ok(vec![Key::char(' ')]));
        assert_eq!(parse_keys("<lt>", " "), Ok(vec![Key::char('<')]));
        assert_eq!(
            parse_keys("<F12>", " "),
            Ok(vec![Key::plain(KeyCode::F(12))])
        );
        assert_eq!(parse_keys("<C-->", " "), Ok(vec![ctrl('-')]));
    }

    #[test]
    fn rejects_invalid_notation() {
        assert_eq!(parse_keys("", " "), Err(KeyParseError::Empty));
        assert_eq!(parse_keys("<C-c", " "), Err(KeyParseError::Unclosed));
        assert_eq!(parse_keys("<>", " "), Err(KeyParseError::EmptyBracket));
        assert_eq!(
            parse_keys("<Foo>", " "),
            Err(KeyParseError::UnknownKey("Foo".into()))
        );
        assert_eq!(
            parse_keys("<F25>", " "),
            Err(KeyParseError::UnknownKey("F25".into()))
        );
        assert_eq!(
            parse_keys("<a>", " "),
            Err(KeyParseError::UnknownKey("a".into()))
        );
        assert_eq!(
            parse_keys("<X-a>", " "),
            Err(KeyParseError::UnknownModifier("X".into()))
        );
        assert_eq!(
            parse_keys("x", "<leader>"),
            Err(KeyParseError::LeaderInLeader)
        );
        assert_eq!(parse_keys("x", ""), Err(KeyParseError::Empty));
    }

    #[test]
    fn display_round_trips() {
        for n in [
            "gg",
            "<Space>ff",
            "<C-c>",
            "<S-CR>",
            "<Esc>",
            "<lt>",
            "<F5>",
        ] {
            let keys = parse_keys(n, " ").unwrap_or_default();
            let shown: String = keys.iter().map(ToString::to_string).collect();
            assert_eq!(parse_keys(&shown, " "), Ok(keys), "{n}");
        }
    }
}
