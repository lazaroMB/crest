//! Drawing the prompt mode control: a round button before the text field,
//! the chip of the mode (outside chat) and the menu the button opens.

use gpui::{
    AnyElement, Context, InteractiveElement as _, IntoElement as _, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, div, px,
};
use gpui_component::{ActiveTheme as _, h_flex};

use super::InputView;
use super::completion::{active, frame, hoverable};
use super::mode::{ChatMode, MENU, MenuEntry};
use super::palette::{muted_cell, small_note};
use crate::kit::{ChipKind, chip};

/// Width of the round button, in pixels.
pub const BUTTON_PX: f32 = 26.0;
/// Width of the chip's `×`, in pixels.
const CLOSE_PX: f32 = 14.0;
/// Space between the button and the chip, in pixels.
const GAP_PX: f32 = 4.0;
/// The most the button and the chip take together (NFR-2 of R02).
pub const CONTROL_MAX_PX: f32 = 160.0;
/// The most the chip takes: what is left of [`CONTROL_MAX_PX`]. A longer
/// label is cut off, so no mode can push the control past its limit.
pub const CHIP_MAX_PX: f32 = CONTROL_MAX_PX - BUTTON_PX - GAP_PX;

// The button, the gap and the chip fit the limit whatever the mode, and the
// `×` fits the chip.
const _: () = assert!(BUTTON_PX + GAP_PX + CHIP_MAX_PX <= CONTROL_MAX_PX && CLOSE_PX < CHIP_MAX_PX);

/// The row of the menu where the inserts start.
const INSERTS_AT: usize = 3;
/// The row of the menu where the manager is.
const MANAGE_AT: usize = 5;

impl InputView {
    /// The round button and, outside chat, the chip. Together they never
    /// take more than [`CONTROL_MAX_PX`].
    pub(super) fn render_mode_control(&self, cx: &mut Context<Self>) -> AnyElement {
        let mode = self.mode_control.mode;
        h_flex()
            .id("prompt-mode-control")
            .debug_selector(|| "prompt-mode-control".to_owned())
            .flex_none()
            .max_w(px(CONTROL_MAX_PX))
            .mt_1()
            .mr_2()
            .gap(px(GAP_PX))
            .child(self.mode_button(cx))
            .children((mode != ChatMode::Chat).then(|| mode_chip(mode, cx)))
            .into_any_element()
    }

    /// The `+` button: it opens the menu, or closes it.
    fn mode_button(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (accent, foreground) = (theme.primary, theme.foreground);
        let open = self.mode_control.menu.is_some();
        round_cell("prompt-mode-button", BUTTON_PX)
            .border_1()
            .border_color(if open { accent } else { theme.border })
            .text_color(if open {
                foreground
            } else {
                theme.muted_foreground
            })
            .hover(move |style| style.text_color(foreground).border_color(accent))
            .child("+")
            .on_click(cx.listener(|this, _, window, cx| {
                this.mode_control.toggle();
                this.focus(window, cx);
                cx.notify();
            }))
            .into_any_element()
    }

    /// The menu above the prompt, while it is open.
    pub(super) fn render_mode_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let selected = self.mode_control.menu?;
        let (muted, border) = (cx.theme().muted_foreground, cx.theme().border);
        let section = |title: &'static str| {
            div()
                .px_2()
                .text_xs()
                .text_color(muted)
                .child(title)
                .into_any_element()
        };
        let view = cx.entity().downgrade();
        let mut rows = vec![section("Mode")];
        for (ix, entry) in MENU.into_iter().enumerate() {
            if ix == INSERTS_AT {
                rows.push(div().h_px().my_1().bg(border).into_any_element());
                rows.push(section("Add"));
            }
            if ix == MANAGE_AT {
                rows.push(div().h_px().my_1().bg(border).into_any_element());
                rows.push(section("Manage"));
            }
            let on = entry == MenuEntry::Mode(self.mode_control.mode);
            let row = menu_row(ix, entry, on, cx);
            let row = hoverable(row, ix, &view);
            rows.push(if ix == selected {
                active(row, cx.theme()).into_any_element()
            } else {
                row.into_any_element()
            });
        }
        let footer = "↑↓ choose · Enter pick · Esc close".to_owned();
        let menu = frame("mode-menu", rows, footer, cx.theme());
        Some(h_flex().child(menu).into_any_element())
    }
}

/// A round clickable cell of `size` pixels with its text centered: the
/// button and the chip's `×`; `id` is also its debug selector.
fn round_cell(id: &'static str, size: f32) -> gpui::Stateful<gpui::Div> {
    div()
        .id(SharedString::from(id))
        .debug_selector(|| id.to_owned())
        .flex_none()
        .size(px(size))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .cursor_pointer()
}

/// `chip` as a pill that holds its label and a `×` on one line and never
/// grows past [`CHIP_MAX_PX`].
fn pill(chip: gpui::Div) -> gpui::Stateful<gpui::Div> {
    chip.id("prompt-mode-chip")
        .debug_selector(|| "prompt-mode-chip".to_owned())
        .flex()
        .items_center()
        .gap_1()
        .max_w(px(CHIP_MAX_PX))
        .overflow_hidden()
        .whitespace_nowrap()
        .rounded_full()
}

/// The chip of `mode`: its name and a `×` that goes back to chat.
fn mode_chip(mode: ChatMode, cx: &mut Context<InputView>) -> AnyElement {
    let accent = cx.theme().primary;
    let close = round_cell("prompt-mode-clear", CLOSE_PX)
        .hover(move |style| style.bg(accent.opacity(0.25)))
        .child("×")
        .on_click(cx.listener(|this, _, window, cx| {
            this.focus(window, cx);
            this.set_chat_mode(ChatMode::Chat, window, cx);
        }));
    pill(chip(mode.label(), accent, ChipKind::Soft, cx))
        .child(close)
        .into_any_element()
}

/// Row `ix` of the menu; `on` says whether it is the mode in use (a check).
fn menu_row(
    ix: usize,
    entry: MenuEntry,
    on: bool,
    cx: &mut Context<InputView>,
) -> gpui::Stateful<gpui::Div> {
    let theme = cx.theme();
    let accent = theme.primary;
    let name_color = if on { accent } else { theme.foreground };
    let (name, hint) = entry.text();
    h_flex()
        .id(("mode-menu", ix))
        .debug_selector(move || format!("mode-menu-{ix}"))
        .gap_3()
        .px_2()
        .cursor_pointer()
        .child(div().text_color(name_color).child(name))
        .child(muted_cell(hint.to_owned(), theme))
        .child(small_note(if on { "✓" } else { " " }.to_owned(), theme))
        .on_click(cx.listener(move |this, _, window, cx| {
            this.pick_entry(entry, window, cx);
        }))
}
