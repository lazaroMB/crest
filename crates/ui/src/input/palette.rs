//! The command palette: what a `/` line lists, its keys and its rows.

use std::rc::Rc;

use application_core::keymap::{Key, KeyCode, Modifiers};
use gpui::{
    App, Context, Div, IntoElement as _, ParentElement as _, SharedString, Styled as _, Window, div,
};
use gpui_component::{ActiveTheme as _, Theme, h_flex};

use super::completion::{Popup, PopupKey, palette_top, window_note};
use super::groups::{GroupRow, PaletteView, group_indices, view_of};
use super::model_popup::model_key;
use super::path_popup::path_key;
use super::popups::Completion;
use super::theme_popup::theme_key;
use super::{InputEvent, InputView, completion};
use crate::help::CommandRow;

/// The commands the palette offers for `text`, best first: names starting
/// with what follows the `/`, then names containing it. Empty unless
/// `text` is a `/` and a partial name (no arguments yet).
pub fn palette_matches<'a>(commands: &'a [CommandRow], text: &str) -> Vec<&'a CommandRow> {
    palette_indices(commands, text)
        .into_iter()
        .map(|ix| &commands[ix])
        .collect()
}

/// [`palette_matches`] as indices into `commands`.
pub(super) fn palette_indices(commands: &[CommandRow], text: &str) -> Vec<usize> {
    let Some(typed) = text.strip_prefix('/') else {
        return Vec::new();
    };
    if typed.starts_with('/') {
        return Vec::new();
    }
    let typed = typed.to_lowercase();
    if typed.contains(char::is_whitespace) {
        // After a space only the rows named with their argument match
        // (`/mode ` lists `mode plan`...).
        return commands
            .iter()
            .enumerate()
            .filter(|(_, row)| {
                let name = row.name.to_lowercase();
                name.contains(' ') && name.starts_with(&typed)
            })
            .map(|(ix, _)| ix)
            .collect();
    }
    let mut starts: Vec<usize> = Vec::new();
    let mut contains: Vec<usize> = Vec::new();
    for (ix, row) in commands.iter().enumerate() {
        let name = row.name.to_lowercase();
        // A row named with its argument (`mode plan`) waits for the space.
        if name.contains(' ') {
            continue;
        }
        if name.starts_with(&typed) {
            starts.push(ix);
        } else if name.contains(&typed) {
            contains.push(ix);
        }
    }
    starts.append(&mut contains);
    starts
}

/// What choosing a row of the palette does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Choice {
    /// Runs the command (`plugin list`).
    Run(String),
    /// Puts `/name … ` in the prompt for the arguments it needs.
    Complete(String),
}

/// What `Enter` on `row` does: the fixed words of its usage run as they
/// are (`/plugin list`, `/clear`, `/help [section]`), and a usage that
/// needs an argument (`/implement <spec>`) is completed up to it.
pub(super) fn choice(row: &CommandRow) -> Choice {
    let mut fixed = Vec::new();
    for word in row.usage.trim_start_matches('/').split_whitespace() {
        if word.starts_with("@<") {
            // A path: complete up to the `@`, which lists the workspace.
            return Choice::Complete(format!("/{} @", fixed.join(" ")));
        }
        if word.starts_with('<') {
            return Choice::Complete(format!("/{} ", fixed.join(" ")));
        }
        if word.starts_with('[') {
            break;
        }
        fixed.push(word);
    }
    if fixed.is_empty() {
        fixed.push(row.name.as_str());
    }
    Choice::Run(fixed.join(" "))
}

/// What `Tab` and `Enter` do on the chosen row of a list of commands: `Tab`
/// completes it, `Enter` runs it (or completes it up to its argument).
fn command_key(key: &Key, choice: &Choice) -> PopupKey {
    match (key.code, choice) {
        (KeyCode::Tab, Choice::Run(line)) => PopupKey::SetText(format!("/{line} ")),
        (KeyCode::Tab | KeyCode::Enter, Choice::Complete(line)) => PopupKey::SetText(line.clone()),
        (KeyCode::Enter, Choice::Run(line)) => PopupKey::Run(line.clone()),
        _ => PopupKey::Ignored,
    }
}

impl InputView {
    /// The commands the `/` palette offers.
    pub fn set_commands(&mut self, commands: Vec<CommandRow>, cx: &mut Context<Self>) {
        self.commands = commands;
        self.palette_rows.take();
        self.palette_group = None;
        cx.notify();
    }

    /// The view the palette has for the current text.
    pub(super) fn palette_view(&self, text: &str) -> PaletteView<'_> {
        view_of(text, self.palette_group.as_deref())
    }

    /// The commands the palette shows for the current text: those of the
    /// opened group, or the matches of what follows the `/`. Empty in the
    /// groups view, which lists [`Self::palette_groups`].
    pub fn palette(&self, cx: &App) -> Vec<&CommandRow> {
        if self.palette_hidden {
            return Vec::new();
        }
        let text = self.shared_text(cx);
        let indices: Rc<[usize]> = match self.palette_view(&text) {
            PaletteView::Groups => return Vec::new(),
            PaletteView::Group(label) => group_indices(&self.commands, label).into(),
            PaletteView::Search => {
                let mut cached = self.palette_rows.borrow_mut();
                match cached.as_ref() {
                    Some((key, rows)) if *key == text => Rc::clone(rows),
                    _ => {
                        let rows: Rc<[usize]> = palette_indices(&self.commands, &text).into();
                        *cached = Some((text, Rc::clone(&rows)));
                        rows
                    }
                }
            }
        };
        indices
            .iter()
            .filter_map(|&ix| self.commands.get(ix))
            .collect()
    }

    /// The palette's keys; returns whether the key was one.
    pub(super) fn palette_key(
        &mut self,
        key: &Key,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.sequencer.pending().is_empty() || key.mods != Modifiers::default() {
            return false;
        }
        if let Some(shown) = self.completion(cx) {
            let pressed = match &shown {
                Completion::Path(popup) => {
                    path_key(key, popup, &self.text(cx), self.palette_selected)
                }
                Completion::Model(popup) => model_key(key, popup, self.palette_selected),
                Completion::Theme(popup) => theme_key(key, popup, self.palette_selected),
            };
            let theme = matches!(shown, Completion::Theme(_));
            match pressed {
                PopupKey::Select(ix) => {
                    self.palette_top = palette_top(ix, self.palette_top, shown.len());
                    if let Completion::Theme(Popup::Entries(entries)) = &shown {
                        self.theme_previewing = true;
                        cx.emit(InputEvent::ThemePreview(entries[ix].name.clone()));
                    }
                    self.palette_selected = ix;
                }
                PopupKey::Hide => {
                    self.palette_hidden = true;
                    if theme {
                        self.end_theme_preview(cx);
                    }
                }
                PopupKey::SetText(text) => self.set_text(&text, window, cx),
                PopupKey::Run(line) => {
                    if theme {
                        // Saving replaces the preview.
                        self.theme_previewing = false;
                    }
                    self.run_line(line, window, cx);
                }
                PopupKey::Ignored => return false,
            }
            return true;
        }
        let groups = self.palette_groups(cx);
        if !groups.is_empty() {
            return self.groups_key(key, &groups);
        }
        if self.leave_group(key, cx) {
            return true;
        }
        let choices: Vec<Choice> = self.palette(cx).into_iter().map(choice).collect();
        if choices.is_empty() {
            return false;
        }
        if self.palette_step(key, choices.len()) {
            return true;
        }
        match command_key(key, &choices[self.palette_selected.min(choices.len() - 1)]) {
            PopupKey::SetText(text) => self.set_text(&text, window, cx),
            PopupKey::Run(line) => self.run_line(line, window, cx),
            _ => return false,
        }
        true
    }

    /// Records `/line`, clears the prompt and runs the command.
    fn run_line(&mut self, line: String, window: &mut Window, cx: &mut Context<Self>) {
        self.record(&format!("/{line}"), cx);
        self.editor
            .update(cx, |editor, cx| editor.set_value("", window, cx));
        cx.emit(InputEvent::Command(line));
    }

    pub(super) fn render_palette(&self, cx: &App) -> Option<gpui::AnyElement> {
        let theme = cx.theme();
        let (selected, top) = (self.palette_selected, self.palette_top);
        let groups = self.palette_groups(cx);
        if !groups.is_empty() {
            let items = completion::render_rows(
                "palette",
                groups.iter(),
                groups.len(),
                selected,
                top,
                theme,
                |group| group_row(group, theme),
            );
            let note = window_note(top, groups.len()).unwrap_or_default();
            let keys = "↑↓ choose · Enter or → open a group · type to filter · Esc hide";
            return Some(completion::frame(
                "command-palette",
                items,
                format!("{keys}{note}"),
                theme,
            ));
        }
        let rows = self.palette(cx);
        if rows.is_empty() {
            return None;
        }
        let opened = self.palette_group.as_deref();
        let mut items = completion::render_rows(
            "palette",
            rows.iter(),
            rows.len(),
            selected,
            top,
            theme,
            |row| command_row(row, opened.is_none(), theme),
        );
        let keys = match opened {
            Some(label) => {
                items.insert(0, group_title(label, theme));
                "↑↓ choose · Tab complete · Enter run · ← all groups · Esc hide"
            }
            None => "↑↓ choose · Tab complete · Enter run · Esc hide",
        };
        let note = window_note(top, rows.len()).unwrap_or_default();
        Some(completion::frame(
            "command-palette",
            items,
            format!("{keys}{note}"),
            theme,
        ))
    }
}

/// A row of the groups view: the group, how its commands are typed, how many.
fn group_row(group: &GroupRow, theme: &Theme) -> Div {
    h_flex()
        .gap_3()
        .px_2()
        .child(
            div()
                .w_48()
                .flex_none()
                .child(SharedString::from(group.label.clone())),
        )
        .child(muted_cell(group.names.join(" "), theme))
        .child(small_note(group.count.to_string(), theme))
}

/// A row of a list of commands: usage, what it does and, in a search
/// across all groups, the group it belongs to.
fn command_row(row: &CommandRow, tagged: bool, theme: &Theme) -> Div {
    let line = h_flex()
        .gap_3()
        .px_2()
        .child(
            div()
                .flex_none()
                .text_color(theme.primary)
                .child(SharedString::from(row.usage.clone())),
        )
        .child(muted_cell(row.description.clone(), theme));
    if tagged {
        line.child(small_note(row.group.clone(), theme))
    } else {
        line
    }
}

/// The name of the opened group above its commands.
fn group_title(label: &str, theme: &Theme) -> gpui::AnyElement {
    div()
        .px_2()
        .pb_1()
        .text_xs()
        .text_color(theme.muted_foreground)
        .child(SharedString::from(format!("← {label}")))
        .into_any_element()
}

/// Small muted text at the end of a row (a count, a group).
fn small_note(text: String, theme: &Theme) -> Div {
    div()
        .flex_none()
        .text_xs()
        .text_color(theme.muted_foreground)
        .child(SharedString::from(text))
}

/// The text of a row that takes the room left, cut off at its end.
fn muted_cell(text: String, theme: &Theme) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .truncate()
        .text_color(theme.muted_foreground)
        .child(SharedString::from(text))
}
