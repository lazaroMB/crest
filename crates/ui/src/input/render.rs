//! Drawing the input: the editor, the tray, the popups and the which-key hint.

use gpui::{
    App, Context, Focusable as _, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Window, canvas, div, px,
};
use gpui_component::input::Textarea;
use gpui_component::v_flex;
use gpui_component::{ActiveTheme as _, h_flex};

use super::completion::palette_top;
use super::cursor::{CursorShape, HiddenCaret, paint_cursor};
use super::popups::Completion;
use super::tray::paste_handler;
use super::{IMAGE_MODE_ACTION, InputView, model_popup, path_popup, theme_popup};

impl Render for InputView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mode_keys = self
            .image_mode_keys
            .get_or_insert_with(|| {
                crate::help::action_keys(self.sequencer.keymap(), IMAGE_MODE_ACTION)
            })
            .clone();
        let tray = self.tray.render(mode_keys.as_deref(), cx);
        let which_key = self.render_which_key(cx);
        let completion = self.completion(cx).map(|shown| {
            // The list may have arrived or shrunk since the last key.
            let len = shown.len();
            if len > 0 {
                let selected = self.palette_selected.min(len - 1);
                self.palette_top = palette_top(selected, self.palette_top, len);
            }
            let (selected, top) = (self.palette_selected, self.palette_top);
            match shown {
                Completion::Path(popup) => path_popup::render(popup, selected, top, cx),
                Completion::Model(popup) => {
                    model_popup::render(popup, selected, top, self.current_model.as_ref(), cx)
                }
                Completion::Theme(popup) => {
                    theme_popup::render(popup, selected, top, &self.current_theme, cx)
                }
            }
        });
        let palette = if completion.is_none() {
            let total = self.palette_len(cx);
            if total > 0 {
                let selected = self.palette_selected.min(total - 1);
                self.palette_top = palette_top(selected, self.palette_top, total);
            }
            self.render_palette(cx)
        } else {
            None
        };
        let focused = self.owns_focus(window, cx);
        let theme = cx.theme();
        let (block, inverted, muted) = (theme.foreground, theme.background, theme.muted_foreground);
        let prompt_color = if focused { theme.primary } else { muted };
        let shape = self.blink.visible().then_some(CursorShape::Block);
        let editor = self.editor.clone();
        // Painted after the textarea, from the caret it laid out this frame.
        let cursor = canvas(
            |_, _, _| {},
            move |_, (), window, cx| {
                let focused = editor.read(cx).focus_handle(cx).is_focused(window);
                if let Some(shape) = shape.filter(|_| focused) {
                    paint_cursor(&editor, shape, block, inverted, window, cx);
                }
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();
        let prompt = h_flex()
            .items_start()
            .child(
                div()
                    .pt_2()
                    .text_color(prompt_color)
                    .font_weight(gpui::FontWeight::BOLD)
                    .child("❯"),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .child(HiddenCaret::new(
                        Textarea::new(&self.editor)
                            .appearance(false)
                            .on_paste(paste_handler(cx.entity().downgrade())),
                        self.editor.clone(),
                    ))
                    .child(cursor),
            );
        let pending = self.pending_keys();
        let status = (!pending.is_empty()).then(|| {
            h_flex()
                .gap_2()
                .text_xs()
                .text_color(muted)
                .child(SharedString::from(pending))
        });
        v_flex()
            .id("crest-input")
            .key_context("CrestInput")
            .track_focus(&self.focus_handle)
            // A click anywhere else closes the popups.
            .on_mouse_down_out(cx.listener(|this, _: &gpui::MouseDownEvent, _, cx| {
                this.hide_popups(cx);
            }))
            .font_family(theme.mono_font_family.clone())
            .child(
                // The wheel over a popup moves its window of rows.
                v_flex()
                    .id("input-popups")
                    .debug_selector(|| "input-popups".to_owned())
                    .on_scroll_wheel(cx.listener(Self::popup_wheel))
                    .children(completion)
                    .children(palette)
                    .children(which_key),
            )
            .children(tray)
            .child(prompt)
            .children(status)
    }
}

impl InputView {
    /// The keys that can follow the pending prefix (the leader), for the
    /// which-key popup. Empty when nothing is pending.
    pub fn which_key(&self) -> Vec<crate::help::NextKey> {
        let pending = self.sequencer.pending();
        if pending.is_empty() {
            return Vec::new();
        }
        crate::help::next_keys(self.keymap(), pending)
    }

    /// The which-key popup: while a prefix waits for its next key, what
    /// each next key does. It goes away with the prefix (a key, `Esc`, or
    /// `timeoutlen`).
    fn render_which_key(&self, cx: &App) -> Option<gpui::AnyElement> {
        let next = self.which_key();
        if next.is_empty() {
            return None;
        }
        let theme = cx.theme();
        let pending = crate::help::keys_label(self.sequencer.pending());
        let cells = next.into_iter().map(|next| {
            h_flex()
                .w(px(300.))
                .gap_2()
                .px_2()
                .child(
                    div()
                        .w(px(64.))
                        .flex_none()
                        .text_color(theme.primary)
                        .child(SharedString::from(next.key)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(theme.muted_foreground)
                        .child(SharedString::from(next.what)),
                )
        });
        Some(
            v_flex()
                .id("which-key")
                .debug_selector(|| "which-key".to_owned())
                .mb_1()
                .py_1()
                .border_1()
                .border_color(theme.border)
                .rounded_md()
                .bg(theme.popover)
                .text_sm()
                .child(h_flex().flex_wrap().children(cells))
                .child(
                    div()
                        .px_2()
                        .pt_1()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(SharedString::from(format!(
                            "{pending} … · a key runs it · Esc or a pause cancels"
                        ))),
                )
                .into_any_element(),
        )
    }
}
