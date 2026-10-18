//! [`HeadlessSink`]: prints the domain events of a turn in the terminal.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use application::ports::events::{DomainEvent, EventSink, NoticeLevel};
use domain::block::{Block, BlockId, BlockKind, Cell, TableSpec, ToolStatus};
use domain::conversation::{StopReason, Usage};

use super::terminal::{Style, Terminal, Writer};

/// What the sink remembers between events.
#[derive(Default)]
struct SinkState {
    /// Name of each tool call block, for its status lines.
    tools: HashMap<BlockId, String>,
    /// Thinking blocks already announced.
    thinking: Vec<BlockId>,
    /// Usage of the session after the last model call.
    usage: Option<(Usage, Option<f64>)>,
}

/// Prints events legibly: streamed text as it comes, thinking collapsed
/// into one dim line, one line per tool call and status, diffs colored,
/// errors and warnings, and the usage when the turn ends.
#[derive(Debug)]
pub struct HeadlessSink {
    terminal: Terminal,
    state: Mutex<SinkState>,
}

impl std::fmt::Debug for SinkState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SinkState").finish_non_exhaustive()
    }
}

impl HeadlessSink {
    /// A sink printing to `terminal`.
    #[must_use]
    pub fn new(terminal: Terminal) -> Self {
        Self {
            terminal,
            state: Mutex::new(SinkState::default()),
        }
    }

    fn print(&self, state: &mut SinkState, w: &mut Writer<'_>, event: DomainEvent) {
        match event {
            DomainEvent::TurnStarted { model, role, .. } => {
                w.line(Some(Style::Dim), &format!("● {model} ({role})"));
            }
            DomainEvent::BlockAdded { block } | DomainEvent::BlockUpdated { block } => {
                print_block(state, w, &block);
            }
            DomainEvent::TextDelta { text, .. } => w.raw(&text),
            DomainEvent::ThinkingDelta { block_id, .. } => {
                if !state.thinking.contains(&block_id) {
                    state.thinking.push(block_id);
                    w.line(Some(Style::Dim), "∴ thinking…");
                }
            }
            DomainEvent::ToolStatusChanged {
                block_id,
                status,
                output,
            } => print_status(state, w, block_id, status, output.as_deref()),
            DomainEvent::ApprovalRequested { .. }
            | DomainEvent::ModeChanged { .. }
            | DomainEvent::Activity { .. } => {}
            DomainEvent::UsageUpdated {
                session, cost_usd, ..
            } => state.usage = Some((session, cost_usd)),
            DomainEvent::ModelFallback { from, to } => {
                w.line(
                    Some(Style::Yellow),
                    &format!("! {from} is not answering; using {to}"),
                );
            }
            DomainEvent::Retrying { attempt, delay_ms } => {
                w.line(
                    Some(Style::Dim),
                    &format!("… retry {attempt} in {delay_ms} ms"),
                );
            }
            DomainEvent::TurnFinished { stop_reason, .. } => {
                w.break_line();
                if let Some(note) = stop_note(stop_reason) {
                    w.line(Some(Style::Yellow), note);
                }
                if let Some((usage, cost)) = state.usage {
                    w.line(Some(Style::Dim), &usage_line(usage, cost));
                }
            }
            DomainEvent::TurnFailed { error, .. } => {
                w.line(Some(Style::Red), &format!("error: {error}"));
            }
            DomainEvent::ModalOpened { spec } => {
                w.line(
                    Some(Style::Dim),
                    &format!("modal `{}` opened (only visible in the window)", spec.title),
                );
            }
            DomainEvent::ModalUpdated { .. } | DomainEvent::ModalClosed { .. } => {}
            DomainEvent::SimulationReady { board } => {
                w.line(
                    Some(Style::Dim),
                    &format!(
                        "board of {} ready (only visible in the window; board.html in its simulation/)",
                        board.spec
                    ),
                );
            }
            DomainEvent::Notice { level, message } => {
                let (style, label) = match level {
                    NoticeLevel::Info => (Style::Dim, "info"),
                    NoticeLevel::Warning => (Style::Yellow, "warning"),
                    NoticeLevel::Error => (Style::Red, "error"),
                };
                w.line(Some(style), &format!("{label}: {message}"));
            }
        }
    }
}

impl EventSink for HeadlessSink {
    fn emit(&self, event: DomainEvent) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let mut w = self.terminal.lock();
        self.print(&mut state, &mut w, event);
    }
}

fn print_block(state: &mut SinkState, w: &mut Writer<'_>, block: &Block) {
    match &block.kind {
        // Streamed text starts empty and comes as deltas; a text block with
        // content is a message of its own (a gate of a flow).
        BlockKind::Text { markdown } if !markdown.is_empty() => w.line(None, markdown.trim_end()),
        BlockKind::Text { .. } => w.break_line(),
        BlockKind::Thinking { .. } => {}
        BlockKind::ToolCall { name, summary, .. } => {
            let first = state.tools.insert(block.id, name.clone()).is_none();
            // The block is added when the model starts the call and updated
            // once its input is known: print it then.
            if !first || !summary.is_empty() {
                let text = if summary.is_empty() {
                    format!("→ {name}")
                } else {
                    format!("→ {name} {summary}")
                };
                w.line(Some(Style::Cyan), &text);
            }
        }
        BlockKind::Diff(diff) => w.diff(&diff.path, &diff.unified),
        BlockKind::Markdown { title, source } => {
            if let Some(title) = title {
                w.line(Some(Style::Bold), &format!("# {title}"));
            }
            w.line(None, source);
        }
        BlockKind::Error { message, .. } => w.line(Some(Style::Red), &format!("error: {message}")),
        BlockKind::Chart(chart) => w.line(Some(Style::Dim), &format!("[chart: {}]", chart.title)),
        BlockKind::Table(table) => print_table(w, table),
        BlockKind::Mermaid(_) => w.line(Some(Style::Dim), "[mermaid diagram]"),
        BlockKind::StageHeader {
            stage,
            index,
            total,
        } => {
            w.line(
                Some(Style::Bold),
                &format!("== {stage} ({index}/{total}) =="),
            );
        }
        BlockKind::Custom { .. } | BlockKind::Sources { .. } | BlockKind::Image { .. } => {}
    }
}

fn cell_text(cell: &Cell) -> String {
    match cell {
        Cell::Null => String::new(),
        Cell::Bool(true) => "yes".to_owned(),
        Cell::Bool(false) => "no".to_owned(),
        Cell::Number(n) => n.to_string(),
        Cell::Text(text) => text.replace('\n', " "),
    }
}

/// One line per row, cells separated by `|`, the header in bold.
fn print_table(w: &mut Writer<'_>, table: &TableSpec) {
    let header: Vec<_> = table.columns.iter().map(|c| c.name.as_str()).collect();
    w.line(Some(Style::Bold), &format!("  {}", header.join(" | ")));
    for row in &table.rows {
        let cells: Vec<_> = row.iter().map(cell_text).collect();
        w.line(None, &format!("  {}", cells.join(" | ")));
    }
}

fn print_status(
    state: &SinkState,
    w: &mut Writer<'_>,
    block_id: BlockId,
    status: ToolStatus,
    output: Option<&str>,
) {
    let name = state.tools.get(&block_id).map_or("tool", String::as_str);
    let output = output.unwrap_or_default();
    let (style, text) = match status {
        ToolStatus::Pending | ToolStatus::AwaitingApproval | ToolStatus::Running => return,
        ToolStatus::Succeeded => (Style::Green, format!("  ✓ {name}: {output}")),
        ToolStatus::Failed => (Style::Red, format!("  ✗ {name}: {output}")),
        ToolStatus::Denied => (Style::Yellow, format!("  ⊘ {name}: {output}")),
        ToolStatus::Cancelled => (Style::Yellow, format!("  ⊘ {name}: cancelled")),
    };
    w.line(Some(style), text.trim_end_matches([' ', ':']));
}

fn stop_note(stop_reason: StopReason) -> Option<&'static str> {
    match stop_reason {
        StopReason::EndTurn | StopReason::ToolUse | StopReason::StopSequence => None,
        StopReason::MaxTokens => Some("(answer cut off: max_tokens reached)"),
        StopReason::PauseTurn => Some("(turn paused)"),
        StopReason::Refusal => Some("(the model declined to answer)"),
        StopReason::Interrupted => Some("(interrupted)"),
    }
}

fn usage_line(usage: Usage, cost: Option<f64>) -> String {
    let mut line = format!(
        "tokens: {} input, {} output, cache {} read / {} written",
        usage.input_tokens,
        usage.output_tokens,
        usage.cache_read_input_tokens,
        usage.cache_creation_input_tokens
    );
    if let Some(cost) = cost {
        line.push_str(&format!(" · ${cost:.4}"));
    }
    line
}

#[cfg(test)]
mod tests {
    use domain::block::DiffSpec;
    use domain::conversation::TurnId;
    use serde_json::Value;

    use super::*;
    use crate::headless::terminal::tests::terminal;

    fn tool_block(summary: &str) -> Block {
        Block::new(BlockKind::ToolCall {
            tool_use_id: "toolu_1".into(),
            name: "grep".into(),
            summary: summary.into(),
            input: Value::Null,
            status: ToolStatus::Pending,
            output: None,
        })
    }

    #[test]
    fn prints_a_turn_legibly() {
        let (term, buf) = terminal(false);
        let sink = HeadlessSink::new(term);
        let text = Block::new(BlockKind::Text {
            markdown: String::new(),
        });
        let thinking = Block::new(BlockKind::Thinking {
            text: String::new(),
            collapsed: true,
        });
        let mut tool = tool_block("");
        sink.emit(DomainEvent::ThinkingDelta {
            block_id: thinking.id,
            text: "a".into(),
        });
        sink.emit(DomainEvent::ThinkingDelta {
            block_id: thinking.id,
            text: "b".into(),
        });
        sink.emit(DomainEvent::BlockAdded {
            block: text.clone(),
        });
        sink.emit(DomainEvent::TextDelta {
            block_id: text.id,
            text: "Looking for ".into(),
        });
        sink.emit(DomainEvent::TextDelta {
            block_id: text.id,
            text: "the function.".into(),
        });
        sink.emit(DomainEvent::BlockAdded {
            block: tool.clone(),
        });
        tool = Block {
            id: tool.id,
            ..tool_block("fn parse")
        };
        sink.emit(DomainEvent::BlockUpdated {
            block: tool.clone(),
        });
        sink.emit(DomainEvent::ToolStatusChanged {
            block_id: tool.id,
            status: ToolStatus::Running,
            output: None,
        });
        sink.emit(DomainEvent::ToolStatusChanged {
            block_id: tool.id,
            status: ToolStatus::Succeeded,
            output: Some("1 match".into()),
        });
        sink.emit(DomainEvent::BlockAdded {
            block: Block::new(BlockKind::Diff(DiffSpec {
                path: "a.rs".into(),
                old_text: Some("a\n".into()),
                new_text: "b\n".into(),
                unified: "@@ -1 +1 @@\n-a\n+b\n".into(),
                approval: None,
            })),
        });
        sink.emit(DomainEvent::UsageUpdated {
            turn: Usage::default(),
            session: Usage {
                input_tokens: 10,
                output_tokens: 5,
                ..Usage::default()
            },
            cost_usd: None,
        });
        sink.emit(DomainEvent::Notice {
            level: NoticeLevel::Warning,
            message: "no fzf".into(),
        });
        sink.emit(DomainEvent::TurnFinished {
            turn_id: TurnId::new(),
            stop_reason: StopReason::EndTurn,
        });
        assert_eq!(
            buf.text(),
            "∴ thinking…\n\
             Looking for the function.\n\
             → grep fn parse\n  ✓ grep: 1 match\n\
             \x20 diff a.rs\n  @@ -1 +1 @@\n  -a\n  +b\n\
             warning: no fzf\n\
             tokens: 10 input, 5 output, cache 0 read / 0 written\n"
        );
    }

    #[test]
    fn messages_and_tables_are_printed_whole() {
        use domain::block::Column;
        let (term, buf) = terminal(false);
        let sink = HeadlessSink::new(term);
        sink.emit(DomainEvent::BlockAdded {
            block: Block::new(BlockKind::Text {
                markdown: "Shall we start?\n".into(),
            }),
        });
        sink.emit(DomainEvent::BlockAdded {
            block: Block::new(BlockKind::Table(TableSpec {
                columns: vec![Column::new("Expected"), Column::new("Met")],
                rows: vec![vec![Cell::Text("p95 < 20ms".into()), Cell::Bool(true)]],
            })),
        });
        assert_eq!(
            buf.text(),
            "Shall we start?\n  Expected | Met\n  p95 < 20ms | yes\n"
        );
    }

    #[test]
    fn failures_and_interruptions_are_reported() {
        let (term, buf) = terminal(false);
        let sink = HeadlessSink::new(term);
        sink.emit(DomainEvent::TurnFailed {
            turn_id: TurnId::new(),
            error: "401".into(),
        });
        sink.emit(DomainEvent::TurnFinished {
            turn_id: TurnId::new(),
            stop_reason: StopReason::Interrupted,
        });
        assert_eq!(buf.text(), "error: 401\n(interrupted)\n");
    }

    #[test]
    fn modals_are_a_one_line_notice() {
        use domain::block::CustomNode;
        use domain::modal::{ModalId, ModalSpec};
        let (term, buf) = terminal(false);
        let sink = HeadlessSink::new(term);
        let id = ModalId::new("cov/report");
        sink.emit(DomainEvent::ModalOpened {
            spec: ModalSpec {
                id: id.clone(),
                plugin: "cov".into(),
                title: "Coverage".into(),
                width: domain::modal::Percent::default(),
                height: domain::modal::Percent::default(),
                position: domain::modal::ModalPosition::default(),
                content: CustomNode::Text { text: "x".into() },
            },
        });
        sink.emit(DomainEvent::ModalUpdated {
            id: id.clone(),
            content: CustomNode::Text { text: "y".into() },
        });
        sink.emit(DomainEvent::ModalClosed { id });
        assert_eq!(
            buf.text(),
            "modal `Coverage` opened (only visible in the window)\n"
        );
    }
}
