//! The line a branched chat starts with, above its blocks.

use gpui::{
    App, Div, InteractiveElement as _, ParentElement as _, SharedString, Stateful, Styled as _, div,
};
use gpui_component::ActiveTheme as _;

/// The banner of a chat: nothing for a chat that is not a branch.
#[derive(Debug, Default)]
pub(crate) struct Banner(Option<String>);

impl Banner {
    /// Sets the line (or takes it away); whether it changed.
    pub(crate) fn set(&mut self, text: Option<String>) -> bool {
        let changed = self.0 != text;
        self.0 = text;
        changed
    }

    /// The line as an element above the blocks, if there is one.
    pub(crate) fn element(&self, cx: &App) -> Option<Stateful<Div>> {
        let text = self.0.clone()?;
        let theme = cx.theme();
        Some(
            div()
                .id("timeline-branch")
                .debug_selector(|| "timeline-branch".to_owned())
                .flex_none()
                .px_4()
                .py_1()
                .border_b_1()
                .border_color(theme.border)
                .bg(theme.muted)
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(SharedString::from(text)),
        )
    }
}

#[cfg(test)]
mod tests {
    use application_core::ports::events::DomainEvent;
    use domain::block::{Block, BlockKind};
    use gpui::{TestAppContext, VisualTestContext};

    use crate::timeline::TimelineView;

    /// The banner shows above the blocks, with and without any, takes its
    /// own room and goes away.
    #[gpui::test]
    fn a_branch_starts_with_its_banner_above_the_blocks(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, cx| TimelineView::new(cx));
        let draw = |cx: &mut VisualTestContext| {
            cx.update(|window, _| window.refresh());
            cx.run_until_parked();
        };
        let set = |banner: Option<&str>, cx: &mut VisualTestContext| {
            view.update(cx, |view, cx| {
                assert!(view.banner.set(banner.map(str::to_owned)));
                cx.notify();
            });
            draw(cx);
        };
        let shown = |cx: &mut VisualTestContext| cx.debug_bounds("timeline-branch");
        // (blocks, banner, shown) in the order they happen.
        let steps = [
            (false, Some("Branched from \"Fix\" after 0 blocks."), true),
            (true, None, false),
        ];
        for (blocks, banner, want) in steps {
            if blocks {
                view.update(cx, |view, cx| {
                    let block = Block::new(BlockKind::Text {
                        markdown: "hola".into(),
                    });
                    view.apply_all([DomainEvent::BlockAdded { block }], cx);
                });
                set(Some("Branched from \"Fix\" after 1 block."), cx);
                let banner = shown(cx).expect("above the blocks");
                let body = cx.debug_bounds("timeline-body").expect("the blocks");
                assert!(banner.bottom() <= body.top());
            }
            set(banner, cx);
            assert_eq!(shown(cx).is_some(), want, "{banner:?}");
        }
    }
}
