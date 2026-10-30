//! The `/theme ` popup of the prompt: while the line is `/theme ` and a
//! word, it lists the themes whose name matches the word, as the `/model `
//! popup lists models. The session hands it the themes once it has
//! resolved them ([`ThemeCompletion::set`]).
//!
//! Moving through the list previews each theme in the window
//! ([`ThemeKey::Preview`]); `Enter` saves the chosen one (`/theme set
//! <name>`), `Tab` completes the line to it and `Esc` hides the popup and
//! goes back to the theme in use.

use gpui::{
    AnyElement, App, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    Styled as _, div,
};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};

use super::{PALETTE_ROWS, window_note};
use crate::session::ThemeChoice;

/// What the popup shows for the current line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThemePopup {
    /// The matching themes, by name.
    Entries(Vec<ThemeChoice>),
    /// Why there is nothing to choose (no match, still loading, an error).
    Message(String),
}

/// What a key pressed while the popup shows asks the input to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThemeKey {
    /// Choose the entry at this index and preview its theme.
    Preview(usize),
    /// Hide the popup and go back to the theme in use.
    Hide,
    /// Replace the prompt with this text.
    SetText(String),
    /// Run this command (without the slash).
    Run(String),
    /// Not the popup's key.
    Ignored,
}

#[derive(Debug, Default)]
enum Listing {
    #[default]
    NotAsked,
    Loading,
    Ready(Vec<ThemeChoice>),
    Failed(String),
}

/// The themes the popup offers.
#[derive(Debug, Default)]
pub struct ThemeCompletion {
    listing: Listing,
}

/// The word after `/theme ` (or `/theme set `) in `text`, if the line is
/// that command and the word is still being typed.
#[must_use]
pub fn theme_query(text: &str) -> Option<&str> {
    let rest = text.strip_prefix("/theme ")?;
    let rest = rest.strip_prefix("set ").unwrap_or(rest);
    (!rest.contains(char::is_whitespace)).then_some(rest)
}

impl ThemeCompletion {
    /// Whether `text` opens the popup and the themes were not asked for
    /// since the last [`Self::forget`]; they then count as asked.
    pub fn wants_listing(&mut self, text: &str) -> bool {
        if theme_query(text).is_none() || !matches!(self.listing, Listing::NotAsked) {
            return false;
        }
        self.listing = Listing::Loading;
        true
    }

    /// Asks again the next time the popup opens (theme files may have
    /// changed on disk).
    pub fn forget(&mut self) {
        self.listing = Listing::NotAsked;
    }

    /// The themes, or why they could not be listed.
    pub fn set(&mut self, themes: Result<Vec<ThemeChoice>, String>) {
        self.listing = match themes {
            Ok(themes) => Listing::Ready(themes),
            Err(error) => Listing::Failed(error),
        };
    }

    /// What the popup shows for `text`, if it is a `/theme ` line.
    #[must_use]
    pub fn popup(&self, text: &str) -> Option<ThemePopup> {
        let query = theme_query(text)?.to_lowercase();
        Some(match &self.listing {
            Listing::NotAsked | Listing::Loading => {
                ThemePopup::Message("loading the themes…".into())
            }
            Listing::Failed(error) => ThemePopup::Message(format!("cannot list themes: {error}")),
            Listing::Ready(themes) => {
                let (mut starts, mut contains): (Vec<_>, Vec<_>) = themes
                    .iter()
                    .filter(|t| t.name.contains(&query))
                    .cloned()
                    .partition(|t| t.name.starts_with(&query));
                starts.append(&mut contains);
                if starts.is_empty() {
                    ThemePopup::Message(format!("no theme matches `{query}`"))
                } else {
                    ThemePopup::Entries(starts)
                }
            }
        })
    }
}

/// What `key` does in `popup` with entry `selected` chosen.
#[must_use]
pub fn theme_key(key: &application::keymap::Key, popup: &ThemePopup, selected: usize) -> ThemeKey {
    use application::keymap::KeyCode;
    let entries = match popup {
        ThemePopup::Entries(entries) => entries,
        ThemePopup::Message(_) if key.code == KeyCode::Esc => return ThemeKey::Hide,
        ThemePopup::Message(_) => return ThemeKey::Ignored,
    };
    let len = entries.len();
    let selected = selected.min(len - 1);
    let name = &entries[selected].name;
    match key.code {
        KeyCode::Down => ThemeKey::Preview((selected + 1) % len),
        KeyCode::Up => ThemeKey::Preview((selected + len - 1) % len),
        KeyCode::Esc => ThemeKey::Hide,
        KeyCode::Tab => ThemeKey::SetText(format!("/theme set {name}")),
        KeyCode::Enter => ThemeKey::Run(format!("theme set {name}")),
        _ => ThemeKey::Ignored,
    }
}

/// Draws `popup` from entry `top` on, with entry `selected` highlighted;
/// `current` is the theme in use, marked in the list.
pub fn render(
    popup: ThemePopup,
    selected: usize,
    top: usize,
    current: &str,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let (rows, position) = match popup {
        ThemePopup::Entries(entries) => {
            let total = entries.len();
            let selected = selected.min(total - 1);
            let position = window_note(top, total);
            let rows: Vec<_> = entries
                .into_iter()
                .enumerate()
                .skip(top)
                .take(PALETTE_ROWS)
                .map(|(ix, choice)| {
                    let marker = if choice.name == current { "●" } else { " " };
                    // A swatch of the theme itself: background, accent, text.
                    let swatch = |path: &str| {
                        div()
                            .size_3()
                            .rounded_sm()
                            .bg(crate::theme::hsla(choice.theme.color(path)))
                    };
                    let mut row =
                        h_flex()
                            .id(("theme", ix))
                            .gap_2()
                            .px_2()
                            .child(div().text_color(theme.primary).child(marker))
                            .child(
                                h_flex()
                                    .gap_0p5()
                                    .child(swatch("background"))
                                    .child(swatch("accent.bg"))
                                    .child(swatch("foreground")),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(theme.foreground)
                                    .child(SharedString::from(choice.name.clone())),
                            )
                            .child(div().text_color(theme.muted_foreground).child(
                                SharedString::from(format!(
                                    "{} · {}",
                                    choice.theme.appearance.name(),
                                    choice.origin
                                )),
                            ));
                    if ix == selected {
                        row = row.bg(theme.list_active).rounded_sm();
                    }
                    row.into_any_element()
                })
                .collect();
            (rows, position)
        }
        ThemePopup::Message(message) => (
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
    let mut footer = "↑↓ preview · Enter save · Tab complete · Esc back".to_owned();
    if let Some(position) = position {
        footer.push_str(&position);
    }
    v_flex()
        .id("theme-palette")
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
    use application::keymap::{Key, KeyCode};

    use super::*;

    fn choice(name: &str) -> ThemeChoice {
        ThemeChoice {
            name: name.into(),
            origin: "file".into(),
            theme: crate::theme::default_theme(),
        }
    }

    fn ready(names: &[&str]) -> ThemeCompletion {
        let mut themes = ThemeCompletion::default();
        themes.set(Ok(names.iter().copied().map(choice).collect()));
        themes
    }

    #[test]
    fn only_a_theme_line_opens_it() {
        assert_eq!(theme_query("/theme "), Some(""));
        assert_eq!(theme_query("/theme set to"), Some("to"));
        assert_eq!(theme_query("/theme"), None);
        assert_eq!(theme_query("/theme a b"), None);
        let mut themes = ThemeCompletion::default();
        assert!(themes.wants_listing("/theme "));
        assert!(!themes.wants_listing("/theme t"), "asked once");
        themes.forget();
        assert!(themes.wants_listing("/theme "));
    }

    #[test]
    fn matches_start_before_they_contain() {
        let themes = ready(&[
            "catppuccin-mocha",
            "rose-pine",
            "tokyonight-night",
            "nightfox",
        ]);
        let Some(ThemePopup::Entries(entries)) = themes.popup("/theme night") else {
            panic!("expected entries");
        };
        let names: Vec<&str> = entries.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["nightfox", "tokyonight-night"]);
        assert_eq!(
            themes.popup("/theme zzz"),
            Some(ThemePopup::Message("no theme matches `zzz`".into()))
        );
    }

    #[test]
    fn keys_preview_complete_and_save() {
        let popup = ThemePopup::Entries(vec![choice("a"), choice("b")]);
        let key = |code| Key::plain(code);
        assert_eq!(
            theme_key(&key(KeyCode::Down), &popup, 0),
            ThemeKey::Preview(1)
        );
        assert_eq!(
            theme_key(&key(KeyCode::Up), &popup, 0),
            ThemeKey::Preview(1)
        );
        assert_eq!(
            theme_key(&key(KeyCode::Tab), &popup, 1),
            ThemeKey::SetText("/theme set b".into())
        );
        assert_eq!(
            theme_key(&key(KeyCode::Enter), &popup, 0),
            ThemeKey::Run("theme set a".into())
        );
        assert_eq!(theme_key(&key(KeyCode::Esc), &popup, 0), ThemeKey::Hide);
    }
}
