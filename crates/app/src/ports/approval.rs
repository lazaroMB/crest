//! Asking the user for permission before an action with side effects.
//!
//! The permission policy of the domain decides whether an action needs the
//! user's approval; when it does, the use case builds an [`ApprovalRequest`]
//! with the exact diff or command and awaits [`ApprovalPort::request`]. The UI
//! answers with a modal dialog; the headless mode asks `y/n` on stdin.

use std::fmt;

use async_trait::async_trait;
use domain::block::DiffSpec;
use domain::conversation::ToolUseId;
use domain::permission::{PermissionRule, ToolEffect};
use serde::{Deserialize, Serialize};

/// Identifies one pending [`ApprovalRequest`].
///
/// Assigned by the use case (a per-session counter is enough), so the UI can
/// match the `ApprovalRequested` domain event with the dialog it answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ApprovalRequestId(pub u64);

impl fmt::Display for ApprovalRequestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

/// What the user sees before deciding: the exact change or command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ApprovalPreview {
    /// A file change, shown as a coloured diff.
    Diff(DiffSpec),
    /// A command to execute, shown verbatim.
    Command {
        /// Program to run.
        command: String,
        /// Arguments, one per element, without shell quoting.
        args: Vec<String>,
        /// Working directory, relative to the workspace root.
        cwd: String,
    },
    /// Any other action, described in plain text.
    Text {
        /// Description of the action.
        text: String,
    },
}

/// A request for the user's permission to run one tool call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalRequest {
    /// Id of this request.
    pub id: ApprovalRequestId,
    /// Tool call that needs the approval.
    pub tool_use_id: ToolUseId,
    /// Name of the tool (e.g. `edit_file`, `run_command`).
    pub tool: String,
    /// Kind of side effect the call has.
    pub effect: ToolEffect,
    /// What the call acts on: a workspace path, a command line or a URL.
    /// This is what an "allow always" rule matches against.
    pub subject: String,
    /// The exact change or command to show.
    pub preview: ApprovalPreview,
}

/// The user's answer to an [`ApprovalRequest`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum ApprovalResponse {
    /// Run this call once.
    Approve,
    /// Run this call and remember the rule for the workspace, so similar
    /// calls are not asked again.
    ApproveAlways {
        /// Rule to store with the workspace's permission rules.
        rule: PermissionRule,
    },
    /// Do not run the call.
    Deny {
        /// Optional explanation, passed back to the model.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
}

/// Why an approval could not be obtained.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ApprovalError {
    /// The turn was interrupted while waiting (e.g. `Ctrl+C`, dialog closed).
    #[error("approval cancelled")]
    Cancelled,
    /// There is nobody to ask (no UI attached, stdin is not a terminal).
    #[error("approval unavailable: {0}")]
    Unavailable(String),
}

/// Asks the user to approve an action with side effects.
#[async_trait]
pub trait ApprovalPort: Send + Sync {
    /// Shows `req` to the user and waits for the answer.
    ///
    /// The wait may be arbitrarily long; dropping the future cancels the
    /// request and the adapter must close any dialog it opened. Callers treat
    /// an error as a denial of the call.
    async fn request(&self, req: ApprovalRequest) -> Result<ApprovalResponse, ApprovalError>;
}

#[cfg(test)]
mod tests {
    use domain::permission::{RuleDecision, RuleMatcher};

    use super::*;

    #[test]
    fn response_round_trips_through_json() {
        let responses = [
            ApprovalResponse::Approve,
            ApprovalResponse::ApproveAlways {
                rule: PermissionRule::new(
                    "run_command",
                    RuleMatcher::Prefix("cargo ".into()),
                    RuleDecision::AllowAlways,
                ),
            },
            ApprovalResponse::Deny { reason: None },
        ];
        for response in responses {
            let json = serde_json::to_string(&response).unwrap();
            let back: ApprovalResponse = serde_json::from_str(&json).unwrap();
            assert_eq!(back, response);
        }
    }

    #[test]
    fn request_round_trips_through_json() {
        let req = ApprovalRequest {
            id: ApprovalRequestId(1),
            tool_use_id: ToolUseId::new("toolu_1"),
            tool: "run_command".into(),
            effect: ToolEffect::Execute,
            subject: "cargo test".into(),
            preview: ApprovalPreview::Command {
                command: "cargo".into(),
                args: vec!["test".into()],
                cwd: ".".into(),
            },
        };
        let json = serde_json::to_string(&req).unwrap();
        let back: ApprovalRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(back, req);
    }

    #[test]
    fn port_is_object_safe() {
        fn assert_dyn(_: Option<std::sync::Arc<dyn ApprovalPort>>) {}
        assert_dyn(None);
    }
}
