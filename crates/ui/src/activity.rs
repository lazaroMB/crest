//! Activity line and prompt queue, between the timeline and the prompt.
//!
//! While the session works (a turn, or a `:` command such as `:research`
//! that runs a flow up to its next gate) one line says so and keeps moving:
//!
//! ```text
//! ⠹ Researching · Stage 3/8 Data · explore · grep "foo" · 1m 12s
//! ```
//!
//! a spinner, a verb for the kind of work, the last stage header, what is
//! going on now (the last [`DomainEvent::Activity`], tool call, retry or
//! streamed text) and the elapsed time. Below it, the prompts queued while
//! the session is busy, oldest first. Nothing takes any height when the
//! session is idle and the queue is empty.
//!
//! [`ActivityState`] is the pure model, updated with an explicit `now` so
//! it is tested without sleeping; [`ActivityView`] draws it and repaints on
//! a timer only while there is work.

use std::time::{Duration, Instant};

use application::ports::events::DomainEvent;
use domain::block::BlockKind;
use gpui::{
    App, Context, IntoElement, ParentElement as _, Render, SharedString, Styled as _, Task, Window,
    div,
};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};

/// Frames of the spinner.
pub const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
/// Time between two spinner frames.
pub const FRAME: Duration = Duration::from_millis(100);
/// Work shorter than this never shows the line, so quick commands do not
/// flash it.
pub const SHOW_AFTER: Duration = Duration::from_millis(150);
/// Characters of a queued prompt or a detail shown before an ellipsis.
const MAX_CHARS: usize = 120;

/// What the session is busy with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkKind {
    /// A turn of the agent (or an answer to a flow's gate).
    Turn,
    /// A `:` command, by name (`research`, `implement`...).
    Command(String),
}

impl WorkKind {
    /// The verb of the activity line.
    pub fn verb(&self) -> String {
        match self {
            Self::Turn => "Working".to_owned(),
            Self::Command(name) => match name.as_str() {
                "research" => "Researching".to_owned(),
                "implement" => "Implementing".to_owned(),
                "simulate" => "Simulating".to_owned(),
                other => format!("Running :{other}"),
            },
        }
    }
}

/// The work in progress.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Work {
    kind: WorkKind,
    started: Instant,
    stage: Option<String>,
    detail: Option<String>,
}

/// The model of the activity line and the queue.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ActivityState {
    work: Option<Work>,
    queue: Vec<String>,
}

impl ActivityState {
    /// Starts, changes the kind of, or (`None`) ends the work. The start
    /// time and progress of work that goes on are kept.
    pub fn set_work(&mut self, kind: Option<WorkKind>, now: Instant) {
        match (kind, &mut self.work) {
            (None, _) => self.work = None,
            (Some(kind), Some(work)) => work.kind = kind,
            (Some(kind), None) => {
                self.work = Some(Work {
                    kind,
                    started: now,
                    stage: None,
                    detail: None,
                });
            }
        }
    }

    /// Whether there is work.
    pub fn is_working(&self) -> bool {
        self.work.is_some()
    }

    /// Shows `queue` (oldest first) below the line.
    pub fn set_queue(&mut self, queue: Vec<String>) {
        self.queue = queue;
    }

    /// The queued prompts, oldest first.
    pub fn queue(&self) -> &[String] {
        &self.queue
    }

    /// Updates the progress from `event`. Returns whether anything changed.
    pub fn apply(&mut self, event: &DomainEvent) -> bool {
        let Some(work) = &mut self.work else {
            return false;
        };
        let detail = match event {
            DomainEvent::BlockAdded { block } => match &block.kind {
                BlockKind::StageHeader {
                    stage,
                    index,
                    total,
                } => {
                    work.stage = Some(format!("Stage {index}/{total} {stage}"));
                    work.detail = None;
                    return true;
                }
                BlockKind::ToolCall { name, summary, .. } if summary.is_empty() => {
                    Some(name.clone())
                }
                BlockKind::ToolCall { name, summary, .. } => Some(format!("{name} {summary}")),
                _ => return false,
            },
            DomainEvent::Activity { message } => Some(message.clone()),
            DomainEvent::Retrying { attempt, .. } => Some(format!("retrying (attempt {attempt})")),
            DomainEvent::TextDelta { .. } => Some("writing".to_owned()),
            DomainEvent::ThinkingDelta { .. } => Some("thinking".to_owned()),
            _ => return false,
        };
        if work.detail == detail {
            return false;
        }
        work.detail = detail;
        true
    }

    /// The activity line at `now` (spinner `frame`), or `None` when idle or
    /// before [`SHOW_AFTER`].
    pub fn line(&self, now: Instant, frame: usize) -> Option<String> {
        let work = self.work.as_ref()?;
        let elapsed = now.saturating_duration_since(work.started);
        if elapsed < SHOW_AFTER {
            return None;
        }
        let mut parts = vec![format!(
            "{} {}",
            SPINNER[frame % SPINNER.len()],
            work.kind.verb()
        )];
        parts.extend(work.stage.clone());
        parts.extend(
            work.detail
                .as_deref()
                .map(|detail| truncate(detail, MAX_CHARS)),
        );
        parts.push(format_elapsed(elapsed));
        if work.kind == WorkKind::Turn {
            parts.push("Ctrl+C interrupts".to_owned());
        }
        Some(parts.join(" · "))
    }
}

/// `12s`, `1m 05s`, `2h 03m`.
pub fn format_elapsed(elapsed: Duration) -> String {
    let secs = elapsed.as_secs();
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m {:02}s", secs / 60, secs % 60),
        _ => format!("{}h {:02}m", secs / 3600, secs / 60 % 60),
    }
}

/// The first line of `text`, cut to `max` characters with an ellipsis.
pub fn truncate(text: &str, max: usize) -> String {
    let line = text.lines().next().unwrap_or_default();
    let more = line.chars().count() > max || text.lines().nth(1).is_some();
    let mut out: String = line.chars().take(max).collect();
    if more {
        out.push('…');
    }
    out
}

/// Draws [`ActivityState`].
pub struct ActivityView {
    state: ActivityState,
    frame: usize,
    ticker: Option<Task<()>>,
}

impl Default for ActivityView {
    fn default() -> Self {
        Self::new()
    }
}

impl ActivityView {
    /// An idle line with an empty queue.
    pub fn new() -> Self {
        Self {
            state: ActivityState::default(),
            frame: 0,
            ticker: None,
        }
    }

    /// The model.
    pub fn state(&self) -> &ActivityState {
        &self.state
    }

    /// The line as drawn now.
    pub fn line(&self, cx: &App) -> Option<String> {
        self.state.line(cx.background_executor().now(), self.frame)
    }

    /// See [`ActivityState::set_work`]; the spinner turns while there is
    /// work.
    pub fn set_work(&mut self, kind: Option<WorkKind>, cx: &mut Context<Self>) {
        let now = cx.background_executor().now();
        self.state.set_work(kind, now);
        if !self.state.is_working() {
            self.ticker = None;
        } else if self.ticker.is_none() {
            self.ticker = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(FRAME).await;
                    let alive = this.update(cx, |this, cx| {
                        this.frame = this.frame.wrapping_add(1);
                        cx.notify();
                    });
                    if alive.is_err() {
                        break;
                    }
                }
            }));
        }
        cx.notify();
    }

    /// See [`ActivityState::set_queue`].
    pub fn set_queue(&mut self, queue: Vec<String>, cx: &mut Context<Self>) {
        if self.state.queue != queue {
            self.state.set_queue(queue);
            cx.notify();
        }
    }

    /// See [`ActivityState::apply`].
    pub fn apply(&mut self, event: &DomainEvent, cx: &mut Context<Self>) {
        if self.state.apply(event) {
            cx.notify();
        }
    }
}

impl Render for ActivityView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let line = self.line(cx);
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let mut view = v_flex()
            .w_full()
            .text_sm()
            .font_family(theme.mono_font_family.clone());
        if let Some(line) = line {
            // The spinner and the verb stand out; the rest is muted.
            let (head, rest) = match line.split_once(" · ") {
                Some((head, rest)) => (head.to_owned(), format!(" · {rest}")),
                None => (line, String::new()),
            };
            view = view.child(
                h_flex()
                    .px_2()
                    .pt_1()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(
                        div()
                            .text_color(theme.primary)
                            .child(SharedString::from(head)),
                    )
                    .child(div().text_color(muted).child(SharedString::from(rest))),
            );
        }
        let queue = self.state.queue();
        if !queue.is_empty() {
            let hint = format!(
                "{} queued · ↑ edits the last · Ctrl+C discards",
                queue.len()
            );
            view = view
                .child(
                    div()
                        .px_2()
                        .pt_1()
                        .text_color(muted)
                        .child(SharedString::from(hint)),
                )
                .children(queue.iter().map(|prompt| {
                    div()
                        .px_2()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_color(muted)
                        .child(SharedString::from(format!(
                            "↳ {}",
                            truncate(prompt, MAX_CHARS)
                        )))
                }));
        }
        view
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::block::{Block, ToolStatus};
    use serde_json::json;

    fn stage(index: u32, name: &str) -> DomainEvent {
        DomainEvent::BlockAdded {
            block: Block::new(BlockKind::StageHeader {
                stage: name.into(),
                index,
                total: 8,
            }),
        }
    }

    fn tool(name: &str, summary: &str) -> DomainEvent {
        DomainEvent::BlockAdded {
            block: Block::new(BlockKind::ToolCall {
                tool_use_id: "t1".into(),
                name: name.into(),
                summary: summary.into(),
                input: json!({}),
                status: ToolStatus::Running,
                output: None,
            }),
        }
    }

    #[test]
    fn idle_has_no_line_and_short_work_waits() {
        let now = Instant::now();
        let mut state = ActivityState::default();
        assert_eq!(state.line(now, 0), None);
        state.set_work(Some(WorkKind::Turn), now);
        assert_eq!(state.line(now + SHOW_AFTER / 2, 0), None);
        assert_eq!(
            state.line(now + Duration::from_secs(3), 0).as_deref(),
            Some("⠋ Working · 3s · Ctrl+C interrupts")
        );
    }

    #[test]
    fn stage_and_detail_follow_the_events() {
        let now = Instant::now();
        let mut state = ActivityState::default();
        assert!(!state.apply(&stage(1, "Intake")), "idle ignores events");
        state.set_work(Some(WorkKind::Command("research".into())), now);
        assert!(state.apply(&stage(3, "Data")));
        assert!(state.apply(&DomainEvent::Activity {
            message: "Data: asking claude".into()
        }));
        let later = now + Duration::from_secs(72);
        assert_eq!(
            state.line(later, 2).as_deref(),
            Some("⠹ Researching · Stage 3/8 Data · Data: asking claude · 1m 12s")
        );
        assert!(state.apply(&tool("grep", "\"foo\"")));
        assert!(state.line(later, 0).unwrap().contains("· grep \"foo\" ·"));
        assert!(state.apply(&stage(4, "Analysis")));
        let line = state.line(later, 0).unwrap();
        assert!(line.contains("Stage 4/8 Analysis · 1m 12s"), "{line}");
    }

    #[test]
    fn work_that_goes_on_keeps_its_start_and_progress() {
        let now = Instant::now();
        let mut state = ActivityState::default();
        state.set_work(Some(WorkKind::Command("research".into())), now);
        state.apply(&stage(2, "Observation"));
        state.set_work(Some(WorkKind::Turn), now + Duration::from_secs(5));
        let line = state.line(now + Duration::from_secs(10), 0).unwrap();
        assert_eq!(
            line,
            "⠋ Working · Stage 2/8 Observation · 10s · Ctrl+C interrupts"
        );
        state.set_work(None, now);
        assert!(!state.is_working());
        assert_eq!(state.line(now + Duration::from_secs(10), 0), None);
    }

    #[test]
    fn elapsed_and_truncation_formats() {
        assert_eq!(format_elapsed(Duration::from_secs(9)), "9s");
        assert_eq!(format_elapsed(Duration::from_secs(65)), "1m 05s");
        assert_eq!(format_elapsed(Duration::from_secs(7380)), "2h 03m");
        assert_eq!(truncate("hola\nmundo", 10), "hola…");
        assert_eq!(truncate("abcdef", 3), "abc…");
        assert_eq!(truncate("abc", 3), "abc");
    }
}
