//! The command palette: what a `/` line lists, its keys and its rows.

use std::rc::Rc;

use application_core::keymap::{Key, KeyCode, Modifiers};
use gpui::{App, Context, ParentElement as _, SharedString, Styled as _, Window, div};
use gpui_component::{ActiveTheme as _, h_flex};

use super::completion::{Popup, PopupKey, palette_top, window_note};
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
fn palette_indices(commands: &[CommandRow], text: &str) -> Vec<usize> {
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

impl InputView {
    /// The commands the `/` palette offers.
    pub fn set_commands(&mut self, commands: Vec<CommandRow>, cx: &mut Context<Self>) {
        self.commands = commands;
        self.palette_rows.take();
        cx.notify();
    }

    /// The commands the palette shows for the current text.
    pub fn palette(&self, cx: &App) -> Vec<&CommandRow> {
        if self.palette_hidden {
            return Vec::new();
        }
        let text = self.shared_text(cx);
        let mut cached = self.palette_rows.borrow_mut();
        let rows = match cached.as_ref() {
            Some((key, rows)) if *key == text => Rc::clone(rows),
            _ => {
                let rows: Rc<[usize]> = palette_indices(&self.commands, &text).into();
                *cached = Some((text, Rc::clone(&rows)));
                rows
            }
        };
        rows.iter()
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
                    self.record(&format!("/{line}"), cx);
                    self.editor
                        .update(cx, |editor, cx| editor.set_value("", window, cx));
                    cx.emit(InputEvent::Command(line));
                }
                PopupKey::Ignored => return false,
            }
            return true;
        }
        let choices: Vec<Choice> = self.palette(cx).into_iter().map(choice).collect();
        if choices.is_empty() {
            return false;
        }
        let len = choices.len();
        let selected = self.palette_selected.min(len - 1);
        match (key.code, &choices[selected]) {
            (KeyCode::Down | KeyCode::Up, _) => {
                let ix = if key.code == KeyCode::Down {
                    (selected + 1) % len
                } else {
                    (selected + len - 1) % len
                };
                self.palette_top = palette_top(ix, self.palette_top, len);
                self.palette_selected = ix;
            }
            (KeyCode::Esc, _) => self.palette_hidden = true,
            (KeyCode::Tab, Choice::Run(line)) => self.set_text(&format!("/{line} "), window, cx),
            (KeyCode::Tab | KeyCode::Enter, Choice::Complete(line)) => {
                self.set_text(line, window, cx);
            }
            (KeyCode::Enter, Choice::Run(line)) => {
                let line = line.clone();
                self.record(&format!("/{line}"), cx);
                self.editor
                    .update(cx, |editor, cx| editor.set_value("", window, cx));
                cx.emit(InputEvent::Command(line));
            }
            _ => return false,
        }
        true
    }

    pub(super) fn render_palette(&self, cx: &App) -> Option<gpui::AnyElement> {
        let rows = self.palette(cx);
        if rows.is_empty() {
            return None;
        }
        let theme = cx.theme();
        let total = rows.len();
        let top = self.palette_top;
        let note = window_note(top, total).unwrap_or_default();
        let items = completion::render_rows(
            "palette",
            rows.iter(),
            total,
            self.palette_selected,
            top,
            theme,
            |row| {
                h_flex()
                    .gap_3()
                    .px_2()
                    .child(
                        div()
                            .text_color(theme.primary)
                            .child(SharedString::from(row.usage.clone())),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(theme.muted_foreground)
                            .child(SharedString::from(row.description.clone())),
                    )
            },
        );
        Some(completion::frame(
            "command-palette",
            items,
            format!("↑↓ choose · Tab complete · Enter run · Esc hide{note}"),
            theme,
        ))
    }
}
