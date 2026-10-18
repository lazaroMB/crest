//! Renderer of the `stage_header` block kind: a separator that marks the
//! workflow entering a new stage, `Stage 5/14 · Hypothesis`, over a
//! progress bar.

use domain::block::BlockKind;
use gpui::{
    AnyElement, App, FontWeight, IntoElement as _, ParentElement as _, SharedString, Styled as _,
    Window, div, px, relative,
};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};

use crate::timeline::TimelineEntry;

/// The header line: `Stage 5/14 · name`.
pub fn label(stage: &str, index: u32, total: u32) -> String {
    format!("Stage {index}/{total} · {stage}")
}

/// Fraction of the workflow reached at stage `index` of `total`, in
/// `0.0..=1.0`. An empty workflow counts as complete.
pub fn progress(index: u32, total: u32) -> f32 {
    if total == 0 {
        return 1.0;
    }
    (index.min(total) as f32) / (total as f32)
}

/// Draws the block held by `entry`.
pub fn render(entry: &TimelineEntry, _window: &mut Window, cx: &mut App) -> AnyElement {
    let BlockKind::StageHeader {
        stage,
        index,
        total,
    } = &entry.block.kind
    else {
        return super::placeholder(entry, cx);
    };
    let theme = cx.theme();
    let rule = || div().flex_1().h(px(1.)).bg(theme.border);
    let header = h_flex()
        .gap_2()
        .child(rule())
        .child(
            div()
                .font_weight(FontWeight::BOLD)
                .text_color(theme.foreground)
                .child(SharedString::from(label(stage, *index, *total))),
        )
        .child(rule());
    let bar = div()
        .w_full()
        .h(px(4.))
        .rounded_full()
        .bg(theme.muted)
        .child(
            div()
                .h_full()
                .rounded_full()
                .bg(theme.primary)
                .w(relative(progress(*index, *total))),
        );
    v_flex()
        .py_1()
        .gap_1()
        .child(header)
        .child(bar)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use application::ports::events::DomainEvent;
    use domain::block::Block;
    use gpui::TestAppContext;

    use super::*;
    use crate::timeline::TimelineView;

    #[test]
    fn label_shows_position_and_name() {
        assert_eq!(label("Hipótesis", 5, 14), "Stage 5/14 · Hipótesis");
    }

    #[test]
    fn progress_is_clamped_and_total_safe() {
        assert!((progress(7, 14) - 0.5).abs() < f32::EPSILON);
        assert!((progress(20, 14) - 1.0).abs() < f32::EPSILON);
        assert!((progress(0, 0) - 1.0).abs() < f32::EPSILON);
    }

    #[gpui::test]
    fn draws_a_stage_header(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, cx| TimelineView::new(cx));
        let block = Block::new(BlockKind::StageHeader {
            stage: "Hipótesis".into(),
            index: 5,
            total: 14,
        });
        view.update(cx, |view, cx| {
            view.apply(&DomainEvent::BlockAdded { block }, cx);
        });
        cx.draw(
            gpui::point(px(0.), px(0.)),
            gpui::size(px(800.), px(600.)),
            |_, _| div().size_full().child(view.clone()),
        );
        view.read_with(cx, |view, _| assert_eq!(view.state().len(), 1));
    }
}
