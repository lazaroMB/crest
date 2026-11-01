//! What the completion popups of the prompt share: the `@` paths
//! ([`super::path_popup`]), the `/model ` models ([`super::model_popup`]),
//! the `/theme ` themes ([`super::theme_popup`]) and, for the window and
//! the frame, the command palette.
//!
//! A popup shows [`PALETTE_ROWS`] entries at a time. `Up`/`Down` walk all
//! of them (wrapping around) and the window scrolls only to keep the chosen
//! one in view ([`palette_top`]); the wheel scrolls it ([`scroll_window`])
//! and the footer says which part is shown ([`window_note`]). `Esc` hides
//! it; what `Tab` and `Enter` do is each popup's ([`nav_key`]).

use application_core::keymap::{Key, KeyCode};
use gpui::{
    AnyElement, App, Div, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    Styled as _, div,
};
use gpui_component::{ActiveTheme as _, Theme, v_flex};

/// Rows of a popup.
pub(super) const PALETTE_ROWS: usize = 8;

/// First visible row of a scrolling popup of `len` entries with entry
/// `selected` chosen, given the one shown so far: the window moves only
/// to keep `selected` in it.
pub(super) fn palette_top(selected: usize, top: usize, len: usize) -> usize {
    let top = if selected < top {
        selected
    } else if selected >= top + PALETTE_ROWS {
        selected + 1 - PALETTE_ROWS
    } else {
        top
    };
    top.min(len.saturating_sub(PALETTE_ROWS))
}

/// The footer note of a popup of `total` entries showing
/// [`PALETTE_ROWS`] from `top`: `· 1–8 of 20 · 12 more`, or nothing when
/// they all fit.
pub(super) fn window_note(top: usize, total: usize) -> Option<String> {
    if total <= PALETTE_ROWS {
        return None;
    }
    let last = (top + PALETTE_ROWS).min(total);
    let below = total - last;
    Some(if below > 0 {
        format!(" · {}–{last} of {total} · {below} more", top + 1)
    } else {
        format!(" · {}–{last} of {total}", top + 1)
    })
}

/// The window of a popup of `len` entries moved by `rows` (down when
/// positive) from `top`, and the chosen entry pulled into it.
pub(super) fn scroll_window(
    top: usize,
    selected: usize,
    rows: isize,
    len: usize,
) -> (usize, usize) {
    let max_top = len.saturating_sub(PALETTE_ROWS);
    let top = top.saturating_add_signed(rows).min(max_top);
    let last = (top + PALETTE_ROWS).min(len).saturating_sub(1);
    (top, selected.clamp(top, last.max(top)))
}

/// What a completion popup shows for the current line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Popup<T> {
    /// The matching entries, best first (never empty).
    Entries(Vec<T>),
    /// Why there is nothing to choose (no match, still listing, an error).
    Message(String),
}

impl<T> Popup<T> {
    /// How many entries there are to choose from.
    #[must_use]
    pub fn len(&self) -> usize {
        match self {
            Self::Entries(entries) => entries.len(),
            Self::Message(_) => 0,
        }
    }

    /// Whether there is nothing to choose.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// What a key pressed while a popup shows asks the input to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PopupKey {
    /// Choose the entry at this index (the `/theme ` popup previews it).
    Select(usize),
    /// Hide the popup until the text changes (the `/theme ` popup goes
    /// back to the theme in use).
    Hide,
    /// Replace the prompt with this text.
    SetText(String),
    /// Run this command (without the slash).
    Run(String),
    /// Not the popup's key: the input handles it.
    Ignored,
}

/// A key of a popup as far as all popups agree on it.
#[derive(Debug, PartialEq, Eq)]
pub enum Nav<'a, T> {
    /// `Up`/`Down`: choose the entry at this index.
    Select(usize),
    /// `Esc`: hide the popup.
    Hide,
    /// `Tab` on the chosen entry.
    Complete(&'a T),
    /// `Enter` on the chosen entry.
    Accept(&'a T),
    /// Not the popup's key.
    Ignored,
}

/// What `key` does in `popup` with entry `selected` chosen: the arrows walk
/// the entries (wrapping around), `Esc` hides it (even with only a message),
/// and `Tab` and `Enter` act on the chosen entry.
#[must_use]
pub fn nav_key<'a, T>(key: &Key, popup: &'a Popup<T>, selected: usize) -> Nav<'a, T> {
    let entries = match popup {
        Popup::Entries(entries) => entries,
        Popup::Message(_) if key.code == KeyCode::Esc => return Nav::Hide,
        Popup::Message(_) => return Nav::Ignored,
    };
    let len = entries.len();
    let selected = selected.min(len - 1);
    match key.code {
        KeyCode::Down => Nav::Select((selected + 1) % len),
        KeyCode::Up => Nav::Select((selected + len - 1) % len),
        KeyCode::Esc => Nav::Hide,
        KeyCode::Tab => Nav::Complete(&entries[selected]),
        KeyCode::Enter => Nav::Accept(&entries[selected]),
        _ => Nav::Ignored,
    }
}

/// The visible rows of a popup of `total` entries from `top` on, the one at
/// `selected` highlighted. `row` draws an entry (an `h_flex` with its
/// cells); each gets the id `(id, index)`.
pub(super) fn render_rows<T>(
    id: &'static str,
    entries: impl IntoIterator<Item = T>,
    total: usize,
    selected: usize,
    top: usize,
    theme: &Theme,
    mut row: impl FnMut(T) -> Div,
) -> Vec<AnyElement> {
    let selected = selected.min(total.saturating_sub(1));
    entries
        .into_iter()
        .enumerate()
        .skip(top)
        .take(PALETTE_ROWS)
        .map(|(ix, entry)| {
            let row = row(entry).id((id, ix));
            if ix == selected {
                row.bg(theme.list_active).rounded_sm().into_any_element()
            } else {
                row.into_any_element()
            }
        })
        .collect()
}

/// How a popup is told apart: its element id, the id of its rows and the
/// keys its footer names.
pub(super) struct Look {
    pub id: &'static str,
    pub row_id: &'static str,
    pub keys: &'static str,
}

/// A popup: its rows (or its message), and a footer that says its keys and
/// which part of the list shows.
pub(super) fn render<T>(
    look: &Look,
    popup: Popup<T>,
    selected: usize,
    top: usize,
    cx: &App,
    row: impl FnMut(T) -> Div,
) -> AnyElement {
    let theme = cx.theme();
    let (rows, note) = match popup {
        Popup::Entries(entries) => {
            let total = entries.len();
            let rows = render_rows(look.row_id, entries, total, selected, top, theme, row);
            (rows, window_note(top, total))
        }
        Popup::Message(message) => (
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
    let footer = format!("{}{}", look.keys, note.unwrap_or_default());
    frame(look.id, rows, footer, theme)
}

/// The box of a popup: `rows` over a `footer` in small muted text.
pub(super) fn frame(
    id: &'static str,
    rows: Vec<AnyElement>,
    footer: String,
    theme: &Theme,
) -> AnyElement {
    v_flex()
        .id(id)
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

    #[test]
    fn keys_walk_hide_and_act_on_the_chosen_entry() {
        let popup = Popup::Entries(vec!["a", "b", "c"]);
        let key = |code| Key::plain(code);
        assert_eq!(nav_key(&key(KeyCode::Down), &popup, 2), Nav::Select(0));
        assert_eq!(nav_key(&key(KeyCode::Up), &popup, 0), Nav::Select(2));
        // A choice past a shrunken list is the last entry.
        assert_eq!(nav_key(&key(KeyCode::Tab), &popup, 9), Nav::Complete(&"c"));
        assert_eq!(nav_key(&key(KeyCode::Enter), &popup, 1), Nav::Accept(&"b"));
        assert_eq!(nav_key(&key(KeyCode::Esc), &popup, 0), Nav::Hide);
        assert_eq!(nav_key(&key(KeyCode::Left), &popup, 0), Nav::Ignored);
        let message: Popup<&str> = Popup::Message("listing…".into());
        assert_eq!(nav_key(&key(KeyCode::Esc), &message, 0), Nav::Hide);
        assert_eq!(nav_key(&key(KeyCode::Down), &message, 0), Nav::Ignored);
        assert_eq!((popup.len(), message.is_empty()), (3, true));
    }
}
