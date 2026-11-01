//! The `/theme` popup: its list, previews and `/theme set`.

use application::ports::events::NoticeLevel;
use gpui::{Context, Window};

use super::Session;

impl Session {
    /// Puts `/theme ` in the prompt, whose popup lists the themes.
    pub fn open_theme_popup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |input, cx| {
            input.set_text("/theme ", window, cx);
            input.focus(window, cx);
        });
        cx.notify();
    }

    /// Resolves the themes and hands them to the input's popup.
    pub(super) fn load_themes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let listing = self.controller.themes();
        cx.spawn_in(window, async move |this, cx| {
            let result = listing.await;
            this.update(cx, |this, cx| {
                if let Ok(themes) = &result {
                    this.theme_choices.clone_from(themes);
                }
                this.input
                    .update(cx, |input, cx| input.set_themes(result, cx));
            })
            .ok();
        })
        .detach();
    }

    /// Saves `name` as the theme and shows it; a theme that cannot be
    /// saved or resolved leaves the one in use.
    pub(super) fn save_theme(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let save = self.controller.save_theme(name);
        cx.spawn_in(window, async move |this, cx| {
            let result = save.await;
            this.update(cx, |this, cx| match result {
                Ok(saved) => {
                    this.theme = saved.theme;
                    crate::app::set_theme(&this.theme, cx);
                    let name = this.theme.name.clone();
                    this.input
                        .update(cx, |input, _| input.set_current_theme(&name));
                    this.notice(NoticeLevel::Info, saved.notice, cx);
                }
                Err(error) => {
                    crate::app::set_theme(&this.theme, cx);
                    this.notice(NoticeLevel::Warning, format!("/theme: {error}"), cx);
                }
            })
            .ok();
        })
        .detach();
    }
}
