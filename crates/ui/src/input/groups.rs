//! The two views of the command palette: `/` alone lists the command
//! groups, a group lists its commands, and any text after the slash
//! searches every command (see [`super::palette`] for the keys).

use application_core::keymap::{Key, KeyCode};
use gpui::{App, Context};

use super::completion::palette_top;
use super::theme_popup::theme_query;
use super::{InputEvent, InputView};
use crate::help::CommandRow;

/// Which rows the palette lists for a line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PaletteView<'a> {
    /// `/` alone: one row per group.
    Groups,
    /// `/` alone with this group opened: its commands.
    Group(&'a str),
    /// `/` and some text: the commands matching it, from every group.
    Search,
}

/// The view `text` gets, given the group opened (if any).
pub(super) fn view_of<'a>(text: &str, open: Option<&'a str>) -> PaletteView<'a> {
    match (text, open) {
        ("/", Some(label)) => PaletteView::Group(label),
        ("/", None) => PaletteView::Groups,
        _ => PaletteView::Search,
    }
}

/// One row of the groups view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct GroupRow {
    /// The group's name (`Chat`, `Plugin · jira-tools`).
    pub label: String,
    /// How its commands are typed (`/clear`, `!`), once each.
    pub names: Vec<String>,
    /// How many commands it lists.
    pub count: usize,
}

/// Whether the palette lists `row` without arguments typed: a row named
/// with its argument (`mode plan`) waits for the space.
fn is_listed(row: &CommandRow) -> bool {
    !row.name.contains(' ')
}

/// How a command is typed: `/research`, or `!` for the shell.
pub(super) fn invocation(row: &CommandRow) -> String {
    if row.usage.starts_with('!') || row.name == "!" {
        return "!".into();
    }
    let name = row.name.split_whitespace().next().unwrap_or(&row.name);
    format!("/{name}")
}

/// The groups that have commands: the built-in ones in the catalog's order,
/// then the others (one per plugin) as first seen.
pub(super) fn group_rows(commands: &[CommandRow]) -> Vec<GroupRow> {
    let mut groups: Vec<GroupRow> = Vec::new();
    for row in commands.iter().filter(|row| is_listed(row)) {
        let at = groups.iter().position(|g| g.label == row.group);
        let at = at.unwrap_or_else(|| {
            groups.push(GroupRow {
                label: row.group.clone(),
                names: Vec::new(),
                count: 0,
            });
            groups.len() - 1
        });
        let group = &mut groups[at];
        let name = invocation(row);
        if !group.names.contains(&name) {
            group.names.push(name);
        }
        group.count += 1;
    }
    let builtin = application_core::usecases::commands::CommandGroup::ALL.map(|g| g.label());
    let rank = |label: &str| {
        builtin
            .iter()
            .position(|b| *b == label)
            .unwrap_or(builtin.len())
    };
    groups.sort_by_key(|g| rank(&g.label));
    groups
}

/// The commands of the group `label`, as indices into `commands`.
pub(super) fn group_indices(commands: &[CommandRow], label: &str) -> Vec<usize> {
    (0..commands.len())
        .filter(|&ix| is_listed(&commands[ix]) && commands[ix].group == label)
        .collect()
}

impl InputView {
    /// The group rows the palette shows: only for a `/` alone.
    pub(super) fn palette_groups(&self, cx: &App) -> Vec<GroupRow> {
        let listed =
            !self.palette_hidden && self.palette_view(&self.shared_text(cx)) == PaletteView::Groups;
        if listed {
            group_rows(&self.commands)
        } else {
            Vec::new()
        }
    }

    /// How many rows the palette lists, of whichever view.
    pub(super) fn palette_len(&self, cx: &App) -> usize {
        self.palette(cx).len() + self.palette_groups(cx).len()
    }

    /// `Up`/`Down` (wrapping) and `Esc` in a palette of `len` rows;
    /// returns whether `key` was one of them.
    pub(super) fn palette_step(&mut self, key: &Key, len: usize) -> bool {
        let selected = self.palette_selected.min(len - 1);
        let ix = match key.code {
            KeyCode::Down => (selected + 1) % len,
            KeyCode::Up => (selected + len - 1) % len,
            KeyCode::Esc => {
                self.palette_hidden = true;
                return true;
            }
            _ => return false,
        };
        self.palette_top = palette_top(ix, self.palette_top, len);
        self.palette_selected = ix;
        true
    }

    /// The keys of the groups view: `Enter`, `Right` and `Tab` open the
    /// chosen group. Anything else is the line's (`Backspace` deletes the
    /// slash).
    pub(super) fn groups_key(&mut self, key: &Key, groups: &[GroupRow]) -> bool {
        match key.code {
            KeyCode::Enter | KeyCode::Right | KeyCode::Tab => {
                let selected = self.palette_selected.min(groups.len() - 1);
                self.palette_group = Some(groups[selected].label.clone());
                self.palette_selected = 0;
                self.palette_top = 0;
                true
            }
            _ => self.palette_step(key, groups.len()),
        }
    }

    /// `Left`, or `Backspace` on the `/`, goes from an opened group back to
    /// the groups, with that group chosen. Returns whether it did.
    pub(super) fn leave_group(&mut self, key: &Key, cx: &App) -> bool {
        let Some(open) = self
            .palette_group
            .as_deref()
            .filter(|_| !self.palette_hidden)
        else {
            return false;
        };
        if !matches!(key.code, KeyCode::Left | KeyCode::Backspace) {
            return false;
        }
        let at = group_rows(&self.commands)
            .iter()
            .position(|group| group.label == open);
        self.palette_group = None;
        self.palette_selected = at.unwrap_or(0);
        self.palette_top = palette_top(self.palette_selected, 0, self.palette_len(cx));
        true
    }

    /// The text changed: the popups start over and ask for what the new text needs.
    pub(super) fn on_change(&mut self, cx: &mut Context<Self>) {
        self.reset_palette();
        let text = self.text(cx);
        self.paths.request(&text, cx);
        if self.models.wants_listing(&text) {
            cx.emit(InputEvent::ModelsWanted);
        }
        if self.themes.wants_listing(&text) {
            cx.emit(InputEvent::ThemesWanted);
        }
        if theme_query(&text).is_none() {
            self.end_theme_preview(cx);
            // Theme files may change: list them again next time.
            self.themes.forget();
        }
        cx.notify();
    }

    /// A change of the text starts the palette over: the first row, the
    /// groups, shown again.
    pub(super) fn reset_palette(&mut self) {
        self.palette_selected = 0;
        self.palette_top = 0;
        self.palette_hidden = false;
        self.palette_group = None;
    }
}

/// Commands of several groups, for the tests of the palette.
#[cfg(test)]
pub(super) fn sample() -> Vec<CommandRow> {
    let table = "help|/help|App
clear|/clear|Chat
!|! <command>|Chat
jira|/jira|Plugin · jira-tools
plugin|/plugin list|Workspace
plugin|/plugin trust <name>|Workspace
mode plan|/mode plan|Model & permissions
mode|/mode [name]|Model & permissions
standup|/standup|Plugin · jira-tools";
    let row = |line: &str| {
        let mut cells = line.split('|').map(String::from);
        CommandRow {
            name: cells.next().unwrap_or_default(),
            usage: cells.next().unwrap_or_default(),
            group: cells.next().unwrap_or_default(),
            ..CommandRow::default()
        }
    };
    table.lines().map(row).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_view_follows_the_text_and_the_open_group() {
        for (text, open, want) in [
            ("/", None, PaletteView::Groups),
            ("/", Some("Chat"), PaletteView::Group("Chat")),
            ("/c", Some("Chat"), PaletteView::Search),
            ("/ ", None, PaletteView::Search),
        ] {
            assert_eq!(view_of(text, open), want, "{text:?} {open:?}");
        }
    }

    #[test]
    fn groups_come_in_catalog_order_then_one_per_plugin() {
        let groups = group_rows(&sample());
        let shown: Vec<_> = groups
            .iter()
            .map(|g| (g.label.as_str(), g.count, g.names.join(" ")))
            .collect();
        assert_eq!(
            shown,
            [
                ("Chat", 2, "/clear !".to_string()),
                ("Model & permissions", 1, "/mode".to_string()),
                ("Workspace", 2, "/plugin".to_string()),
                ("App", 1, "/help".to_string()),
                ("Plugin · jira-tools", 2, "/jira /standup".to_string()),
            ]
        );
        assert!(groups.len() <= super::super::completion::PALETTE_ROWS);
    }

    #[test]
    fn a_group_lists_only_its_own_commands() {
        let rows = sample();
        let names = |label| -> Vec<String> {
            group_indices(&rows, label)
                .iter()
                .map(|&ix| invocation(&rows[ix]))
                .collect()
        };
        assert_eq!(names("Chat"), ["/clear", "!"]);
        assert_eq!(names("Plugin · jira-tools"), ["/jira", "/standup"]);
        assert!(names("Nope").is_empty());
    }
}
