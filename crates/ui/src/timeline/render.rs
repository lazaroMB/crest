//! How each block kind is drawn in the timeline.
//!
//! `Text`, `ToolCall` and `Error` have their own look (a `Text` block shows
//! its number slots resolved and, under a finished reply, the chip of
//! numbers not from an engine); every other kind goes to
//! [`crate::blocks::render`].

use domain::block::{BlockId, BlockKind, ToolStatus};
use gpui::{
    AnyElement, App, ElementId, FontWeight, Hsla, IntoElement, ParentElement as _, SharedString,
    Styled as _, Window, div,
};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};

use super::TimelineEntry;

/// Serde tag of a block kind (`"text"`, `"tool_call"`...), as shown in the
/// placeholder of kinds without a view yet.
pub fn kind_name(kind: &BlockKind) -> &'static str {
    match kind {
        BlockKind::Text { .. } => "text",
        BlockKind::Thinking { .. } => "thinking",
        BlockKind::ToolCall { .. } => "tool_call",
        BlockKind::Table(_) => "table",
        BlockKind::Chart(_) => "chart",
        BlockKind::Diff(_) => "diff",
        BlockKind::Mermaid(_) => "mermaid",
        BlockKind::Markdown { .. } => "markdown",
        BlockKind::Custom { .. } => "custom",
        BlockKind::Sources { .. } => "sources",
        BlockKind::Image { .. } => "image",
        BlockKind::StageHeader { .. } => "stage_header",
        BlockKind::Shell { .. } => "shell",
        BlockKind::Error { .. } => "error",
        BlockKind::Computed { .. } => "computed",
        BlockKind::Form(_) => "form",
        BlockKind::Plot(_) => "plot",
        BlockKind::TestReport(_) => "test_report",
        BlockKind::Findings(_) => "findings",
        BlockKind::Dataset(_) => "dataset",
        BlockKind::Molecule(_) => "molecule",
        BlockKind::Plate(_) => "plate",
        BlockKind::Trace(_) => "trace",
        BlockKind::FlameGraph(_) => "flame_graph",
        BlockKind::Logs(_) => "logs",
        BlockKind::QueryPlan(_) => "query_plan",
        BlockKind::Har(_) => "har",
        BlockKind::TerraformPlan(_) => "terraform_plan",
        BlockKind::Diagram(_) => "diagram",
        BlockKind::Benchmarks(_) => "benchmarks",
        BlockKind::Waveform(_) => "waveform",
        BlockKind::Netlist(_) => "netlist",
        BlockKind::Material(_) => "material",
        BlockKind::CadModel(_) => "cad_model",
        BlockKind::Drawing(_) => "drawing",
        BlockKind::IfcSchedule(_) => "ifc_schedule",
        BlockKind::Sequence(_) => "sequence",
        BlockKind::Reaction(_) => "reaction",
        BlockKind::MoleculeGrid(_) => "molecule_grid",
        BlockKind::Spectrum(_) => "spectrum",
        BlockKind::Molecule3d(_) => "molecule_3d",
        BlockKind::Ghs(_) => "ghs",
    }
}

/// Short label of a tool status.
pub fn status_label(status: ToolStatus) -> &'static str {
    match status {
        ToolStatus::Pending => "pending",
        ToolStatus::AwaitingApproval => "awaiting approval",
        ToolStatus::Running => "running",
        ToolStatus::Succeeded => "done",
        ToolStatus::Failed => "failed",
        ToolStatus::Denied => "denied",
        ToolStatus::Cancelled => "cancelled",
    }
}

/// The text recorded when a call was denied, shown on the collapsed row.
/// Empty output is left out.
fn denial_note(status: ToolStatus, output: Option<&str>) -> Option<&str> {
    if status != ToolStatus::Denied {
        return None;
    }
    output.map(str::trim).filter(|text| !text.is_empty())
}

/// One-line summary of a tool call: `name · key arguments · status`. The
/// arguments part is left out when the summary is empty.
pub fn tool_call_line(name: &str, summary: &str, status: ToolStatus) -> String {
    let summary = summary.trim();
    if summary.is_empty() {
        format!("{name} · {}", status_label(status))
    } else {
        format!("{name} · {summary} · {}", status_label(status))
    }
}

/// Plain-text content of a block, for `y` (yank).
pub fn copy_text(kind: &BlockKind) -> String {
    match kind {
        BlockKind::Text { markdown } => markdown.clone(),
        BlockKind::Thinking { text, .. } => text.clone(),
        BlockKind::ToolCall {
            name,
            summary,
            input,
            status,
            output,
            ..
        } => {
            let line = tool_call_line(name, summary, *status);
            let input = serde_json::to_string_pretty(input).unwrap_or_default();
            match output {
                Some(output) => format!("{line}\n{input}\n{output}"),
                None => format!("{line}\n{input}"),
            }
        }
        BlockKind::Error { message, hint } => match hint {
            Some(hint) => format!("{message}\n{hint}"),
            None => message.clone(),
        },
        BlockKind::Markdown { source, .. } => source.clone(),
        BlockKind::Shell {
            command,
            stdout,
            stderr,
            ..
        } => format!("$ {command}\n{stdout}{stderr}"),
        other => serde_json::to_string_pretty(other).unwrap_or_default(),
    }
}

fn status_color(status: ToolStatus, cx: &App) -> Hsla {
    let theme = cx.theme();
    match status {
        ToolStatus::Pending | ToolStatus::Cancelled => theme.muted_foreground,
        ToolStatus::AwaitingApproval => theme.warning,
        ToolStatus::Running => theme.info,
        ToolStatus::Succeeded => theme.success,
        ToolStatus::Failed | ToolStatus::Denied => theme.danger,
    }
}

fn element_id(prefix: &str, id: BlockId) -> ElementId {
    ElementId::Name(SharedString::from(format!("{prefix}-{id}")))
}

/// Draws the content of one block (without the selection frame).
pub fn render_block(entry: &TimelineEntry, window: &mut Window, cx: &mut App) -> AnyElement {
    let id = entry.block.id;
    match &entry.block.kind {
        // Streaming re-renders the same keyed TextView: its state sees the
        // longer text as an append and only reparses the tail.
        // Number slots show the engine's values; a finished reply with
        // numbers typed by the model gets the "not from an engine" chip.
        BlockKind::Text { markdown } => {
            let shown = entry.numbers.resolved.as_ref().unwrap_or(markdown);
            // TeX math is pulled out before CommonMark parses the text.
            let text =
                crate::blocks::math::markdown(element_id("text", id), shown).into_any_element();
            if entry.numbers.unverified.is_empty() {
                text
            } else {
                v_flex()
                    .gap_1()
                    .child(text)
                    .child(super::numbers::render(
                        id,
                        &entry.numbers.unverified,
                        window,
                        cx,
                    ))
                    .into_any_element()
            }
        }
        BlockKind::ToolCall {
            name,
            summary,
            input,
            status,
            output,
            ..
        } => render_tool_call(
            name,
            summary,
            input,
            *status,
            output.as_deref(),
            entry.expanded,
            cx,
        ),
        BlockKind::Error { message, hint } => render_error(message, hint.as_deref(), cx),
        _ => crate::blocks::render(entry, window, cx)
            .unwrap_or_else(|| crate::blocks::placeholder(entry, cx)),
    }
}

fn render_tool_call(
    name: &str,
    summary: &str,
    input: &serde_json::Value,
    status: ToolStatus,
    output: Option<&str>,
    expanded: bool,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let marker = if expanded { "▾" } else { "▸" };
    let summary = summary.trim();
    let mut line = h_flex()
        .gap_1()
        .child(div().text_color(theme.muted_foreground).child(marker))
        .child(
            div()
                .font_weight(FontWeight::BOLD)
                .child(SharedString::from(name.to_owned())),
        );
    if !summary.is_empty() {
        line = line
            .child(div().text_color(theme.muted_foreground).child("·"))
            .child(
                div()
                    .text_color(theme.muted_foreground)
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(SharedString::from(summary.to_owned())),
            );
    }
    line = line
        .child(div().text_color(theme.muted_foreground).child("·"))
        .child(
            div()
                .text_color(status_color(status, cx))
                .child(status_label(status)),
        );
    if !expanded {
        if let Some(note) = denial_note(status, output) {
            line = line
                .child(div().text_color(theme.muted_foreground).child("·"))
                .child(
                    div()
                        .text_color(status_color(status, cx))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(SharedString::from(note.to_owned())),
                );
        }
        return line.into_any_element();
    }
    let input = serde_json::to_string_pretty(input).unwrap_or_default();
    let mut details = v_flex()
        .pl_4()
        .gap_1()
        .text_color(theme.muted_foreground)
        .child(SharedString::from(input));
    if let Some(output) = output {
        details = details.child(
            div()
                .text_color(theme.foreground)
                .child(SharedString::from(output.to_owned())),
        );
    }
    v_flex().child(line).child(details).into_any_element()
}

fn render_error(message: &str, hint: Option<&str>, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let mut el = v_flex()
        .pl_2()
        .border_l_2()
        .border_color(theme.danger)
        .child(
            div()
                .text_color(theme.danger)
                .child(SharedString::from(message.to_owned())),
        );
    if let Some(hint) = hint {
        el = el.child(
            div()
                .text_color(theme.muted_foreground)
                .child(SharedString::from(format!("→ {hint}"))),
        );
    }
    el.into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::block::Block;

    #[test]
    fn a_denial_shows_its_recorded_reason() {
        assert_eq!(
            denial_note(ToolStatus::Denied, Some("denied by user: no")),
            Some("denied by user: no")
        );
        assert_eq!(denial_note(ToolStatus::Denied, Some("  ")), None);
        assert_eq!(denial_note(ToolStatus::Succeeded, Some("ok")), None);
        assert_eq!(denial_note(ToolStatus::Denied, None), None);
    }

    #[test]
    fn tool_line_shows_name_args_and_status() {
        assert_eq!(
            tool_call_line("read_file", "src/main.rs:1-40", ToolStatus::Running),
            "read_file · src/main.rs:1-40 · running"
        );
        assert_eq!(
            tool_call_line("bash", "  ", ToolStatus::Succeeded),
            "bash · done"
        );
    }

    #[test]
    fn unknown_kinds_use_their_serde_tag() {
        let block = Block::new(BlockKind::StageHeader {
            stage: "explore".into(),
            index: 1,
            total: 2,
        });
        let json = serde_json::to_value(&block).unwrap_or_default();
        assert_eq!(json["type"], kind_name(&block.kind));
    }

    #[test]
    fn copy_text_of_error_includes_hint() {
        let kind = BlockKind::Error {
            message: "rate limited".into(),
            hint: Some("wait a minute".into()),
        };
        assert_eq!(copy_text(&kind), "rate limited\nwait a minute");
    }
}
