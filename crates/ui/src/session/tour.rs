//! Opening the welcome tour and answering its buttons.

use gpui::{Context, Window};

use super::Session;
use crate::session::SessionEvent;

impl Session {
    /// The step on screen, if the tour is open.
    pub fn tour_step(&self) -> Option<crate::tour::Step> {
        self.tour
    }

    /// Opens the welcome screen. A tour already open goes back to it
    /// instead of stacking another one.
    pub fn open_tour(&mut self, cx: &mut Context<Self>) {
        self.tour = Some(crate::tour::Step::Welcome);
        cx.emit(SessionEvent::Tour);
        cx.notify();
    }

    /// Applies `action` and, when it closes the tour, remembers the version.
    ///
    /// The commands step types `/` into the prompt so the existing command
    /// list opens. Leaving that step puts the previous text back.
    pub fn tour_act(
        &mut self,
        action: crate::tour::Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(step) = self.tour else {
            return;
        };
        if crate::tour::remembers(action) && self.tutorial_seen != Some(crate::tour::VERSION) {
            self.tutorial_seen = Some(crate::tour::VERSION);
            let controller = std::sync::Arc::clone(&self.controller);
            cx.spawn(async move |_, _| {
                let _ = controller.save_tutorial(crate::tour::VERSION).await;
            })
            .detach();
        }
        let next = crate::tour::after(step, action);
        self.sync_tour_prompt(step, next, window, cx);
        self.tour = next;
        cx.emit(SessionEvent::Tour);
        cx.notify();
    }

    /// Shows `/` for the commands step and restores the prompt afterwards.
    fn sync_tour_prompt(
        &mut self,
        step: crate::tour::Step,
        next: Option<crate::tour::Step>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let showing = next == Some(crate::tour::Step::Commands);
        let was = step == crate::tour::Step::Commands;
        if showing && !was {
            self.tour_saved_prompt = Some(self.input.read(cx).text(cx));
            self.input
                .update(cx, |input, cx| input.set_text("/", window, cx));
        } else if was && !showing {
            let saved = self.tour_saved_prompt.take().unwrap_or_default();
            self.input
                .update(cx, |input, cx| input.set_text(&saved, window, cx));
        }
    }
}
