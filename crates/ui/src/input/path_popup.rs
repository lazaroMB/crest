//! The `@` popup of the prompt: lists the workspace directory the last
//! `@` word names ([`super::paths`]) through the [`FileSearchPort`], keeps
//! the listings of that word, and turns the popup's keys into what the
//! input should do. The [`InputView`] owns it, feeds it the text and the
//! keys, and applies the result to the editor.

use std::collections::HashMap;
use std::sync::Arc;

use application::keymap::{Key, KeyCode};
use application::ports::search::{FileSearchPort, TreeNodeKind, TreeRequest};
use domain::workspace::WorkspacePath;
use gpui::{
    AnyElement, App, Context, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, Styled as _, Task, div,
};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};

use super::paths::{PathEntry, TokenDir, complete, path_matches, path_token, token_dir};
use super::{InputView, PALETTE_ROWS, window_note};

/// What the `@` popup shows for the current word.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathPopup {
    /// The matching entries of the directory, best first.
    Entries(Vec<PathEntry>),
    /// Why there is nothing to choose (no match, still listing, an error).
    Message(String),
}

/// What a key pressed while the popup shows asks the input to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathKey {
    /// Choose the entry at this index.
    Select(usize),
    /// Hide the popup until the text changes.
    Hide,
    /// Replace the prompt with this text (a completed entry).
    SetText(String),
    /// Not the popup's key: the input handles it (`Enter` on a word that
    /// is already the chosen entry submits the line).
    Ignored,
}

/// The entries of a directory, as far as they are known.
enum Listing {
    Loading,
    Ready(Vec<PathEntry>),
    Failed(String),
}

/// Listings of the directories `@` words name.
#[derive(Default)]
pub struct PathCompletion {
    files: Option<Arc<dyn FileSearchPort>>,
    listings: HashMap<WorkspacePath, Listing>,
    tasks: Vec<Task<()>>,
}

impl PathCompletion {
    /// Lists directories with `files`; without it there are no suggestions.
    pub fn set_file_search(&mut self, files: Arc<dyn FileSearchPort>) {
        self.files = Some(files);
        self.forget();
    }

    fn forget(&mut self) {
        self.listings.clear();
        self.tasks.clear();
    }

    /// What the popup shows for `text`, if it ends with an `@` word.
    pub fn popup(&self, text: &str) -> Option<PathPopup> {
        self.files.as_ref()?;
        let token = path_token(text)?;
        let dir = match token_dir(&token) {
            TokenDir::Inside(dir) => dir,
            TokenDir::Outside => {
                return Some(PathPopup::Message(
                    "only paths inside the workspace can be completed".into(),
                ));
            }
        };
        let shown = if dir.is_root() {
            "the workspace".to_owned()
        } else {
            format!("{}/", dir.as_str())
        };
        Some(match self.listings.get(&dir) {
            None | Some(Listing::Loading) => PathPopup::Message(format!("listing {shown}…")),
            Some(Listing::Failed(err)) => PathPopup::Message(format!("cannot list {shown}: {err}")),
            Some(Listing::Ready(entries)) => {
                let matches: Vec<PathEntry> = path_matches(entries, token.prefix)
                    .into_iter()
                    .cloned()
                    .collect();
                if matches.is_empty() && token.prefix.is_empty() {
                    PathPopup::Message(format!("nothing to open in {shown}"))
                } else if matches.is_empty() {
                    PathPopup::Message(format!("nothing in {shown} matches `{}`", token.prefix))
                } else {
                    PathPopup::Entries(matches)
                }
            }
        })
    }

    /// Lists the directory the `@` word of `text` names unless it is known;
    /// with no `@` word the listings are forgotten, so the next one sees new
    /// files.
    pub fn request(&mut self, text: &str, cx: &mut Context<InputView>) {
        let Some(files) = self.files.clone() else {
            return;
        };
        let Some(token) = path_token(text) else {
            self.forget();
            return;
        };
        let TokenDir::Inside(dir) = token_dir(&token) else {
            return;
        };
        if self.listings.contains_key(&dir) {
            return;
        }
        self.listings.insert(dir.clone(), Listing::Loading);
        let request = TreeRequest {
            path: Some(dir.clone()),
            depth: 1,
        };
        self.tasks.push(cx.spawn(async move |this, cx| {
            let result = files.tree(request).await;
            let listing = match result {
                Ok(tree) => Listing::Ready(
                    tree.children
                        .into_iter()
                        .map(|node| PathEntry {
                            dir: node.kind == TreeNodeKind::Dir,
                            name: node.name,
                        })
                        .collect(),
                ),
                Err(err) => Listing::Failed(err.to_string()),
            };
            this.update(cx, |input, cx| {
                // Forgotten meanwhile (the `@` word is gone): drop it.
                if let Some(known) = input.paths.listings.get_mut(&dir) {
                    *known = listing;
                    cx.notify();
                }
            })
            .ok();
        }));
    }
}

/// What `key` does in `popup` for the prompt `text`, with entry `selected`
/// chosen.
pub fn path_key(key: &Key, popup: &PathPopup, text: &str, selected: usize) -> PathKey {
    let entries = match popup {
        PathPopup::Entries(entries) => entries,
        PathPopup::Message(_) if key.code == KeyCode::Esc => return PathKey::Hide,
        PathPopup::Message(_) => return PathKey::Ignored,
    };
    let len = entries.len();
    let selected = selected.min(len - 1);
    let Some(token) = path_token(text) else {
        return PathKey::Ignored;
    };
    let completed = complete(text, &token, &entries[selected]);
    match key.code {
        KeyCode::Down => PathKey::Select((selected + 1) % len),
        KeyCode::Up => PathKey::Select((selected + len - 1) % len),
        KeyCode::Esc => PathKey::Hide,
        // Already the chosen entry: the line is submitted.
        KeyCode::Enter if completed.trim_end() == text.trim_end() => PathKey::Ignored,
        KeyCode::Tab | KeyCode::Enter => PathKey::SetText(completed),
        _ => PathKey::Ignored,
    }
}

/// Draws `popup` from entry `top` on, with entry `selected` highlighted.
pub fn render(popup: PathPopup, selected: usize, top: usize, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let (rows, note) = match popup {
        PathPopup::Entries(entries) => {
            let selected = selected.min(entries.len() - 1);
            let note = window_note(top, entries.len());
            let rows: Vec<_> = entries
                .into_iter()
                .enumerate()
                .skip(top)
                .take(PALETTE_ROWS)
                .map(|(ix, entry)| {
                    let (marker, name, color) = if entry.dir {
                        ("▸", format!("{}/", entry.name), theme.primary)
                    } else {
                        (" ", entry.name, theme.foreground)
                    };
                    let mut row = h_flex()
                        .id(("path", ix))
                        .gap_2()
                        .px_2()
                        .child(div().text_color(theme.muted_foreground).child(marker))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_color(color)
                                .child(SharedString::from(name)),
                        );
                    if ix == selected {
                        row = row.bg(theme.list_active).rounded_sm();
                    }
                    row.into_any_element()
                })
                .collect();
            (rows, note)
        }
        PathPopup::Message(message) => (
            vec![
                div()
                    .px_2()
                    .text_color(theme.muted_foreground)
                    .child(SharedString::from(message))
                    .into_any_element(),
            ],
            None,
        ),
    };
    let footer = format!(
        "↑↓ choose · Tab complete · Enter accept · Esc hide{}",
        note.unwrap_or_default()
    );
    v_flex()
        .id("path-palette")
        .mb_1()
        .py_1()
        .border_1()
        .border_color(theme.border)
        .rounded_md()
        .bg(theme.popover)
        .text_sm()
        .children(rows)
        .child(
            div()
                .px_2()
                .pt_1()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(SharedString::from(footer)),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(names: &[(&str, bool)]) -> PathPopup {
        PathPopup::Entries(
            names
                .iter()
                .map(|(name, dir)| PathEntry {
                    name: (*name).into(),
                    dir: *dir,
                })
                .collect(),
        )
    }

    #[test]
    fn keys_choose_complete_and_submit() {
        let popup = entries(&[("src", true), ("main.rs", false)]);
        let key = |code| Key::plain(code);
        assert_eq!(
            path_key(&key(KeyCode::Down), &popup, "@", 0),
            PathKey::Select(1)
        );
        assert_eq!(
            path_key(&key(KeyCode::Up), &popup, "@", 0),
            PathKey::Select(1)
        );
        assert_eq!(
            path_key(&key(KeyCode::Tab), &popup, "/open @", 0),
            PathKey::SetText("/open @src/".into())
        );
        assert_eq!(
            path_key(&key(KeyCode::Enter), &popup, "@ma", 1),
            PathKey::SetText("@main.rs ".into())
        );
        // A name with spaces switches to the quoted form, which keeps
        // completing inside the directory.
        let spaced = entries(&[("My Docs", true), ("notes v2.md", false)]);
        assert_eq!(
            path_key(&key(KeyCode::Tab), &spaced, "/open @My", 0),
            PathKey::SetText("/open @\"My Docs/".into())
        );
        assert_eq!(
            path_key(&key(KeyCode::Enter), &spaced, "/open @\"My Docs/no", 1),
            PathKey::SetText("/open @\"My Docs/notes v2.md\" ".into())
        );
        // The word already is the entry: Enter is the input's.
        let file = entries(&[("main.rs", false)]);
        assert_eq!(
            path_key(&key(KeyCode::Enter), &file, "@main.rs", 0),
            PathKey::Ignored
        );
        assert_eq!(path_key(&key(KeyCode::Esc), &popup, "@", 0), PathKey::Hide);
        let message = PathPopup::Message("listing…".into());
        assert_eq!(
            path_key(&key(KeyCode::Esc), &message, "@", 0),
            PathKey::Hide
        );
        assert_eq!(
            path_key(&key(KeyCode::Enter), &message, "@", 0),
            PathKey::Ignored
        );
    }

    #[test]
    fn without_a_file_search_there_is_no_popup() {
        assert_eq!(PathCompletion::default().popup("@src/"), None);
    }
}
