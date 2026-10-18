//! The steps after Load plan, one file each, and what they share: the
//! agent turn restricted to the task ([`run_agent`]) and skipping a task.

pub mod commit;
pub mod implement;
pub mod report;
pub mod test;
pub mod verify;

use std::sync::Arc;

use domain::plan::Task;

use super::node::{NodeError, StepContext, StepOutcome, final_text};
use super::scope::{TaskScope, scoped_tools};
use super::state::{EntryKind, Step, TaskStatus};
use crate::usecases::run_turn::TurnOptions;

/// System prompt of the implementation agent.
pub const IMPLEMENT_SYSTEM_PROMPT: &str = "\
You are the implementation agent of crest. You implement one task of an approved plan, \
exactly as written: the creative work is done, now you execute, verify and report.
- Change only the files the task lists. Changing any other file stops for the user's \
approval and is recorded as a deviation from the plan: do it only when the task cannot be \
done otherwise, and say why.
- Read the files before changing them. Keep the change minimal and in the style of the code.
- Run the acceptance checks yourself with run_command when you can, and fix what fails.
- Do not commit: the flow commits each task when its checks pass.
- End your answer with one line `STATUS: done`, or `STATUS: blocked: <reason>` when the task \
cannot be done as planned (a missing API, a wrong assumption of the plan). You may add lines \
`IDEA: <text>` with ideas for later work that are out of this task's scope.";

/// How the agent says its work ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentStatus {
    /// The task is done.
    Done,
    /// The task cannot be done as planned.
    Blocked(String),
}

/// What the agent answered, parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentReport {
    /// Its answer without the `STATUS` and `IDEA` lines.
    pub summary: String,
    /// How it ended; `Done` when it did not say.
    pub status: AgentStatus,
    /// Its `IDEA` lines.
    pub ideas: Vec<String>,
}

/// Longest summary kept in the state, in characters.
const SUMMARY_CHARS: usize = 600;

/// Strips `prefix` (ASCII, any case) from `line`.
fn strip_tag<'a>(line: &'a str, prefix: &str) -> Option<&'a str> {
    let head = line.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| line[prefix.len()..].trim())
}

/// Parses the final answer of the agent.
#[must_use]
pub fn parse_report(text: &str) -> AgentReport {
    let mut status = AgentStatus::Done;
    let mut ideas = Vec::new();
    let mut summary = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim().trim_start_matches(['*', '`']).trim();
        if let Some(rest) = strip_tag(trimmed, "STATUS:") {
            let rest = rest.trim_end_matches(['*', '`']).trim();
            status = match strip_tag(rest, "blocked") {
                Some(reason) => {
                    let reason = reason.trim_start_matches([':', '-', '—']).trim();
                    AgentStatus::Blocked(if reason.is_empty() {
                        "no reason given".to_owned()
                    } else {
                        reason.to_owned()
                    })
                }
                None => AgentStatus::Done,
            };
        } else if let Some(idea) = strip_tag(trimmed, "IDEA:") {
            if !idea.is_empty() {
                ideas.push(idea.to_owned());
            }
        } else {
            summary.push(line);
        }
    }
    let summary = summary.join("\n").trim().to_owned();
    let summary = match summary.char_indices().nth(SUMMARY_CHARS) {
        Some((cut, _)) => format!("{}…", &summary[..cut]),
        None => summary,
    };
    AgentReport {
        summary,
        status,
        ideas,
    }
}

/// Runs one agent turn on `task` with `prompt`, its file tools restricted
/// to the task's files (and the extra files the user already allowed).
/// Every touch outside them goes to the journal as a deviation, and the
/// agent's ideas as ideas.
///
/// # Errors
///
/// [`NodeError::Unavailable`] without an agent, or the turn's error.
pub async fn run_agent(
    ctx: &mut StepContext,
    task: &Task,
    prompt: String,
) -> Result<AgentReport, NodeError> {
    let kit = ctx.agent_kit()?.clone();
    let extra = ctx
        .state()
        .record(&task.id)
        .map(|r| r.extra_files.clone())
        .unwrap_or_default();
    let scope = Arc::new(TaskScope::new(
        task.id.clone(),
        task.files.iter().chain(&extra),
        Arc::clone(&kit.deps.approval),
    ));
    let tools = scoped_tools(&kit.tools, &scope).map_err(|e| NodeError::Failed(e.to_string()))?;
    let agent = kit.service(tools, IMPLEMENT_SYSTEM_PROMPT);
    let mut session = ctx.new_session();
    let result = agent
        .run_turn(&mut session, prompt, TurnOptions::new(task.role))
        .await;

    for touch in scope.touches() {
        let text = if touch.approved {
            format!(
                "{} changed {} outside the task's files, with the user's approval.",
                touch.tool, touch.path
            )
        } else {
            format!(
                "{} tried to change {} outside the task's files; the user denied it ({}).",
                touch.tool,
                touch.path,
                touch.reason.as_deref().unwrap_or("no reason given")
            )
        };
        ctx.log(EntryKind::Deviation, text);
        if touch.approved {
            let record = ctx.state_mut().record_mut(&task.id);
            if !record.extra_files.contains(&touch.path) {
                record.extra_files.push(touch.path);
            }
        }
    }
    result?;
    let report = parse_report(&final_text(&session));
    for idea in &report.ideas {
        ctx.log(EntryKind::Idea, idea.clone());
    }
    Ok(report)
}

/// Leaves the current task without a commit and moves on to the next.
pub fn skip_task(ctx: &mut StepContext) -> StepOutcome {
    if let Some(id) = ctx.state().current_task().map(|t| t.id.clone()) {
        ctx.log(
            EntryKind::Deviation,
            format!("Task {id} skipped by the user; its changes stay uncommitted."),
        );
        ctx.state_mut().record_mut(&id).status = TaskStatus::Skipped;
    }
    ctx.state_mut().advance();
    StepOutcome::GoTo(Step::Implement)
}

#[cfg(test)]
pub(crate) mod tests;
