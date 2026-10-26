//! Neovim-style key sequences: notation parser, a keymap and a sequencer
//! that resolves pending prefixes (`<leader>ff`) with `timeoutlen`.
//!
//! There are no modes: the prompt is always ready to type, so mappings use
//! chords (`<C-c>`, `<C-Tab>`) or the leader chord (`<C-g>` by default)
//! and never plain printable keys.
//!
//! This module is pure logic: no async runtime and no UI toolkit. Time is
//! passed in as an [`Instant`] so tests never sleep.
//!
//! The types here ([`Key`], [`KeySequence`], [`Action`]) are the
//! keymap's own vocabulary. The Lua host (`app.keymap.set`) and the UI layer
//! convert their own representations into them.

use std::collections::HashMap;
use std::fmt;
use std::time::{Duration, Instant};

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
    Insert,
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
/// equal: shift on a plain letter becomes the uppercase letter (`<S-a>` ==
/// `A`), and a letter with ctrl, alt or cmd is lowercased with the shift kept
/// as a modifier (`<C-C>` == `<C-c>`, but `<C-S-c>` is its own chord, as
/// terminals use it to copy). Hosts should build incoming keys with
/// [`Key::new`], which applies the same rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Key {
    pub code: KeyCode,
    pub mods: Modifiers,
}

impl Key {
    /// Builds a normalized key.
    pub fn new(code: KeyCode, mods: Modifiers) -> Self {
        let mut key = Self { code, mods };
        if let KeyCode::Char(c) = key.code
            && c.is_alphabetic()
        {
            if key.mods.ctrl || key.mods.alt || key.mods.cmd {
                key.code = KeyCode::Char(c.to_lowercase().next().unwrap_or(c));
            } else if key.mods.shift {
                key.code = KeyCode::Char(c.to_uppercase().next().unwrap_or(c));
                key.mods.shift = false;
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
        "ins" | "insert" => KeyCode::Insert,
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
            KeyCode::Insert => "Insert".to_owned(),
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

/// Leader used when none is configured (`app.opt.leader`).
/// Not `<C-Space>`: macOS takes it to switch the input source. Alt is not
/// used either: on macOS, Option composes characters.
pub const DEFAULT_LEADER: &str = "<C-g>";

#[derive(Debug, Clone)]
struct Node<A> {
    action: Option<A>,
    children: HashMap<Key, Node<A>>,
}

impl<A> Default for Node<A> {
    fn default() -> Self {
        Self {
            action: None,
            children: HashMap::new(),
        }
    }
}

/// Result of looking up a key sequence in a [`Keymap`].
#[derive(Debug, PartialEq, Eq)]
pub struct Lookup<'a, A> {
    /// Action mapped to exactly this sequence.
    pub exact: Option<&'a A>,
    /// Whether some longer mapping starts with this sequence.
    pub is_prefix: bool,
}

/// Trie of `KeySequence -> A`.
///
/// `<leader>` is expanded when a mapping is registered, as in Neovim: changing
/// the leader later does not move existing mappings.
#[derive(Debug, Clone)]
pub struct Keymap<A> {
    leader: String,
    root: Node<A>,
}

impl<A> Default for Keymap<A> {
    fn default() -> Self {
        Self::new()
    }
}

impl<A> Keymap<A> {
    /// An empty keymap with the default leader.
    pub fn new() -> Self {
        Self {
            leader: DEFAULT_LEADER.to_owned(),
            root: Node::default(),
        }
    }

    /// The leader that `<leader>` expands to in future registrations.
    pub fn leader(&self) -> &str {
        &self.leader
    }

    /// Sets the leader (notation, e.g. `"<C-g>"` or `"<C-,>"`).
    /// Validated eagerly.
    pub fn set_leader(&mut self, leader: &str) -> Result<(), KeyParseError> {
        parse_inner(leader, None)?;
        self.leader = leader.to_owned();
        Ok(())
    }

    /// Maps `notation`, replacing any previous action for it.
    pub fn set(&mut self, notation: &str, action: A) -> Result<(), KeyParseError> {
        let keys = parse_keys(notation, &self.leader)?;
        self.set_keys(&keys, action);
        Ok(())
    }

    /// Maps an already parsed sequence, replacing any previous action.
    pub fn set_keys(&mut self, keys: &[Key], action: A) {
        let mut node = &mut self.root;
        for key in keys {
            node = node.children.entry(*key).or_default();
        }
        node.action = Some(action);
    }

    /// Removes the mapping for `notation`, returning its action.
    pub fn del(&mut self, notation: &str) -> Result<Option<A>, KeyParseError> {
        let keys = parse_keys(notation, &self.leader)?;
        Ok(remove(&mut self.root, &keys))
    }

    /// Looks up `keys`.
    pub fn lookup(&self, keys: &[Key]) -> Lookup<'_, A> {
        let node = keys
            .iter()
            .try_fold(&self.root, |node, key| node.children.get(key));
        Lookup {
            exact: node.and_then(|n| n.action.as_ref()),
            is_prefix: node.is_some_and(|n| !n.children.is_empty()),
        }
    }
}

/// One mapping of a [`Keymap`], as listed by [`Keymap::entries`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeymapEntry<A> {
    /// Keys, with the leader already expanded.
    pub keys: KeySequence,
    /// What it runs.
    pub action: A,
}

impl<A> KeymapEntry<A> {
    /// The keys in Neovim notation (`<C-g>ff`).
    pub fn notation(&self) -> String {
        self.keys.iter().map(ToString::to_string).collect()
    }
}

impl<A: Clone> Keymap<A> {
    /// Every mapping, by keys in notation order.
    pub fn entries(&self) -> Vec<KeymapEntry<A>> {
        let mut out = Vec::new();
        let mut keys = Vec::new();
        collect(&self.root, &mut keys, &mut out);
        out.sort_by_cached_key(KeymapEntry::notation);
        out
    }
}

fn collect<A: Clone>(node: &Node<A>, keys: &mut KeySequence, out: &mut Vec<KeymapEntry<A>>) {
    if let Some(action) = &node.action {
        out.push(KeymapEntry {
            keys: keys.clone(),
            action: action.clone(),
        });
    }
    for (key, child) in &node.children {
        keys.push(*key);
        collect(child, keys, out);
        keys.pop();
    }
}

/// Removes the action at `keys`, pruning nodes left empty.
fn remove<A>(node: &mut Node<A>, keys: &[Key]) -> Option<A> {
    let Some((first, rest)) = keys.split_first() else {
        return node.action.take();
    };
    let child = node.children.get_mut(first)?;
    let removed = remove(child, rest);
    if child.action.is_none() && child.children.is_empty() {
        node.children.remove(first);
    }
    removed
}

/// Outcome of feeding keys to a [`KeySequencer`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Feed<A> {
    /// A mapping fired.
    Matched(A),
    /// The keys so far are a prefix of a longer mapping; waiting for more.
    Pending,
    /// Keys that matched nothing. The host types them into the prompt (or
    /// lets the focused panel have them).
    NoMatch(Vec<Key>),
}

/// Default `timeoutlen`: how long a prefix waits for the next key.
pub const DEFAULT_TIMEOUTLEN: Duration = Duration::from_millis(1000);

/// Stateful resolver of key presses against a [`Keymap`].
///
/// [`feed`](Self::feed) returns a list because one key can settle several
/// things at once: with `<leader>f` mapped, typing the leader, `f` and `x`
/// fires `<leader>f` and then reports `x` as unmatched.
///
/// When the pending keys sit unresolved for `timeoutlen`, [`tick`](Self::tick)
/// settles them: an exact mapping that is also a prefix fires, otherwise the
/// keys come back as [`Feed::NoMatch`]. The host schedules a call to `tick`
/// at [`deadline`](Self::deadline).
#[derive(Debug, Clone)]
pub struct KeySequencer<A> {
    keymap: Keymap<A>,
    pending: Vec<Key>,
    last_key_at: Option<Instant>,
    timeoutlen: Duration,
}

impl<A: Clone> KeySequencer<A> {
    /// A sequencer over `keymap`.
    pub fn new(keymap: Keymap<A>) -> Self {
        Self {
            keymap,
            pending: Vec::new(),
            last_key_at: None,
            timeoutlen: DEFAULT_TIMEOUTLEN,
        }
    }

    /// How long a prefix waits for the next key.
    pub fn timeoutlen(&self) -> Duration {
        self.timeoutlen
    }

    /// Sets `timeoutlen`.
    pub fn set_timeoutlen(&mut self, timeoutlen: Duration) {
        self.timeoutlen = timeoutlen;
    }

    /// When the pending keys time out, if any are pending.
    pub fn deadline(&self) -> Option<Instant> {
        match (self.pending.is_empty(), self.last_key_at) {
            (false, Some(at)) => Some(at + self.timeoutlen),
            _ => None,
        }
    }

    /// The keymap in use.
    pub fn keymap(&self) -> &Keymap<A> {
        &self.keymap
    }

    /// Mutable access to the keymap. Drops any pending keys, since they may
    /// no longer be a valid prefix.
    pub fn keymap_mut(&mut self) -> &mut Keymap<A> {
        self.reset();
        &mut self.keymap
    }

    /// Keys typed so far that form an unresolved prefix.
    pub fn pending(&self) -> &[Key] {
        &self.pending
    }

    /// Drops pending keys without resolving them.
    pub fn reset(&mut self) {
        self.pending.clear();
        self.last_key_at = None;
    }

    /// Feeds one key press received at `now`. Pending keys that already
    /// timed out are settled first, as if [`tick`](Self::tick) had run.
    pub fn feed(&mut self, key: Key, now: Instant) -> Vec<Feed<A>> {
        let mut out = self.tick(now);
        self.pending.push(key);
        self.last_key_at = Some(now);
        out.extend(self.resolve(false));
        out
    }

    /// Settles pending keys if `timeoutlen` has elapsed since the last key.
    /// Returns nothing when there is nothing to settle yet.
    pub fn tick(&mut self, now: Instant) -> Vec<Feed<A>> {
        match self.deadline() {
            Some(deadline) if now >= deadline => self.resolve(true),
            _ => Vec::new(),
        }
    }

    /// Consumes `pending` as far as possible. Prefers waiting while the whole
    /// buffer is a prefix; otherwise fires the longest exact mapping at the
    /// start, or passes the first key through, and repeats with the rest.
    /// With `timed_out` it never waits.
    fn resolve(&mut self, timed_out: bool) -> Vec<Feed<A>> {
        let mut out = Vec::new();
        while !self.pending.is_empty() {
            let mut best = None;
            let mut whole_is_prefix = false;
            for len in 1..=self.pending.len() {
                let found = self.keymap.lookup(&self.pending[..len]);
                if let Some(action) = found.exact {
                    best = Some((len, action.clone()));
                }
                if !found.is_prefix {
                    break;
                }
                whole_is_prefix = len == self.pending.len();
            }
            if whole_is_prefix && !timed_out {
                out.push(Feed::Pending);
                return out;
            }
            match best {
                Some((len, action)) => {
                    self.pending.drain(..len);
                    out.push(Feed::Matched(action));
                }
                None => {
                    let key = self.pending.remove(0);
                    match out.last_mut() {
                        Some(Feed::NoMatch(keys)) => keys.push(key),
                        _ => out.push(Feed::NoMatch(vec![key])),
                    }
                }
            }
        }
        self.last_key_at = None;
        out
    }
}

/// What a mapping does: a builtin action by name, or a Lua callback that
/// the plugin host registered under an id.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Action {
    Builtin(String),
    Callback(u64),
}

impl Action {
    /// A builtin action by name.
    pub fn builtin(name: &str) -> Self {
        Self::Builtin(name.to_owned())
    }
}

/// Builtin mappings, with the default leader (documented in `docs/usage.md`):
/// keys, action and what the action does (shown by the help).
const DEFAULTS: &[(&str, &str, &str)] = &[
    ("<CR>", "submit", "Send the prompt (or run a /command)"),
    ("<S-CR>", "newline", "Insert a new line"),
    ("<Esc>", "focus.input", "Go back to the prompt"),
    (
        "<C-c>",
        "input.cancel",
        "Copy the selection (of the prompt, or of the chat when it has the focus), clear the prompt, or interrupt the turn",
    ),
    (
        "<C-S-c>",
        "selection.copy",
        "Copy the selected text (of the prompt, else of the chat); never interrupts",
    ),
    (
        "<C-Insert>",
        "selection.copy",
        "Copy the selected text (of the prompt, else of the chat); never interrupts",
    ),
    ("<F1>", "help.open", "Open the help"),
    ("<leader>h", "help.open", "Open the help"),
    ("<leader>e", "panel.tree", "Show or hide the workspace tree"),
    ("<leader>ff", "picker.files", "Find a file by name"),
    ("<leader>fg", "picker.grep", "Search text in the workspace"),
    (
        "<leader>m",
        "model.pick",
        "Choose the model of the next turn",
    ),
    ("<leader>p", "permission.cycle", "Cycle the permission mode"),
    (
        "<leader>w",
        "focus.next",
        "Move the focus to the next panel",
    ),
    ("<C-Tab>", "focus.next", "Move the focus to the next panel"),
    ("<leader>k", "block.prev", "Select the previous block"),
    ("<leader>j", "block.next", "Select the next block"),
    ("<leader>gg", "block.first", "Select the first block"),
    ("<leader>G", "block.last", "Select the last block"),
    (
        "<leader>o",
        "block.toggle",
        "Expand or collapse the selected block",
    ),
    (
        "<leader>t",
        "block.toggle_view",
        "Switch the block between chart and table",
    ),
    ("<leader>y", "block.yank", "Copy the selected block"),
    (
        "<leader>i",
        "image.mode",
        "Switch pasted images between sending them to the model and adding them to the chat only",
    ),
];

/// What the builtin action `name` does, if it is one.
pub fn action_description(name: &str) -> Option<&'static str> {
    DEFAULTS
        .iter()
        .find(|(_, action, _)| *action == name)
        .map(|(_, _, description)| *description)
}

/// Names of every builtin action, in table order, without repeats.
pub fn builtin_actions() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = Vec::new();
    for (_, action, _) in DEFAULTS {
        if !names.contains(action) {
            names.push(action);
        }
    }
    names
}

/// The builtin keymap. User mappings are layered on top with
/// [`Keymap::set`], which replaces a default with the same keys.
pub fn default_keymap() -> Keymap<Action> {
    let mut keymap = Keymap::new();
    for (notation, name, _) in DEFAULTS {
        // The table is static and covered by tests, so parsing cannot fail;
        // skipping instead of panicking keeps this path free of unwrap.
        if let Ok(keys) = parse_keys(notation, DEFAULT_LEADER) {
            keymap.set_keys(&keys, Action::builtin(name));
        }
    }
    keymap
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
        assert_eq!(
            parse_keys("<leader>e", DEFAULT_LEADER),
            Ok(vec![ctrl('g'), Key::char('e')])
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

    fn keys(n: &str) -> KeySequence {
        parse_keys(n, " ").unwrap_or_default()
    }

    #[test]
    fn keymap_looks_up_exact_and_prefix() {
        let mut km = Keymap::new();
        assert_eq!(km.set_leader(" "), Ok(()));
        assert_eq!(km.set("g", 1), Ok(()));
        assert_eq!(km.set("gg", 2), Ok(()));
        assert_eq!(km.set("<leader>ff", 3), Ok(()));
        let g = km.lookup(&keys("g"));
        assert_eq!((g.exact, g.is_prefix), (Some(&1), true));
        let gg = km.lookup(&keys("gg"));
        assert_eq!((gg.exact, gg.is_prefix), (Some(&2), false));
        let sp = km.lookup(&keys("<Space>f"));
        assert_eq!((sp.exact, sp.is_prefix), (None, true));
        assert_eq!(km.lookup(&keys("<Space>ff")).exact, Some(&3));
    }

    #[test]
    fn keymap_override_and_delete() {
        let mut km = Keymap::new();
        assert_eq!(km.set("<C-j>", "a"), Ok(()));
        assert_eq!(km.set("<C-j>", "b"), Ok(()));
        assert_eq!(km.lookup(&[ctrl('j')]).exact, Some(&"b"));
        assert_eq!(km.del("<C-j>"), Ok(Some("b")));
        assert_eq!(km.lookup(&[ctrl('j')]).exact, None);
        assert_eq!(km.del("<C-j>"), Ok(None));
    }

    #[test]
    fn keymap_leader_is_expanded_at_registration() {
        let mut km = Keymap::new();
        assert_eq!(km.set_leader(","), Ok(()));
        assert_eq!(km.set("<leader>m", 1), Ok(()));
        assert_eq!(km.set_leader(" "), Ok(()));
        assert_eq!(km.lookup(&keys(",m")).exact, Some(&1));
        assert_eq!(
            km.set_leader("<leader>"),
            Err(KeyParseError::LeaderInLeader)
        );
        assert_eq!(km.leader(), " ");
    }

    fn feed_all<A: Clone>(seq: &mut KeySequencer<A>, n: &str, now: Instant) -> Vec<Feed<A>> {
        keys(n).into_iter().flat_map(|k| seq.feed(k, now)).collect()
    }

    fn sequencer() -> KeySequencer<&'static str> {
        let mut km = Keymap::new();
        assert_eq!(km.set_leader(" "), Ok(()));
        for (n, a) in [
            ("gg", "first"),
            ("G", "last"),
            ("j", "next"),
            ("<leader>ff", "files"),
        ] {
            assert_eq!(km.set(n, a), Ok(()));
        }
        KeySequencer::new(km)
    }

    #[test]
    fn sequencer_leader_ff() {
        let mut seq = sequencer();
        let t = Instant::now();
        assert_eq!(seq.feed(Key::char(' '), t), vec![Feed::Pending]);
        assert_eq!(seq.feed(Key::char('f'), t), vec![Feed::Pending]);
        assert_eq!(seq.pending(), keys("<Space>f").as_slice());
        assert_eq!(seq.feed(Key::char('f'), t), vec![Feed::Matched("files")]);
        assert!(seq.pending().is_empty());
    }

    #[test]
    fn sequencer_gg_vs_g_upper() {
        let mut seq = sequencer();
        let t = Instant::now();
        assert_eq!(seq.feed(Key::char('G'), t), vec![Feed::Matched("last")]);
        assert_eq!(seq.feed(Key::char('g'), t), vec![Feed::Pending]);
        assert_eq!(seq.feed(Key::char('g'), t), vec![Feed::Matched("first")]);
    }

    #[test]
    fn sequencer_broken_prefix_replays_rest() {
        let mut seq = sequencer();
        let t = Instant::now();
        // `g` is only a prefix, so it is passed through; `j` still fires.
        assert_eq!(
            feed_all(&mut seq, "gj", t),
            vec![
                Feed::Pending,
                Feed::NoMatch(keys("g")),
                Feed::Matched("next")
            ]
        );
        assert_eq!(
            feed_all(&mut seq, "<Space>x", t),
            vec![Feed::Pending, Feed::NoMatch(keys("<Space>x"))]
        );
        assert_eq!(seq.feed(Key::char('z'), t), vec![Feed::NoMatch(keys("z"))]);
    }

    #[test]
    fn sequencer_sees_overridden_mapping() {
        let mut seq = sequencer();
        let t = Instant::now();
        assert_eq!(seq.keymap_mut().set("j", "custom"), Ok(()));
        assert_eq!(seq.feed(Key::char('j'), t), vec![Feed::Matched("custom")]);
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn timeout_fires_short_mapping_that_is_also_prefix() {
        let mut seq = sequencer();
        assert_eq!(seq.keymap_mut().set("g", "g"), Ok(()));
        let t = Instant::now();
        assert_eq!(seq.feed(Key::char('g'), t), vec![Feed::Pending]);
        assert_eq!(seq.deadline(), Some(t + DEFAULT_TIMEOUTLEN));
        assert_eq!(seq.tick(t + ms(999)), vec![]);
        assert_eq!(seq.tick(t + ms(1000)), vec![Feed::Matched("g")]);
        assert_eq!(seq.deadline(), None);
        assert_eq!(seq.tick(t + ms(2000)), vec![]);
        // Within timeoutlen the longer mapping still wins.
        assert_eq!(seq.feed(Key::char('g'), t + ms(3000)), vec![Feed::Pending]);
        assert_eq!(
            seq.feed(Key::char('g'), t + ms(3999)),
            vec![Feed::Matched("first")]
        );
    }

    #[test]
    fn timeout_without_exact_mapping_drops_prefix() {
        let mut seq = sequencer();
        seq.set_timeoutlen(ms(300));
        let t = Instant::now();
        assert_eq!(
            feed_all(&mut seq, "<Space>f", t),
            vec![Feed::Pending, Feed::Pending]
        );
        assert_eq!(seq.tick(t + ms(300)), vec![Feed::NoMatch(keys("<Space>f"))]);
        assert!(seq.pending().is_empty());
    }

    #[test]
    fn timeout_is_measured_from_last_key() {
        let mut seq = sequencer();
        let t = Instant::now();
        assert_eq!(seq.feed(Key::char(' '), t), vec![Feed::Pending]);
        assert_eq!(seq.feed(Key::char('f'), t + ms(900)), vec![Feed::Pending]);
        assert_eq!(seq.tick(t + ms(1500)), vec![]);
        assert_eq!(
            seq.feed(Key::char('f'), t + ms(1800)),
            vec![Feed::Matched("files")]
        );
    }

    #[test]
    fn late_key_settles_expired_prefix_first() {
        let mut seq = sequencer();
        assert_eq!(seq.keymap_mut().set("g", "g"), Ok(()));
        let t = Instant::now();
        assert_eq!(seq.feed(Key::char('g'), t), vec![Feed::Pending]);
        assert_eq!(
            seq.feed(Key::char('g'), t + ms(1500)),
            vec![Feed::Matched("g"), Feed::Pending]
        );
    }

    #[test]
    fn default_table_parses() {
        for (notation, _, _) in DEFAULTS {
            assert!(parse_keys(notation, DEFAULT_LEADER).is_ok(), "{notation}");
        }
    }

    fn default_keys(n: &str) -> KeySequence {
        parse_keys(n, DEFAULT_LEADER).unwrap_or_default()
    }

    #[test]
    fn default_keymap_follows_the_table() {
        let km = default_keymap();
        let get = |n: &str| km.lookup(&default_keys(n)).exact.cloned();
        let b = |name: &str| Some(Action::builtin(name));
        assert_eq!(get("<Esc>"), b("focus.input"));
        assert_eq!(get("<S-Enter>"), b("newline"));
        assert_eq!(get("<C-g>ff"), b("picker.files"));
        assert_eq!(get("<C-g>e"), b("panel.tree"));
        assert_eq!(get("<C-g>k"), b("block.prev"));
        assert_eq!(get("<C-g>gg"), b("block.first"));
        assert_eq!(get("<C-g>G"), b("block.last"));
        assert_eq!(get("<C-c>"), b("input.cancel"));
        assert_eq!(get("<C-S-c>"), b("selection.copy"));
        assert_eq!(get("<C-Insert>"), b("selection.copy"));
        assert_eq!(get("<F1>"), b("help.open"));
    }

    #[test]
    fn ctrl_shift_letter_is_its_own_chord() {
        let ctrl_shift_c = Key::new(
            KeyCode::Char('c'),
            Modifiers {
                ctrl: true,
                shift: true,
                ..Modifiers::default()
            },
        );
        // As a host may report the shifted letter.
        let reported_upper = Key::new(KeyCode::Char('C'), ctrl_shift_c.mods);
        assert_eq!(reported_upper, ctrl_shift_c);
        assert_ne!(ctrl_shift_c, ctrl('c'));
        assert_eq!(parse_keys("<C-S-c>", " "), Ok(vec![ctrl_shift_c]));
        assert_eq!(parse_keys("<C-S-C>", " "), Ok(vec![ctrl_shift_c]));
        assert_eq!(ctrl_shift_c.to_string(), "<C-S-c>");
        // Neovim notation: an uppercase letter alone does not add a shift.
        assert_eq!(parse_keys("<C-C>", " "), Ok(vec![ctrl('c')]));
        assert_eq!(
            parse_keys("<C-Insert>", " "),
            Ok(vec![Key::new(KeyCode::Insert, ctrl('c').mods)])
        );
    }

    #[test]
    fn no_default_takes_a_printable_key() {
        for entry in default_keymap().entries() {
            let first = entry.keys[0];
            let plain_char = matches!(first.code, KeyCode::Char(_))
                && !(first.mods.ctrl || first.mods.alt || first.mods.cmd);
            assert!(!plain_char, "{} would steal typing", entry.notation());
        }
    }

    #[test]
    fn no_default_needs_alt() {
        for entry in default_keymap().entries() {
            let alt = entry.keys.iter().any(|k| k.mods.alt);
            assert!(
                !alt,
                "{}: Option composes characters on macOS",
                entry.notation()
            );
        }
    }

    #[test]
    fn leader_shift_g_is_an_uppercase_g() {
        let shift_g = Key::new(
            KeyCode::Char('g'),
            Modifiers {
                shift: true,
                ..Modifiers::default()
            },
        );
        let keys = parse_keys("<leader>G", DEFAULT_LEADER).unwrap();
        assert_eq!(keys[1], shift_g);
    }

    #[test]
    fn every_default_action_has_a_description() {
        for (notation, action, description) in DEFAULTS {
            assert!(!description.trim().is_empty(), "{notation} -> {action}");
            assert!(action_description(action).is_some(), "{action}");
        }
        assert_eq!(action_description("nope"), None);
        let actions = builtin_actions();
        assert!(actions.contains(&"help.open"));
        assert_eq!(
            actions.iter().filter(|a| **a == "help.open").count(),
            1,
            "no repeats"
        );
    }

    #[test]
    fn entries_list_defaults_and_overrides_with_the_leader_resolved() {
        let mut km = default_keymap();
        assert_eq!(km.set_leader(","), Ok(()));
        assert_eq!(km.set("<leader>x", Action::Callback(3)), Ok(()));
        assert_eq!(km.set("<C-g>k", Action::builtin("block.last")), Ok(()));
        let entries = km.entries();
        let find = |n: &str| {
            entries
                .iter()
                .find(|e| e.notation() == n)
                .map(|e| e.action.clone())
        };
        assert_eq!(find(",x"), Some(Action::Callback(3)));
        assert_eq!(find("<C-g>k"), Some(Action::builtin("block.last")));
        assert_eq!(find("<C-g>ff"), Some(Action::builtin("picker.files")));
        assert_eq!(find("<CR>"), Some(Action::builtin("submit")));
        assert_eq!(entries.len(), DEFAULTS.len() + 1, "one new mapping");
        let notations: Vec<String> = entries.iter().map(KeymapEntry::notation).collect();
        let mut sorted = notations.clone();
        sorted.sort();
        assert_eq!(notations, sorted, "sorted by keys");
    }

    #[test]
    fn default_keymap_can_be_overridden() {
        let mut seq = KeySequencer::new(default_keymap());
        assert_eq!(
            seq.keymap_mut().set("<leader>ff", Action::Callback(7)),
            Ok(())
        );
        let t = Instant::now();
        let feed = |seq: &mut KeySequencer<Action>, n: &str| -> Vec<Feed<Action>> {
            default_keys(n)
                .into_iter()
                .flat_map(|k| seq.feed(k, t))
                .collect()
        };
        assert_eq!(
            feed(&mut seq, "<C-g>ff"),
            vec![
                Feed::Pending,
                Feed::Pending,
                Feed::Matched(Action::Callback(7))
            ]
        );
        assert_eq!(
            feed(&mut seq, "<C-g>fg").last(),
            Some(&Feed::Matched(Action::builtin("picker.grep")))
        );
    }
}
