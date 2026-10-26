//! The "N numbers not from an engine" chip under an assistant reply: the
//! numeric literals the lint found outside number slots and the user's
//! prompts ([`domain::computed::lint`]). Clicking it lists them.

use domain::block::BlockId;
use gpui::{
    AnyElement, App, Context, ElementId, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};

/// The chip of one block; open shows the list.
pub struct UnverifiedChip {
    id: BlockId,
    numbers: Vec<String>,
    open: bool,
}

impl UnverifiedChip {
    /// Whether the list is shown.
    pub fn is_open(&self) -> bool {
        self.open
    }
}

/// The chip's label.
pub fn label(count: usize) -> String {
    format!(
        "⚠ {count} number{} not from an engine",
        if count == 1 { "" } else { "s" }
    )
}

impl Render for UnverifiedChip {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let chip = div()
            .id(ElementId::Name(SharedString::from(format!(
                "unverified-{}",
                self.id
            ))))
            .debug_selector(|| "unverified-chip".to_owned())
            .px_1()
            .rounded_sm()
            .text_xs()
            .bg(theme.warning.opacity(0.12))
            .text_color(theme.warning)
            .cursor_pointer()
            .child(SharedString::from(label(self.numbers.len())))
            .on_click(cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                this.open = !this.open;
                cx.notify();
            }));
        let mut column = v_flex().gap_0p5().child(h_flex().child(chip));
        if self.open {
            column = column.child(div().text_xs().text_color(theme.muted_foreground).child(
                SharedString::from(format!(
                    "Typed by the model, not computed: {}. Ask it to compute them with a \
                         tool and cite them as slots.",
                    self.numbers.join(", ")
                )),
            ));
        }
        column
    }
}

/// The chip of block `id` listing `numbers`.
pub fn render(id: BlockId, numbers: &[String], window: &mut Window, cx: &mut App) -> AnyElement {
    let chip = crate::blocks::block_state(id, window, cx, |_, _| UnverifiedChip {
        id,
        numbers: numbers.to_vec(),
        open: false,
    });
    chip.update(cx, |chip, cx| {
        if chip.numbers != numbers {
            chip.numbers = numbers.to_vec();
            cx.notify();
        }
    });
    chip.into_any_element()
}

#[cfg(test)]
mod tests {
    use application::ports::events::DomainEvent;
    use domain::block::{Block, BlockKind};
    use domain::conversation::{StopReason, TurnId};
    use gpui::{Modifiers, TestAppContext, px};

    use super::*;
    use crate::timeline::TimelineView;

    #[test]
    fn the_label_counts_the_numbers() {
        assert_eq!(label(1), "⚠ 1 number not from an engine");
        assert_eq!(label(3), "⚠ 3 numbers not from an engine");
    }

    #[gpui::test]
    fn a_finished_reply_with_typed_numbers_shows_the_chip(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, cx| TimelineView::new(cx));
        let reply = Block::new(BlockKind::Text {
            markdown: "The mean is 12.7 and the SD 3.42.".into(),
        });
        let id = reply.id;
        view.update(cx, |view, cx| {
            view.apply(&DomainEvent::BlockAdded { block: reply }, cx);
            view.apply(
                &DomainEvent::TurnFinished {
                    turn_id: TurnId::new(),
                    stop_reason: StopReason::EndTurn,
                },
                cx,
            );
        });
        let drawn = view.clone();
        cx.draw(
            gpui::point(px(0.), px(0.)),
            gpui::size(px(800.), px(600.)),
            move |_, _| div().size_full().child(drawn.clone()),
        );
        let bounds = cx.debug_bounds("unverified-chip").expect("chip drawn");
        cx.simulate_click(bounds.center(), Modifiers::none());
        let chip = cx
            .update(|_, cx| crate::blocks::existing_block_state::<UnverifiedChip>(id, cx))
            .expect("chip state");
        chip.read_with(cx, |chip, _| {
            assert!(chip.is_open());
            assert_eq!(chip.numbers, ["12.7", "3.42"]);
        });
    }
}
