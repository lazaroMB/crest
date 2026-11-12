//! Renderer of the `agent` block kind: a subagent working in the
//! background (`/agent`, `&`, `task` or a forked skill sent there).
//!
//! `agent #3 · Find permission checks` with its kind and status, the job
//! it was given, its latest tool calls while it works, and its report once
//! it ends. Long prompts and reports show their first lines until the
//! block is expanded (`Enter` on the selected block, or a click).

use domain::block::{AgentStatus, BlockKind, BlockTag};
use gpui::{
    AnyElement, App, FontWeight, IntoElement as _, ParentElement as _, SharedString, Styled as _,
    Window, div,
};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};

use super::BlockView;
use super::copy::CopyFormat;
use super::shell::head;
use crate::timeline::{BlockWidth, TimelineEntry};

/// Lines of the prompt shown while the block is collapsed.
pub const PROMPT_LINES: usize = 3;
/// Lines of the report shown while the block is collapsed.
pub const REPORT_LINES: usize = 12;

/// The header: `agent #3 · name`.
#[must_use]
pub fn title(agent_id: u64, name: &str) -> String {
    format!("agent #{agent_id} · {name}")
}

/// What follows the title: the kind, who started it and the status.
#[must_use]
pub fn status_line(kind: &str, origin: &str, status: AgentStatus) -> String {
    let by = if origin == "user" { "you" } else { "the agent" };
    let status = match status {
        AgentStatus::Running => "running…",
        other => other.label(),
    };
    format!("{kind} · started by {by} · {status}")
}

/// `text` cut to `limit` lines, with a note of how many were left out.
fn cut(text: &str, limit: usize) -> String {
    let (shown, hidden) = head(text, limit);
    if hidden == 0 {
        shown
    } else {
        format!("{shown}\n… {hidden} more lines (Enter expands)")
    }
}

/// Draws the block held by `entry`.
pub fn render(entry: &TimelineEntry, _window: &mut Window, cx: &mut App) -> AnyElement {
    let BlockKind::Agent {
        agent_id,
        name,
        kind,
        origin,
        prompt,
        status,
        activity,
        report,
    } = &entry.block.kind
    else {
        return super::placeholder(entry, cx);
    };
    let theme = cx.theme();
    let expanded = entry.expanded;
    let status_color = match status {
        AgentStatus::Running => theme.primary,
        AgentStatus::Done => theme.success,
        AgentStatus::Failed => theme.danger,
        AgentStatus::Cancelled => theme.muted_foreground,
    };
    let header = h_flex()
        .gap_2()
        .child(
            div()
                .font_weight(FontWeight::BOLD)
                .text_color(theme.foreground)
                .child(SharedString::from(title(*agent_id, name))),
        )
        .child(
            div()
                .text_color(status_color)
                .child(SharedString::from(status_line(kind, origin, *status))),
        );
    let limit = |lines| if expanded { usize::MAX } else { lines };
    let mut body = v_flex()
        .gap_1()
        .pl_3()
        .border_l_2()
        .border_color(status_color);
    body = body.child(
        div()
            .text_color(theme.muted_foreground)
            .whitespace_normal()
            .child(SharedString::from(cut(prompt, limit(PROMPT_LINES)))),
    );
    // The tool calls while it works; once it ended, only when expanded.
    if !activity.is_empty() && (*status == AgentStatus::Running || expanded) {
        let lines = activity.join("\n");
        body = body.child(
            div()
                .font_family(theme.mono_font_family.clone())
                .text_color(theme.muted_foreground)
                .whitespace_normal()
                .child(SharedString::from(lines)),
        );
    }
    if let Some(report) = report {
        let color = if *status == AgentStatus::Failed {
            theme.danger
        } else {
            theme.foreground
        };
        body = body.child(
            div()
                .text_color(color)
                .whitespace_normal()
                .child(SharedString::from(cut(report, limit(REPORT_LINES)))),
        );
    }
    v_flex()
        .gap_1()
        .py_1()
        .child(header)
        .child(body)
        .into_any_element()
}

/// The registry entry of agent blocks ([`super::BlockView`]).
pub struct View;

impl BlockView for View {
    fn tag(&self) -> BlockTag {
        BlockTag::Agent
    }

    fn render(&self, entry: &TimelineEntry, window: &mut Window, cx: &mut App) -> AnyElement {
        render(entry, window, cx)
    }

    fn width(&self) -> BlockWidth {
        BlockWidth::Prose
    }

    fn expanded_by_default(&self, _kind: &BlockKind) -> bool {
        false
    }

    fn copy_formats(&self, kind: &BlockKind) -> Vec<CopyFormat> {
        match kind {
            BlockKind::Agent {
                report: Some(report),
                ..
            } => vec![CopyFormat::new(
                "report",
                "the agent's report",
                report.clone(),
            )],
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_and_status() {
        assert_eq!(title(3, "Find x"), "agent #3 · Find x");
        assert_eq!(
            status_line("explore", "user", AgentStatus::Running),
            "explore · started by you · running…"
        );
        assert_eq!(
            status_line("general", "model", AgentStatus::Done),
            "general · started by the agent · done"
        );
        assert_eq!(cut("a\nb\nc", 2), "a\nb\n… 1 more lines (Enter expands)");
        assert_eq!(cut("a", 2), "a");
    }

    #[gpui::test]
    fn draws_a_running_and_a_finished_agent(cx: &mut gpui::TestAppContext) {
        use application_core::ports::events::DomainEvent;
        use domain::block::Block;

        use crate::timeline::TimelineView;

        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, cx| TimelineView::new(cx));
        let running = Block::new(BlockKind::Agent {
            agent_id: 1,
            name: "Find permission checks".into(),
            kind: "explore".into(),
            origin: "user".into(),
            prompt: "Find where permissions are checked".into(),
            status: AgentStatus::Running,
            activity: vec!["grep \"decide\" ✓".into(), "read_file src/lib.rs …".into()],
            report: None,
        });
        let done = Block::new(BlockKind::Agent {
            agent_id: 2,
            name: "Review".into(),
            kind: "general".into(),
            origin: "model".into(),
            prompt: "Review".into(),
            status: AgentStatus::Done,
            activity: Vec::new(),
            report: Some("All good.".into()),
        });
        view.update(cx, |view, cx| {
            view.apply_all(
                [
                    DomainEvent::BlockAdded { block: running },
                    DomainEvent::BlockAdded { block: done },
                ],
                cx,
            );
        });
        cx.draw(
            gpui::point(gpui::px(0.), gpui::px(0.)),
            gpui::size(gpui::px(800.), gpui::px(600.)),
            |_, _| div().size_full().child(view.clone()),
        );
        cx.run_until_parked();
    }
}
