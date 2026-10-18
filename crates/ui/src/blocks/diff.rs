//! Renderer of the `diff` block kind: a header with the path and the
//! approval state, then the unified diff with one color per line kind.
//!
//! [`render_unified`] is shared with the approval dialog, so the user sees
//! the same colored diff in the timeline and when deciding.

use domain::block::{ApprovalState, BlockKind, DiffSpec};
use gpui::{
    AnyElement, App, FontWeight, Hsla, IntoElement as _, ParentElement as _, SharedString,
    Styled as _, div,
};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};

use crate::timeline::TimelineEntry;

/// What a line of a unified diff is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffLineKind {
    /// `--- a/…`, `+++ b/…`, `diff …`, `index …`: file headers.
    FileHeader,
    /// `@@ -1,3 +1,4 @@`: start of a hunk.
    Hunk,
    /// `+…`: an added line.
    Added,
    /// `-…`: a removed line.
    Removed,
    /// Anything else: unchanged context or `\ No newline at end of file`.
    Context,
}

/// Classifies one line (without its newline) of a unified diff.
pub fn classify_line(line: &str) -> DiffLineKind {
    if line.starts_with("+++ ")
        || line.starts_with("--- ")
        || line.starts_with("diff ")
        || line.starts_with("index ")
    {
        DiffLineKind::FileHeader
    } else if line.starts_with("@@") {
        DiffLineKind::Hunk
    } else if line.starts_with('+') {
        DiffLineKind::Added
    } else if line.starts_with('-') {
        DiffLineKind::Removed
    } else {
        DiffLineKind::Context
    }
}

/// Counts of added and removed lines, for the header.
pub fn line_stats(unified: &str) -> (usize, usize) {
    unified
        .lines()
        .fold((0, 0), |(added, removed), line| match classify_line(line) {
            DiffLineKind::Added => (added + 1, removed),
            DiffLineKind::Removed => (added, removed + 1),
            _ => (added, removed),
        })
}

/// Short label of an approval state.
pub fn approval_label(state: ApprovalState) -> &'static str {
    match state {
        ApprovalState::Pending => "awaiting approval",
        ApprovalState::Approved => "approved",
        ApprovalState::Rejected => "rejected",
    }
}

fn approval_color(state: ApprovalState, cx: &App) -> Hsla {
    let theme = cx.theme();
    match state {
        ApprovalState::Pending => theme.warning,
        ApprovalState::Approved => theme.success,
        ApprovalState::Rejected => theme.danger,
    }
}

fn line_color(kind: DiffLineKind, cx: &App) -> Hsla {
    let theme = cx.theme();
    match kind {
        DiffLineKind::FileHeader => theme.muted_foreground,
        DiffLineKind::Hunk => theme.info,
        DiffLineKind::Added => theme.success,
        DiffLineKind::Removed => theme.danger,
        DiffLineKind::Context => theme.foreground,
    }
}

/// The unified diff, one colored row per line. An empty diff shows a muted
/// "no changes" line.
pub fn render_unified(unified: &str, cx: &App) -> AnyElement {
    let theme = cx.theme();
    if unified.trim().is_empty() {
        return div()
            .text_color(theme.muted_foreground)
            .child("(no changes)")
            .into_any_element();
    }
    let added_bg = theme.success.opacity(0.12);
    let removed_bg = theme.danger.opacity(0.12);
    v_flex()
        .font_family(theme.mono_font_family.clone())
        .children(unified.lines().map(|line| {
            let kind = classify_line(line);
            // An empty context line still takes one row.
            let text = if line.is_empty() { " " } else { line };
            let row = div()
                .px_1()
                .whitespace_nowrap()
                .text_color(line_color(kind, cx))
                .child(SharedString::from(text.to_owned()));
            match kind {
                DiffLineKind::Added => row.bg(added_bg),
                DiffLineKind::Removed => row.bg(removed_bg),
                _ => row,
            }
        }))
        .into_any_element()
}

/// Header of a diff: path, `+a −r`, "new file" and the approval state.
pub fn render_header(spec: &DiffSpec, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let (added, removed) = line_stats(&spec.unified);
    let mut header = h_flex()
        .gap_2()
        .child(
            div()
                .font_weight(FontWeight::BOLD)
                .child(SharedString::from(spec.path.clone())),
        )
        .child(
            div()
                .text_color(theme.success)
                .child(SharedString::from(format!("+{added}"))),
        )
        .child(
            div()
                .text_color(theme.danger)
                .child(SharedString::from(format!("−{removed}"))),
        );
    if spec.old_text.is_none() {
        header = header.child(div().text_color(theme.muted_foreground).child("new file"));
    }
    if let Some(state) = spec.approval {
        header = header.child(
            div()
                .text_color(approval_color(state, cx))
                .child(approval_label(state)),
        );
    }
    header.into_any_element()
}

/// Draws the block held by `entry`: the header, and the diff while the
/// block is expanded.
pub fn render(entry: &TimelineEntry, cx: &App) -> AnyElement {
    let BlockKind::Diff(spec) = &entry.block.kind else {
        return super::placeholder(entry, cx);
    };
    let mut block = v_flex().gap_1().child(render_header(spec, cx));
    if entry.expanded {
        block = block.child(
            div()
                .border_l_2()
                .border_color(cx.theme().border)
                .child(render_unified(&spec.unified, cx)),
        );
    }
    block.into_any_element()
}

#[cfg(test)]
mod tests {
    use application::ports::events::DomainEvent;
    use domain::block::Block;
    use gpui::{TestAppContext, px};

    use super::*;
    use crate::timeline::TimelineView;

    const DIFF: &str = "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,2 +1,3 @@\n fn a() {}\n-fn b() {}\n+fn c() {}\n+fn d() {}\n";

    #[test]
    fn classifies_each_line_kind() {
        let kinds: Vec<DiffLineKind> = DIFF.lines().map(classify_line).collect();
        assert_eq!(
            kinds,
            [
                DiffLineKind::FileHeader,
                DiffLineKind::FileHeader,
                DiffLineKind::Hunk,
                DiffLineKind::Context,
                DiffLineKind::Removed,
                DiffLineKind::Added,
                DiffLineKind::Added,
            ]
        );
    }

    #[test]
    fn content_that_looks_like_a_header_is_a_change() {
        // Headers have a space after the marker; `---x` removes `--x`.
        assert_eq!(classify_line("---x"), DiffLineKind::Removed);
        assert_eq!(classify_line("+++"), DiffLineKind::Added);
        assert_eq!(classify_line(""), DiffLineKind::Context);
    }

    #[test]
    fn stats_skip_headers() {
        assert_eq!(line_stats(DIFF), (2, 1));
        assert_eq!(line_stats(""), (0, 0));
    }

    #[test]
    fn labels_every_approval_state() {
        assert_eq!(approval_label(ApprovalState::Pending), "awaiting approval");
        assert_eq!(approval_label(ApprovalState::Approved), "approved");
        assert_eq!(approval_label(ApprovalState::Rejected), "rejected");
    }

    #[gpui::test]
    fn draws_diffs_in_every_state(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, cx| TimelineView::new(cx));
        let diff = |unified: &str, old: Option<&str>, approval| {
            Block::new(BlockKind::Diff(DiffSpec {
                path: "src/lib.rs".into(),
                old_text: old.map(str::to_owned),
                new_text: "fn c() {}\n".into(),
                unified: unified.into(),
                approval,
            }))
        };
        let blocks = [
            diff(DIFF, Some("fn b() {}\n"), Some(ApprovalState::Pending)),
            diff("", Some(""), None),
            diff("+fn c() {}\n", None, Some(ApprovalState::Rejected)),
        ];
        view.update(cx, |view, cx| {
            view.apply_all(blocks.map(|block| DomainEvent::BlockAdded { block }), cx);
        });
        cx.draw(
            gpui::point(px(0.), px(0.)),
            gpui::size(px(800.), px(600.)),
            |_, _| div().size_full().child(view.clone()),
        );
        view.read_with(cx, |view, _| assert_eq!(view.state().len(), 3));
    }
}
