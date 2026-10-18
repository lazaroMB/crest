//! Renderer of the `thinking` block kind.
//!
//! Collapsed (the default, from the block's `collapsed` flag through
//! [`TimelineEntry::expanded`]) it is one muted line: `Thinking…` plus the
//! first line of the reasoning. Expanded (`Enter` or a click) it shows the
//! whole text in a muted tone under the same header.

use domain::block::BlockKind;
use gpui::{
    AnyElement, App, FontWeight, IntoElement as _, ParentElement as _, SharedString, Styled as _,
    div,
};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};

use crate::timeline::TimelineEntry;

/// Header shown in both states.
const LABEL: &str = "Thinking…";
/// Characters of the first line kept in the collapsed summary; the ellipsis
/// cuts the rest anyway, so shaping a huge line would be wasted work.
const SUMMARY_CHARS: usize = 160;

/// First non-blank line of `text`, trimmed and cut to [`SUMMARY_CHARS`].
pub fn summary_line(text: &str) -> &str {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    match line.char_indices().nth(SUMMARY_CHARS) {
        Some((end, _)) => &line[..end],
        None => line,
    }
}

/// Draws the block held by `entry`.
pub fn render(entry: &TimelineEntry, cx: &App) -> AnyElement {
    let BlockKind::Thinking { text, .. } = &entry.block.kind else {
        return super::placeholder(entry, cx);
    };
    let theme = cx.theme();
    let marker = if entry.expanded { "▾" } else { "▸" };
    let mut header = h_flex()
        .gap_1()
        .text_color(theme.muted_foreground)
        .child(marker)
        .child(div().font_weight(FontWeight::MEDIUM).italic().child(LABEL));
    if !entry.expanded {
        let summary = summary_line(text);
        if !summary.is_empty() {
            header = header.child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(SharedString::from(summary.to_owned())),
            );
        }
        return header.into_any_element();
    }
    v_flex()
        .gap_1()
        .child(header)
        .child(
            div()
                .ml_1()
                .pl_3()
                .border_l_1()
                .border_color(theme.border)
                .text_color(theme.muted_foreground)
                .child(SharedString::from(text.clone())),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::ports::events::DomainEvent;
    use domain::block::Block;
    use gpui::{TestAppContext, point, px, size};

    use crate::timeline::TimelineState;

    #[test]
    fn summary_is_first_non_blank_line() {
        assert_eq!(
            summary_line("\n  \n  Primero leo el archivo.\nLuego"),
            "Primero leo el archivo."
        );
        assert_eq!(summary_line(""), "");
        let long = "á".repeat(SUMMARY_CHARS + 20);
        assert_eq!(summary_line(&long).chars().count(), SUMMARY_CHARS);
    }

    fn thinking(collapsed: bool) -> Block {
        Block::new(BlockKind::Thinking {
            text: "Voy a revisar main.rs\ny luego los tests".into(),
            collapsed,
        })
    }

    fn state_with(blocks: impl IntoIterator<Item = Block>) -> TimelineState {
        let mut state = TimelineState::new();
        for block in blocks {
            state.apply(&DomainEvent::BlockAdded { block });
        }
        state
    }

    #[test]
    fn collapsed_by_default_unless_the_block_says_otherwise() {
        let state = state_with([thinking(true), thinking(false)]);
        assert!(!state.entries()[0].expanded);
        assert!(state.entries()[1].expanded);
    }

    #[gpui::test]
    fn draws_collapsed_and_expanded(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let state = state_with([thinking(true)]);
        let collapsed = state.entries()[0].clone();
        let mut expanded = collapsed.clone();
        expanded.expanded = true;
        let cx = cx.add_empty_window();
        for entry in [collapsed, expanded] {
            cx.draw(point(px(0.), px(0.)), size(px(600.), px(200.)), |_, cx| {
                render(&entry, cx)
            });
        }
    }
}
