//! Renderer of the `markdown` block kind: a document (tool `show_markdown`
//! or a written `.md`) with its title above it.
//!
//! The document view is gpui-component's markdown `TextView` (headings,
//! GFM tables, code blocks highlighted with Tree-sitter, TeX math through
//! [`super::math`]). `t` switches to
//! the source, drawn as one fenced `markdown` code block so the same
//! highlighter colors the raw text.

use domain::block::BlockKind;
use gpui::{
    AnyElement, App, FontWeight, IntoElement as _, ParentElement as _, SharedString, Styled as _,
    Window, div,
};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};

use crate::timeline::TimelineEntry;

/// Wraps `source` in a fenced `markdown` code block whose fence is longer
/// than any backtick run inside it, so the source shows verbatim.
pub fn fenced_source(source: &str) -> String {
    super::custom::fenced_code(Some("markdown"), source)
}

/// Draws the block held by `entry`.
pub fn render(entry: &TimelineEntry, window: &mut Window, cx: &mut App) -> AnyElement {
    let BlockKind::Markdown { title, source } = &entry.block.kind else {
        return super::placeholder(entry, cx);
    };
    let id = entry.block.id;
    // Both views keep their parse across frames (`block_state`): a long
    // document scrolled out of the timeline and back is not parsed again.
    let body = if entry.alt_view {
        super::math::code_block(id, Some("markdown"), source, window, cx)
    } else {
        super::math::markdown_block(id, source, window, cx)
    };
    let theme = cx.theme();
    let (mode, hint) = if entry.alt_view {
        ("source", "t: document")
    } else {
        ("document", "t: source")
    };
    let title = title.as_deref().filter(|t| !t.trim().is_empty());
    let header = h_flex()
        .gap_2()
        .pb_1()
        .border_b_1()
        .border_color(theme.border)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.foreground)
                .child(SharedString::from(title.unwrap_or("Markdown").to_owned())),
        )
        .child(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(format!("{mode} · {hint}")),
        );
    v_flex()
        .gap_2()
        .px_3()
        .py_2()
        .rounded(theme.radius)
        .border_1()
        .border_color(theme.border)
        .bg(theme.secondary)
        .child(header)
        .child(body)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::keymap::Action;
    use application::ports::events::DomainEvent;
    use domain::block::Block;

    use crate::timeline::TimelineView;
    use gpui::{AppContext as _, TestAppContext, point, px, size};
    use gpui_component::highlighter::LanguageRegistry;

    const DOC: &str = "# crest\n\n## Tabla\n\n| ext | bytes |\n| --- | ---: |\n| rs | 120 |\n\n\
                       ```rust\nfn main() {}\n```\n";

    #[test]
    fn fence_outgrows_inner_backticks() {
        assert_eq!(fenced_source("# a"), "```markdown\n# a\n```\n");
        let wrapped = fenced_source(DOC);
        assert!(wrapped.starts_with("````markdown\n"));
        assert!(wrapped.ends_with("\n````\n"));
        assert!(fenced_source("x ````` y").starts_with("``````markdown"));
    }

    #[test]
    fn markdown_and_code_grammars_are_registered() {
        let registry = LanguageRegistry::singleton();
        for lang in ["markdown", "rust", "bash", "json", "toml"] {
            assert!(registry.language(lang).is_some(), "{lang} missing");
        }
    }

    #[gpui::test]
    fn document_parses_headings_tables_and_code(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let state = cx.new(|cx| gpui_component::text::TextViewState::markdown(DOC, cx));
        cx.run_until_parked();
        let text = state.read_with(cx, |state, _| state.rendered_text().as_str().to_owned());
        for shown in ["crest", "Tabla", "ext", "120", "fn main() {}"] {
            assert!(text.contains(shown), "{shown:?} missing from {text:?}");
        }
        for markup in ["# ", "| ---", "```"] {
            assert!(
                !text.contains(markup),
                "{markup:?} left as text in {text:?}"
            );
        }
    }

    #[gpui::test]
    fn draws_document_and_source_in_the_timeline(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, cx| TimelineView::new(cx));
        let block = Block::new(BlockKind::Markdown {
            title: Some("README.md".into()),
            source: DOC.into(),
        });
        view.update(cx, |view, cx| {
            view.apply(&DomainEvent::BlockAdded { block }, cx);
            view.handle_action(&Action::builtin("block.last"), cx);
        });
        for alt_view in [false, true] {
            cx.draw(point(px(0.), px(0.)), size(px(700.), px(500.)), |_, _| {
                div().size_full().child(view.clone())
            });
            cx.run_until_parked();
            view.update(cx, |view, cx| {
                assert_eq!(view.state().entries()[0].alt_view, alt_view);
                view.handle_action(&Action::builtin("block.toggle_view"), cx);
            });
        }
    }
}
