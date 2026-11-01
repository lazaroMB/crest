//! The permission mode, the model override and the `/model` and `/mode`
//! popups.

use application::ports::events::{DomainEvent, NoticeLevel};
use domain::model::ModelId;
use domain::permission::PermissionMode;
use gpui::{Context, Window};

use super::{ModelChoice, Session, SessionEvent};

/// The palette rows `/mode ` lists: one per permission mode.
pub(super) fn mode_rows() -> Vec<crate::help::CommandRow> {
    PermissionMode::ALL
        .iter()
        .map(|mode| crate::help::CommandRow {
            name: format!("mode {mode}"),
            usage: format!("/mode {mode}"),
            description: format!("Set the permission mode to {mode}"),
            source: crate::help::BUILTIN_SOURCE.into(),
        })
        .collect()
}

/// The aliases whose model is in `catalog` (an alias of a provider without
/// a key is no choice), then the catalog.
fn with_aliases(aliases: Vec<ModelChoice>, catalog: Vec<ModelChoice>) -> Vec<ModelChoice> {
    let served = |model: &ModelId| catalog.iter().any(|c| &c.model == model);
    let mut models: Vec<ModelChoice> = aliases
        .into_iter()
        .filter(|a| a.alias != a.model.as_str() && served(&a.model))
        .collect();
    models.extend(catalog);
    models
}

impl Session {
    /// Sets the permission mode and tells the status bar.
    pub fn set_permission_mode(&mut self, mode: PermissionMode, cx: &mut Context<Self>) {
        self.controller.set_permission_mode(mode);
        self.dispatch(DomainEvent::ModeChanged { mode }, cx);
    }

    /// `<leader>p`.
    pub fn cycle_permission_mode(&mut self, cx: &mut Context<Self>) {
        let mode = self.controller.permission_mode().next();
        self.set_permission_mode(mode, cx);
    }

    /// Sets (or clears) the override of the next turn.
    pub fn set_model_override(&mut self, model: Option<String>, cx: &mut Context<Self>) {
        let message = match &model {
            Some(model) => format!("next turn with `{model}`"),
            None => "next turn with the default model".to_owned(),
        };
        self.model_override = model;
        self.notice(NoticeLevel::Info, message, cx);
    }

    /// Puts `/model ` in the prompt, whose popup lists the models (the
    /// input asks for them with [`InputEvent::ModelsWanted`]).
    pub fn open_model_popup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |input, cx| {
            input.set_text("/model ", window, cx);
            input.focus(window, cx);
        });
        cx.notify();
    }

    /// Puts `/mode ` in the prompt, whose palette lists the permission
    /// modes.
    pub fn open_mode_popup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |input, cx| {
            input.set_text("/mode ", window, cx);
            input.focus(window, cx);
        });
        cx.notify();
    }

    /// Lists the providers' models once per session and hands them to the
    /// input's popup, after the aliases whose model a provider serves.
    pub(super) fn load_catalog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(models) = self.catalog_models.clone() {
            self.input
                .update(cx, |input, cx| input.set_models(Ok(models), cx));
            return;
        }
        let listing = self.controller.catalog_models();
        let aliases = self.controller.models();
        cx.spawn_in(window, async move |this, cx| {
            let result = listing.await.map(|catalog| with_aliases(aliases, catalog));
            this.update(cx, |this, cx| {
                if let Ok(models) = &result {
                    this.catalog_models = Some(models.clone());
                }
                this.input
                    .update(cx, |input, cx| input.set_models(result, cx));
            })
            .ok();
        })
        .detach();
    }

    /// Saves `choice` as the selected model, then reopens the session so
    /// every role and flow uses it; if it cannot be saved, it is still the
    /// model of the next turn.
    pub(super) fn select_model(
        &mut self,
        choice: ModelChoice,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.idle_for("/model", cx) {
            self.set_model_override(Some(choice.alias), cx);
            return;
        }
        let select = self.controller.select_model(choice.model.clone());
        cx.spawn_in(window, async move |this, cx| {
            let result = select.await;
            this.update(cx, |this, cx| match result {
                Ok(message) => cx.emit(SessionEvent::Reopen {
                    notice: Some((NoticeLevel::Info, message)),
                }),
                Err(error) => {
                    this.set_model_override(Some(choice.alias), cx);
                    this.notice(NoticeLevel::Warning, error, cx);
                }
            })
            .ok();
        })
        .detach();
    }
}
