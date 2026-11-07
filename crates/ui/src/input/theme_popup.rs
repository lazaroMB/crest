//! The `/theme ` popup of the prompt: while the line is `/theme ` and a
//! word, it lists the themes whose name matches the word, as the `/model `
//! popup lists models. The session hands it the themes once it has
//! resolved them ([`ThemeCompletion::set`]).
//!
//! Moving through the list previews each theme in the window
//! ([`ThemeKey::Select`]); `Enter` saves the chosen one (`/theme set
//! <name>`), `Tab` completes the line to it and `Esc` hides the popup and
//! goes back to the theme in use.

use gpui::{AnyElement, ParentElement as _, SharedString, Styled as _, div};
use gpui_component::h_flex;

use super::completion::{self, Look, Nav, Paint, Popup, PopupKey, nav_key};
use crate::session::ThemeChoice;

/// What the popup shows for the current line: the matching themes, by
/// name, or why there is nothing to choose (no match, still loading, an
/// error).
pub type ThemePopup = Popup<ThemeChoice>;

/// What a key pressed while the popup shows asks the input to do; choosing
/// an entry ([`PopupKey::Select`]) previews its theme, and hiding the popup
/// goes back to the theme in use.
pub type ThemeKey = PopupKey;

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

/// What `key` does in `popup` with entry `selected` chosen: `Tab`
/// completes the line to the chosen theme and `Enter` saves it.
#[must_use]
pub fn theme_key(
    key: &application_core::keymap::Key,
    popup: &ThemePopup,
    selected: usize,
) -> ThemeKey {
    match nav_key(key, popup, selected) {
        Nav::Select(ix) => ThemeKey::Select(ix),
        Nav::Hide => ThemeKey::Hide,
        Nav::Complete(choice) => ThemeKey::SetText(format!("/theme set {}", choice.name)),
        Nav::Accept(choice) => ThemeKey::Run(format!("theme set {}", choice.name)),
        Nav::Ignored => ThemeKey::Ignored,
    }
}

/// Draws `popup` from entry `top` on, with entry `selected` highlighted;
/// `current` is the theme in use, marked in the list.
pub(super) fn render(popup: ThemePopup, paint: &Paint, current: &str) -> AnyElement {
    let theme = paint.theme;
    completion::render(
        &Look {
            id: "theme-palette",
            row_id: "theme",
            keys: "↑↓ preview · Enter save · Tab complete · Esc back",
        },
        popup,
        paint,
        |choice| {
            let marker = if choice.name == current { "●" } else { " " };
            // A swatch of the theme itself: background, accent, text.
            let swatch = |path: &str| {
                div()
                    .size_3()
                    .rounded_sm()
                    .bg(crate::theme::hsla(choice.theme.color(path)))
            };
            h_flex()
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
                .child(
                    div()
                        .text_color(theme.muted_foreground)
                        .child(SharedString::from(format!(
                            "{} · {}",
                            choice.theme.appearance.name(),
                            choice.origin
                        ))),
                )
        },
    )
}

#[cfg(test)]
mod tests {
    use application_core::keymap::{Key, KeyCode};

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
            ThemeKey::Select(1)
        );
        assert_eq!(theme_key(&key(KeyCode::Up), &popup, 0), ThemeKey::Select(1));
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
