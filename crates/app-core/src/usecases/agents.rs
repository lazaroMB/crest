//! The reports of background agents, on their way to the chat's agent.
//!
//! A subagent sent to the background (`/agent`, `&`, `task` or a forked
//! skill with `background: true`) works while the chat goes on. When it
//! ends, its [`AgentReport`] waits in the [`AgentInbox`] until the chat's
//! agent can read it:
//!
//! - a running turn takes it at its next model call, after the results of
//!   the tools it called ([`AgentInbox::drain`] in the agent loop);
//! - otherwise the window starts a turn for it (`wake`), with the reports
//!   as its message ([`wake_prompt`]).

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex, PoisonError};

use domain::block::AgentStatus;

/// The report of a background agent that ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentReport {
    /// Its number in the session.
    pub id: u64,
    /// The title of its job.
    pub name: String,
    /// How it ended.
    pub status: AgentStatus,
    /// What it reported, or why it failed.
    pub text: String,
}

impl AgentReport {
    /// The report as the model reads it.
    #[must_use]
    pub fn to_message(&self) -> String {
        format!(
            "<agent-report id=\"{}\" name=\"{}\" status=\"{}\">\n{}\n</agent-report>",
            self.id,
            self.name.replace('"', "'"),
            self.status.label(),
            self.text.trim()
        )
    }
}

/// The reports not read yet, shared by the agents and the chat's loop.
#[derive(Debug, Clone, Default)]
pub struct AgentInbox(Arc<Mutex<VecDeque<AgentReport>>>);

impl AgentInbox {
    /// An empty inbox.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Leaves `report` for the chat's agent.
    pub fn push(&self, report: AgentReport) {
        self.lock().push_back(report);
    }

    /// Takes every report, oldest first.
    #[must_use]
    pub fn drain(&self) -> Vec<AgentReport> {
        self.lock().drain(..).collect()
    }

    /// Whether a report is waiting.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lock().is_empty()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, VecDeque<AgentReport>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The text that joins a message of the turn with `reports`, or `None`.
#[must_use]
pub fn reports_text(reports: &[AgentReport]) -> Option<String> {
    if reports.is_empty() {
        return None;
    }
    let mut out = String::from(
        "Background agents finished while you worked; their reports follow. Use them for \
         the task, and tell the user what matters in them.",
    );
    for report in reports {
        let _ = write!(out, "\n\n{}", report.to_message());
    }
    Some(out)
}

/// The message of a turn started only to read `reports` (nobody typed it).
#[must_use]
pub fn wake_prompt(reports: &[AgentReport]) -> Option<String> {
    reports_text(reports).map(|text| {
        format!(
            "{text}\n\n(This message comes from crest, not from the user: the agents above \
             were started earlier in this conversation. Continue the work they were part of, \
             or summarize what they found for the user.)"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(id: u64) -> AgentReport {
        AgentReport {
            id,
            name: format!("Find \"x\" {id}"),
            status: AgentStatus::Done,
            text: "Found it in src/lib.rs:4.\n".into(),
        }
    }

    #[test]
    fn the_inbox_hands_each_report_once_in_order() {
        let inbox = AgentInbox::new();
        assert!(inbox.is_empty());
        inbox.push(report(1));
        inbox.clone().push(report(2));
        assert!(!inbox.is_empty());
        let reports = inbox.drain();
        assert_eq!(reports.iter().map(|r| r.id).collect::<Vec<_>>(), [1, 2]);
        assert!(inbox.drain().is_empty());
    }

    #[test]
    fn reports_read_as_tagged_messages() {
        assert_eq!(
            report(3).to_message(),
            "<agent-report id=\"3\" name=\"Find 'x' 3\" status=\"done\">\nFound it in \
             src/lib.rs:4.\n</agent-report>"
        );
        assert!(reports_text(&[]).is_none());
        let wake = wake_prompt(&[report(1), report(2)]).unwrap();
        assert!(wake.contains("id=\"1\"") && wake.contains("id=\"2\""));
        assert!(wake.contains("not from the user"));
    }
}
