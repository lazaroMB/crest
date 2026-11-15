//! A turn summary is a run of thinking, tool calls and their diffs already
//! in the timeline. The view draws one chip in front of the run; the blocks
//! stay where they are.

use domain::block::{BlockKind, ToolStatus};

use super::TimelineEntry;

/// A contiguous run of thinking and tool calls, indexes into the timeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    /// First block of the run.
    pub start: usize,
    /// One past the last block of the run.
    pub end: usize,
}

/// Tools of a run that finished, and tools that failed, were denied or
/// were cancelled. A tool still running counts in neither.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Counts {
    /// [`ToolStatus::Succeeded`].
    pub done: u32,
    /// [`ToolStatus::Failed`], [`ToolStatus::Denied`] or [`ToolStatus::Cancelled`].
    pub failed: u32,
}

/// Whether a block belongs in a turn summary. A diff rides with the tool
/// that produced it. Prompts, replies and every other kind stay on their
/// own row.
pub fn is_trace(kind: &BlockKind) -> bool {
    matches!(
        kind,
        BlockKind::Thinking { .. } | BlockKind::ToolCall { .. } | BlockKind::Diff(_)
    )
}

/// The run that contains `ix`, when that block is thinking, a tool call or
/// a diff.
#[must_use]
pub fn span_at(entries: &[TimelineEntry], ix: usize) -> Option<Span> {
    if !entries
        .get(ix)
        .is_some_and(|entry| is_trace(&entry.block.kind))
    {
        return None;
    }
    let mut start = ix;
    while start > 0 && is_trace(&entries[start - 1].block.kind) {
        start -= 1;
    }
    let mut end = ix + 1;
    while entries
        .get(end)
        .is_some_and(|entry| is_trace(&entry.block.kind))
    {
        end += 1;
    }
    Some(Span { start, end })
}

/// The +1 and −1 of `span`.
#[must_use]
pub fn counts(entries: &[TimelineEntry], span: Span) -> Counts {
    let mut counts = Counts::default();
    for entry in &entries[span.start..span.end] {
        let BlockKind::ToolCall { status, .. } = entry.block.kind else {
            continue;
        };
        match status {
            ToolStatus::Succeeded => counts.done += 1,
            ToolStatus::Failed | ToolStatus::Denied | ToolStatus::Cancelled => counts.failed += 1,
            ToolStatus::Pending | ToolStatus::AwaitingApproval | ToolStatus::Running => {}
        }
    }
    counts
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::block::{Block, BlockKind};
    use serde_json::json;

    fn thinking() -> TimelineEntry {
        TimelineEntry::new(Block::new(BlockKind::Thinking {
            text: "hmm".into(),
            collapsed: true,
        }))
    }

    fn tool(status: ToolStatus) -> TimelineEntry {
        TimelineEntry::new(Block::new(BlockKind::ToolCall {
            tool_use_id: "toolu".into(),
            name: "git".into(),
            summary: String::new(),
            input: json!({}),
            status,
            output: None,
        }))
    }

    fn text(markdown: &str) -> TimelineEntry {
        TimelineEntry::new(Block::new(BlockKind::Text {
            markdown: markdown.into(),
        }))
    }

    #[test]
    fn a_run_of_thinking_and_tools_is_one_span() {
        let entries = vec![
            text("> hola"),
            thinking(),
            tool(ToolStatus::Succeeded),
            tool(ToolStatus::Denied),
            tool(ToolStatus::Running),
            text("listo"),
        ];
        assert!(span_at(&entries, 0).is_none());
        assert!(span_at(&entries, 5).is_none());
        let span = span_at(&entries, 3).unwrap();
        assert_eq!(span, Span { start: 1, end: 5 });
        assert_eq!(counts(&entries, span), Counts { done: 1, failed: 1 });
        assert_eq!(span_at(&entries, 1).unwrap().start, 1);
    }

    #[test]
    fn a_diff_stays_inside_the_tool_run() {
        let entries = vec![
            thinking(),
            tool(ToolStatus::Succeeded),
            TimelineEntry::new(Block::new(BlockKind::Diff(domain::block::DiffSpec {
                path: ".gitignore".into(),
                old_text: None,
                new_text: String::new(),
                unified: "--- a/.gitignore\n+++ b/.gitignore\n".into(),
                approval: None,
            }))),
            text("listo"),
        ];
        let span = span_at(&entries, 2).unwrap();
        assert_eq!(span, Span { start: 0, end: 3 });
        assert_eq!(counts(&entries, span), Counts { done: 1, failed: 0 });
    }

    #[test]
    fn a_reply_between_tools_splits_the_runs() {
        let entries = vec![
            tool(ToolStatus::Succeeded),
            text("mid"),
            tool(ToolStatus::Failed),
        ];
        assert_eq!(span_at(&entries, 0).unwrap().end, 1);
        assert_eq!(span_at(&entries, 2).unwrap().start, 2);
    }
}
