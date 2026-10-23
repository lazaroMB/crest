//! The `/model ` popup of the prompt: while the line is `/model ` (or
//! `/model set `) and a word, it lists the models of the providers with a
//! key that match the word, as the `@` popup lists paths. The session hands
//! it the models once it has listed them ([`ModelCompletion::set`]); the
//! [`InputView`] feeds it the text and the keys and applies the result.
//!
//! `Enter` saves the chosen model (`/model set <id>`), `Tab` completes the
//! line to it, and `Esc` hides the popup, after which `Enter` sends the line
//! as typed (`/model opus`: the model of the next turn only).

use domain::model::ModelId;
use gpui::{
    AnyElement, App, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    Styled as _, div,
};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};

use super::PALETTE_ROWS;
use crate::session::ModelChoice;

/// What the popup shows for the current line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelPopup {
    /// The matching models, best first.
    Entries(Vec<ModelChoice>),
    /// Why there is nothing to choose (no match, still listing, an error).
    Message(String),
}

/// What a key pressed while the popup shows asks the input to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelKey {
    /// Choose the entry at this index.
    Select(usize),
    /// Hide the popup until the text changes.
    Hide,
    /// Replace the prompt with this text.
    SetText(String),
    /// Run this command (without the slash).
    Run(String),
    /// Not the popup's key.
    Ignored,
}

/// The models, as far as they are known.
#[derive(Debug, Default)]
enum Listing {
    #[default]
    NotAsked,
    Loading,
    Ready(Vec<ModelChoice>),
    Failed(String),
}

/// The models the popup offers.
#[derive(Debug, Default)]
pub struct ModelCompletion {
    listing: Listing,
}

/// The word after `/model ` (or `/model set `) in `text`, if the line is
/// that command and the word is still being typed.
#[must_use]
pub fn model_query(text: &str) -> Option<&str> {
    let rest = text.strip_prefix("/model ")?;
    let rest = rest.strip_prefix("set ").unwrap_or(rest);
    (!rest.contains(char::is_whitespace)).then_some(rest)
}

/// The choices matching `query`, best first: ids or aliases starting with
/// it (also after the provider prefix), then those containing it.
#[must_use]
pub fn model_matches<'a>(choices: &'a [ModelChoice], query: &str) -> Vec<&'a ModelChoice> {
    let query = query.to_lowercase();
    let mut starts = Vec::new();
    let mut contains = Vec::new();
    for choice in choices {
        let id = choice.model.as_str().to_lowercase();
        let bare = id.split_once(':').map_or(id.as_str(), |(_, bare)| bare);
        let alias = choice.alias.to_lowercase();
        if id.starts_with(&query) || bare.starts_with(&query) || alias.starts_with(&query) {
            starts.push(choice);
        } else if id.contains(&query) || alias.contains(&query) {
            contains.push(choice);
        }
    }
    starts.append(&mut contains);
    starts
}

impl ModelCompletion {
    /// Whether `text` opens the popup and the models were never asked for;
    /// it then counts as asked, so the caller lists them once.
    pub fn wants_listing(&mut self, text: &str) -> bool {
        if model_query(text).is_none() || !matches!(self.listing, Listing::NotAsked) {
            return false;
        }
        self.listing = Listing::Loading;
        true
    }

    /// The models of the providers, or why they could not be listed. A model
    /// of the default provider comes twice (with and without its prefix):
    /// only the prefixed one stays, which says where it runs.
    pub fn set(&mut self, models: Result<Vec<ModelChoice>, String>) {
        self.listing = match models {
            Ok(models) => {
                let prefixed: Vec<ModelId> = models
                    .iter()
                    .filter(|c| c.model.provider().is_some())
                    .map(|c| c.model.clone())
                    .collect();
                let twice = |c: &ModelChoice| {
                    c.model.provider().is_none()
                        && prefixed.iter().any(|p| {
                            p.provider()
                                .is_some_and(|(_, bare)| bare == c.model.as_str())
                        })
                };
                Listing::Ready(models.into_iter().filter(|c| !twice(c)).collect())
            }
            Err(error) => Listing::Failed(error),
        };
    }

    /// What the popup shows for `text`, if it is a `/model ` line.
    #[must_use]
    pub fn popup(&self, text: &str) -> Option<ModelPopup> {
        let query = model_query(text)?;
        Some(match &self.listing {
            Listing::NotAsked | Listing::Loading => {
                ModelPopup::Message("listing the models of your providers…".into())
            }
            Listing::Failed(error) => ModelPopup::Message(format!("cannot list models: {error}")),
            Listing::Ready(models) if models.is_empty() => ModelPopup::Message(
                "your providers offer no model this account can use; /login to add a key".into(),
            ),
            Listing::Ready(models) => {
                let matches: Vec<ModelChoice> =
                    model_matches(models, query).into_iter().cloned().collect();
                if matches.is_empty() {
                    ModelPopup::Message(format!("no model matches `{query}`"))
                } else {
                    ModelPopup::Entries(matches)
                }
            }
        })
    }
}

/// What `key` does in `popup` for the line `text`, with entry `selected`
/// chosen.
#[must_use]
pub fn model_key(key: &application::keymap::Key, popup: &ModelPopup, selected: usize) -> ModelKey {
    use application::keymap::KeyCode;
    let entries = match popup {
        ModelPopup::Entries(entries) => entries,
        ModelPopup::Message(_) if key.code == KeyCode::Esc => return ModelKey::Hide,
        ModelPopup::Message(_) => return ModelKey::Ignored,
    };
    let len = entries.len().min(PALETTE_ROWS);
    let selected = selected.min(len - 1);
    let id = entries[selected].model.as_str();
    match key.code {
        KeyCode::Down => ModelKey::Select((selected + 1) % len),
        KeyCode::Up => ModelKey::Select((selected + len - 1) % len),
        KeyCode::Esc => ModelKey::Hide,
        KeyCode::Tab => ModelKey::SetText(format!("/model set {id}")),
        KeyCode::Enter => ModelKey::Run(format!("model set {id}")),
        _ => ModelKey::Ignored,
    }
}

/// Draws `popup` with entry `selected` highlighted; `current` is the model
/// the next turn uses, marked in the list.
pub fn render(
    popup: ModelPopup,
    selected: usize,
    current: Option<&ModelId>,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let (rows, more) = match popup {
        ModelPopup::Entries(entries) => {
            let selected = selected.min(entries.len().min(PALETTE_ROWS) - 1);
            let more = entries.len().saturating_sub(PALETTE_ROWS);
            let rows: Vec<_> = entries
                .into_iter()
                .take(PALETTE_ROWS)
                .enumerate()
                .map(|(ix, choice)| {
                    let marker = if current == Some(&choice.model) {
                        "●"
                    } else {
                        " "
                    };
                    let (provider, name) = match choice.model.provider() {
                        Some((provider, bare)) => (provider.to_string(), bare.to_owned()),
                        None => (String::new(), choice.model.as_str().to_owned()),
                    };
                    let alias = (choice.alias != choice.model.as_str()).then_some(choice.alias);
                    let mut row = h_flex()
                        .id(("model", ix))
                        .gap_2()
                        .px_2()
                        .child(div().text_color(theme.primary).child(marker))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_color(theme.foreground)
                                .child(SharedString::from(match alias {
                                    Some(alias) => format!("{alias}  {name}"),
                                    None => name,
                                })),
                        )
                        .child(
                            div()
                                .text_color(theme.muted_foreground)
                                .child(SharedString::from(provider)),
                        );
                    if ix == selected {
                        row = row.bg(theme.list_active).rounded_sm();
                    }
                    row.into_any_element()
                })
                .collect();
            (rows, more)
        }
        ModelPopup::Message(message) => (
            vec![
                div()
                    .px_2()
                    .text_color(theme.muted_foreground)
                    .child(SharedString::from(message))
                    .into_any_element(),
            ],
            0,
        ),
    };
    let mut footer =
        "↑↓ choose · Enter save as your model · Tab complete · Esc hide (Enter: next turn only)"
            .to_owned();
    if more > 0 {
        footer.push_str(&format!(" · {more} more, keep typing"));
    }
    v_flex()
        .id("model-palette")
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

    fn choice(id: &str) -> ModelChoice {
        ModelChoice {
            alias: id.into(),
            model: ModelId::new(id),
        }
    }

    fn ready(ids: &[&str]) -> ModelCompletion {
        let mut models = ModelCompletion::default();
        models.set(Ok(ids.iter().copied().map(choice).collect()));
        models
    }

    #[test]
    fn only_a_model_line_opens_it() {
        assert_eq!(model_query("/model "), Some(""));
        assert_eq!(model_query("/model gl"), Some("gl"));
        assert_eq!(
            model_query("/model set opencode-zen:g"),
            Some("opencode-zen:g")
        );
        assert_eq!(model_query("/model"), None);
        assert_eq!(model_query("/mode "), None);
        assert_eq!(model_query("/model set a b"), None);
        let mut models = ModelCompletion::default();
        assert!(!models.wants_listing("hola"));
        assert!(models.wants_listing("/model "));
        assert!(!models.wants_listing("/model g"), "asked once");
    }

    #[test]
    fn matches_start_before_they_contain_and_the_bare_twin_goes() {
        let models = ready(&[
            "glm-5.3",
            "opencode-go:glm-5.3",
            "opencode-zen:kimi-k3",
            "opencode-zen:big-glm",
        ]);
        let Some(ModelPopup::Entries(entries)) = models.popup("/model gl") else {
            panic!("expected entries");
        };
        let ids: Vec<&str> = entries.iter().map(|c| c.model.as_str()).collect();
        assert_eq!(ids, ["opencode-go:glm-5.3", "opencode-zen:big-glm"]);
        assert_eq!(
            models.popup("/model xyz"),
            Some(ModelPopup::Message("no model matches `xyz`".into()))
        );
        assert!(matches!(
            ModelCompletion::default().popup("/model "),
            Some(ModelPopup::Message(_))
        ));
    }

    #[test]
    fn keys_choose_complete_and_save() {
        let popup = ModelPopup::Entries(vec![choice("opencode-zen:a"), choice("anthropic:b")]);
        let key = |code| Key::plain(code);
        assert_eq!(
            model_key(&key(KeyCode::Down), &popup, 0),
            ModelKey::Select(1)
        );
        assert_eq!(model_key(&key(KeyCode::Up), &popup, 0), ModelKey::Select(1));
        assert_eq!(
            model_key(&key(KeyCode::Tab), &popup, 1),
            ModelKey::SetText("/model set anthropic:b".into())
        );
        assert_eq!(
            model_key(&key(KeyCode::Enter), &popup, 0),
            ModelKey::Run("model set opencode-zen:a".into())
        );
        assert_eq!(model_key(&key(KeyCode::Esc), &popup, 0), ModelKey::Hide);
        let message = ModelPopup::Message("listing…".into());
        assert_eq!(
            model_key(&key(KeyCode::Enter), &message, 0),
            ModelKey::Ignored
        );
        assert_eq!(model_key(&key(KeyCode::Esc), &message, 0), ModelKey::Hide);
    }
}
