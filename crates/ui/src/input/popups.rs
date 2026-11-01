//! The completion popups of the input: `@` paths, `/model` and `/theme`.

use std::sync::Arc;

use application_core::ports::search::FileSearchPort;
use domain::model::ModelId;
use gpui::{App, Context, ScrollWheelEvent, Window, px};

use super::completion::{PALETTE_ROWS, Popup, scroll_window};
use super::{InputEvent, InputView, ModelPopup, PathPopup, ThemePopup, WHEEL_ROW};
use crate::session::{ModelChoice, ThemeChoice};

/// The completion popup the input shows.
pub(super) enum Completion {
    /// `@` paths.
    Path(PathPopup),
    /// `/model ` models.
    Model(ModelPopup),
    /// `/theme ` themes.
    Theme(ThemePopup),
}

impl Completion {
    /// Entries to choose from.
    pub(super) fn len(&self) -> usize {
        match self {
            Self::Path(popup) => popup.len(),
            Self::Model(popup) => popup.len(),
            Self::Theme(popup) => popup.len(),
        }
    }
}

impl InputView {
    /// Lists the directories `@` words name; without it they get no
    /// suggestions.
    pub fn set_file_search(&mut self, files: Arc<dyn FileSearchPort>) {
        self.paths.set_file_search(files);
    }

    /// What the `@` popup shows for the current text, if anything.
    pub fn path_popup(&self, cx: &App) -> Option<PathPopup> {
        if self.palette_hidden {
            return None;
        }
        self.paths.popup(&self.shared_text(cx))
    }

    /// Hands the `/model ` popup the models of the providers, or why they
    /// could not be listed.
    pub fn set_models(&mut self, models: Result<Vec<ModelChoice>, String>, cx: &mut Context<Self>) {
        self.models.set(models);
        cx.notify();
    }

    /// The model the next turn uses, marked in the `/model ` popup.
    pub fn set_current_model(&mut self, model: Option<ModelId>) {
        self.current_model = model;
    }

    /// Hands the `/theme ` popup the themes, or why they could not be
    /// listed.
    pub fn set_themes(&mut self, themes: Result<Vec<ThemeChoice>, String>, cx: &mut Context<Self>) {
        self.themes.set(themes);
        cx.notify();
    }

    /// The theme in use, marked in the `/theme ` popup; the themes are
    /// listed again the next time it opens.
    pub fn set_current_theme(&mut self, name: &str) {
        self.current_theme = name.to_owned();
        self.themes.forget();
    }

    /// What the `/theme ` popup shows for the current text, if anything.
    pub fn theme_popup(&self, cx: &App) -> Option<ThemePopup> {
        if self.palette_hidden {
            return None;
        }
        self.themes.popup(&self.shared_text(cx))
    }

    pub(super) fn end_theme_preview(&mut self, cx: &mut Context<Self>) {
        if std::mem::take(&mut self.theme_previewing) {
            cx.emit(InputEvent::ThemePreviewEnd);
        }
    }

    /// What the `/model ` popup shows for the current text, if anything.
    pub fn model_popup(&self, cx: &App) -> Option<ModelPopup> {
        if self.palette_hidden {
            return None;
        }
        self.models.popup(&self.shared_text(cx))
    }

    /// The completion popup the input shows, if any: the `@` one first,
    /// then `/model `, then `/theme ` (the same one `render` picks).
    pub(super) fn completion(&self, cx: &App) -> Option<Completion> {
        if let Some(popup) = self.path_popup(cx) {
            return Some(Completion::Path(popup));
        }
        if let Some(popup) = self.model_popup(cx) {
            return Some(Completion::Model(popup));
        }
        self.theme_popup(cx).map(Completion::Theme)
    }

    /// Entries of the popup the input shows, the same one `render` picks.
    fn popup_len(&self, cx: &App) -> usize {
        match self.completion(cx) {
            Some(shown) => shown.len(),
            None => self.palette(cx).len(),
        }
    }

    /// The wheel over a popup scrolls its rows, carrying the chosen one
    /// along when it would leave the window (the `/theme ` popup previews
    /// it).
    pub(super) fn popup_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let len = self.popup_len(cx);
        if len <= PALETTE_ROWS {
            return;
        }
        cx.stop_propagation();
        // Up is a positive delta; small touchpad deltas add up to a row.
        self.wheel_rest -= f32::from(event.delta.pixel_delta(px(WHEEL_ROW)).y);
        let rows = (self.wheel_rest / WHEEL_ROW).trunc();
        if rows == 0.0 {
            return;
        }
        self.wheel_rest -= rows * WHEEL_ROW;
        let before = self.palette_selected.min(len - 1);
        let (top, selected) = scroll_window(self.palette_top, before, rows as isize, len);
        self.palette_top = top;
        self.palette_selected = selected;
        if selected != before
            && let Some(Completion::Theme(Popup::Entries(entries))) = self.completion(cx)
            && let Some(choice) = entries.get(selected)
        {
            self.theme_previewing = true;
            cx.emit(InputEvent::ThemePreview(choice.name.clone()));
        }
        cx.notify();
    }
}
