//! The prompt mode control: what the prompt sends a line as (chat, research
//! or deep research) and the model of the menu of its button. What it looks
//! like is in [`super::mode_view`].
//!
//! The button opens a menu above the prompt with the modes, two inserts
//! (`@` a workspace file, `/` a command) and the MCP servers manager. In
//! the menu `Up`/`Down` choose,
//! `Enter` picks and `Esc` closes it; any other key closes it and is typed.
//! `prompt.mode` ([`PROMPT_MODE_ACTION`]) cycles the modes from the
//! keyboard, and typing `@` or `/` does what the inserts do.
//!
//! The mode stays until it is changed back: sending does not reset it. Its
//! chip's `×`, `Backspace` in an empty prompt and choosing Chat do.

use application_core::keymap::{Key, KeyCode, Modifiers};
use gpui::{Context, Window};

use super::{InputEvent, InputView, placeholder_with};

/// Action of `<leader>M`: cycles the prompt mode.
pub const PROMPT_MODE_ACTION: &str = "prompt.mode";

/// What the prompt sends a line as.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ChatMode {
    /// The line goes to the agent as typed.
    #[default]
    Chat,
    /// The line is the problem of `/research`.
    Research,
    /// The line is the problem of `/research --deep`.
    Deep,
}

/// The words of a mode.
struct Words {
    label: &'static str,
    hint: &'static str,
    placeholder: &'static str,
    /// What goes before the text in the line sent; none for chat.
    prefix: Option<&'static str>,
}

/// The words of every mode, in the order of [`ChatMode::ALL`].
const WORDS: [Words; 3] = [
    Words {
        label: "Chat",
        hint: "talk to the agent",
        placeholder: "Ask crest…",
        prefix: None,
    },
    Words {
        label: "Research",
        hint: "5 stages, gates where you decide",
        placeholder: "Describe the problem to research…",
        prefix: Some("/research "),
    },
    Words {
        label: "Deep research",
        hint: "14 stages, experiments and a prototype",
        placeholder: "Describe the problem for a deep research…",
        prefix: Some("/research --deep "),
    },
];

impl ChatMode {
    /// Every mode, in the order of the menu and of the cycle.
    pub const ALL: [Self; 3] = [Self::Chat, Self::Research, Self::Deep];

    fn words(self) -> &'static Words {
        &WORDS[self as usize]
    }

    /// The name the menu and the chip show.
    #[must_use]
    pub fn label(self) -> &'static str {
        self.words().label
    }

    /// What the mode does, shown in the menu.
    #[must_use]
    pub fn hint(self) -> &'static str {
        self.words().hint
    }

    /// What the empty prompt asks for.
    #[must_use]
    pub fn placeholder(self) -> &'static str {
        self.words().placeholder
    }

    /// The line sent for `text`: in research the command a person could
    /// type, `/research <text>` (`/research --deep <text>` in deep
    /// research). A line that starts with `/` or `!` and an empty one go as
    /// typed, and chat never changes the line.
    #[must_use]
    pub fn command_for(self, text: &str) -> String {
        let body = text.trim();
        match self.words().prefix {
            Some(prefix) if !body.is_empty() && !body.starts_with(['/', '!']) => {
                format!("{prefix}{body}")
            }
            _ => text.to_owned(),
        }
    }

    /// The mode after this one: chat, research, deep research, chat.
    #[must_use]
    pub fn next(self) -> Self {
        Self::ALL[(self as usize + 1) % Self::ALL.len()]
    }
}

/// A row of the menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuEntry {
    /// Switch to this mode.
    Mode(ChatMode),
    /// Type this character in the prompt, which opens its completion.
    Insert(char),
    /// Open the MCP servers manager (`/mcp`).
    Mcp,
}

/// The rows of the menu: the modes, the inserts, then the manager.
pub const MENU: [MenuEntry; 6] = [
    MenuEntry::Mode(ChatMode::Chat),
    MenuEntry::Mode(ChatMode::Research),
    MenuEntry::Mode(ChatMode::Deep),
    MenuEntry::Insert('@'),
    MenuEntry::Insert('/'),
    MenuEntry::Mcp,
];

impl MenuEntry {
    /// The row's name and its note: what a mode does, or the character an
    /// insert types.
    #[must_use]
    pub fn text(self) -> (&'static str, &'static str) {
        match self {
            Self::Mode(mode) => (mode.label(), mode.hint()),
            Self::Insert('@') => ("File from the workspace", "@"),
            Self::Insert(_) => ("Command", "/"),
            Self::Mcp => ("MCP servers", "/mcp"),
        }
    }
}

/// What a key did in the menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Press {
    /// Not the menu's: it was closed if open, and the key goes on.
    Pass,
    /// The menu took it.
    Taken,
    /// `Enter` on this row; the menu closed.
    Pick(MenuEntry),
}

/// The mode of the prompt and the menu of its button.
#[derive(Debug, Default)]
pub(super) struct ModeControl {
    pub mode: ChatMode,
    /// The chosen row while the menu is open.
    pub menu: Option<usize>,
}

impl ModeControl {
    /// Opens the menu on the current mode, or closes it.
    pub fn toggle(&mut self) {
        let current = ChatMode::ALL.iter().position(|m| *m == self.mode);
        self.menu = self.menu.map_or(current, |_| None);
    }

    /// A key while the menu may be open: the arrows choose (wrapping around),
    /// `Enter` picks, `Esc` closes; anything else closes it and goes on.
    pub fn press(&mut self, key: &Key) -> Press {
        let (Some(at), true) = (self.menu, key.mods == Modifiers::default()) else {
            return self.pass();
        };
        let len = MENU.len();
        match key.code {
            KeyCode::Down => self.menu = Some((at + 1) % len),
            KeyCode::Up => self.menu = Some((at + len - 1) % len),
            KeyCode::Enter => {
                self.menu = None;
                return Press::Pick(MENU[at.min(len - 1)]);
            }
            KeyCode::Esc => self.menu = None,
            _ => return self.pass(),
        }
        Press::Taken
    }

    fn pass(&mut self) -> Press {
        self.menu = None;
        Press::Pass
    }
}

impl InputView {
    /// The mode the prompt sends its lines in.
    pub fn chat_mode(&self) -> ChatMode {
        self.mode_control.mode
    }

    /// Whether the mode menu is open.
    pub fn mode_menu_open(&self) -> bool {
        self.mode_control.menu.is_some()
    }

    /// Changes the mode, and with it what the empty prompt asks for.
    pub fn set_chat_mode(&mut self, mode: ChatMode, window: &mut Window, cx: &mut Context<Self>) {
        self.mode_control.mode = mode;
        let placeholder = placeholder_with(mode, self.sequencer.keymap());
        self.editor.update(cx, |editor, cx| {
            editor.set_placeholder(placeholder, window, cx);
        });
        cx.notify();
    }

    /// `prompt.mode`: the next mode.
    pub(super) fn cycle_mode(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_chat_mode(self.mode_control.mode.next(), window, cx);
    }

    /// Does what `entry` says and gives the prompt the focus; closes the menu.
    pub(super) fn pick_entry(
        &mut self,
        entry: MenuEntry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.mode_control.menu = None;
        self.focus(window, cx);
        match entry {
            MenuEntry::Mode(mode) => self.set_chat_mode(mode, window, cx),
            MenuEntry::Insert(c) => self.insert_text(&c.to_string(), window, cx),
            MenuEntry::Mcp => cx.emit(InputEvent::Command("mcp".into())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mode_sends_the_command_a_person_could_type() {
        // (mode, typed, sent)
        let table = [
            (ChatMode::Chat, "why", "why"),
            (ChatMode::Research, "why", "/research why"),
            (ChatMode::Research, "  why  ", "/research why"),
            (ChatMode::Deep, "why", "/research --deep why"),
            (ChatMode::Deep, "/help", "/help"),
            (ChatMode::Deep, "!ls", "!ls"),
            (ChatMode::Research, "  !ls", "  !ls"),
            (ChatMode::Research, "//x", "//x"),
            (ChatMode::Research, "", ""),
            (ChatMode::Deep, "   ", "   "),
        ];
        for (mode, typed, sent) in table {
            assert_eq!(mode.command_for(typed), sent, "{mode:?} {typed:?}");
        }
    }

    #[test]
    fn the_modes_cycle_and_the_menu_lists_the_modes_the_inserts_and_the_manager() {
        let cycle = ChatMode::ALL.map(ChatMode::next);
        assert_eq!(cycle, [ChatMode::Research, ChatMode::Deep, ChatMode::Chat]);
        assert_eq!(ChatMode::default(), ChatMode::Chat);
        let rows = MENU.map(|entry| entry.text());
        assert_eq!(
            rows[2],
            ("Deep research", "14 stages, experiments and a prototype")
        );
        assert_eq!(
            rows[3..],
            [
                ("File from the workspace", "@"),
                ("Command", "/"),
                ("MCP servers", "/mcp")
            ]
        );
    }

    #[test]
    fn the_menu_keys_walk_pick_and_close() {
        let key = |code| Key::plain(code);
        let mut control = ModeControl::default();
        assert_eq!(control.press(&key(KeyCode::Down)), Press::Pass, "closed");
        control.toggle();
        assert_eq!(control.press(&key(KeyCode::Up)), Press::Taken);
        assert_eq!(control.menu, Some(5), "wraps around");
        for _ in 0..3 {
            control.press(&key(KeyCode::Down));
        }
        assert_eq!(control.press(&key(KeyCode::Enter)), Press::Pick(MENU[2]));
        assert_eq!(control.menu, None);
        control.toggle();
        assert_eq!(control.press(&key(KeyCode::Esc)), Press::Taken);
        control.toggle();
        assert_eq!(control.press(&key(KeyCode::Char('a'))), Press::Pass);
        assert_eq!(control.menu, None, "a typed key closes it");
    }
}
