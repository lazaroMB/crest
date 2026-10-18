//! Approval of tool calls: the permission policy decides, and when it says
//! `Ask` the user is asked through the [`ApprovalPort`] with the exact diff
//! or command.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use domain::block::{BlockId, ToolStatus};
use domain::conversation::ToolUseId;
use domain::permission::{ActionRequest, Decision, PermissionPolicy};

use crate::ports::approval::{
    ApprovalPort, ApprovalPreview, ApprovalRequest, ApprovalRequestId, ApprovalResponse,
};
use crate::ports::events::{DomainEvent, EventSink};

/// Permission policy shared between the agent loop, which adds "allow
/// always" rules, and the UI, which cycles the mode.
pub type SharedPermissions = Arc<Mutex<PermissionPolicy>>;

/// Locks `permissions`, ignoring poisoning: the policy is plain data and
/// stays valid if a holder panicked.
pub fn lock_permissions(permissions: &SharedPermissions) -> MutexGuard<'_, PermissionPolicy> {
    permissions.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What the user is asked about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingApproval {
    /// The action, as the policy saw it.
    pub action: ActionRequest,
    /// Tool call being approved.
    pub tool_use_id: ToolUseId,
    /// Diff or command to show; `None` shows the subject as text.
    pub preview: Option<ApprovalPreview>,
    /// `ToolCall` block of the call, whose status becomes
    /// `AwaitingApproval`.
    pub tool_block: BlockId,
    /// Block that shows what is approved (the diff), or the tool block.
    pub shown_in: BlockId,
}

/// The user's verdict on a [`PendingApproval`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserVerdict {
    /// Run the call.
    Approved,
    /// Do not run it; the message goes back to the model.
    Rejected(String),
}

/// Decides tool calls against the shared [`PermissionPolicy`] and asks
/// the user when needed.
pub struct ApprovalGate {
    port: Arc<dyn ApprovalPort>,
    permissions: SharedPermissions,
    next_id: AtomicU64,
}

impl std::fmt::Debug for ApprovalGate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApprovalGate")
            .field("permissions", &self.permissions)
            .finish_non_exhaustive()
    }
}

impl ApprovalGate {
    /// A gate asking through `port` under `permissions`.
    #[must_use]
    pub fn new(port: Arc<dyn ApprovalPort>, permissions: SharedPermissions) -> Self {
        Self {
            port,
            permissions,
            next_id: AtomicU64::new(1),
        }
    }

    /// The shared policy.
    #[must_use]
    pub fn permissions(&self) -> &SharedPermissions {
        &self.permissions
    }

    /// What the policy says about `action`, without asking anybody.
    #[must_use]
    pub fn decide(&self, action: &ActionRequest) -> Decision {
        lock_permissions(&self.permissions).decide(action)
    }

    /// Asks the user about `pending`, emitting `ToolStatusChanged`
    /// (`AwaitingApproval`) and `ApprovalRequested` first.
    ///
    /// "Approve always" adds its rule to the shared policy (persisting it
    /// per workspace is the caller's job). A failed request (no UI,
    /// cancelled dialog) counts as a rejection. Dropping the future
    /// cancels the request.
    pub async fn ask(&self, pending: PendingApproval, events: &dyn EventSink) -> UserVerdict {
        let request_id = ApprovalRequestId(self.next_id.fetch_add(1, Ordering::Relaxed));
        events.emit(DomainEvent::ToolStatusChanged {
            block_id: pending.tool_block,
            status: ToolStatus::AwaitingApproval,
            output: None,
        });
        events.emit(DomainEvent::ApprovalRequested {
            request_id,
            block_id: pending.shown_in,
        });
        let action = pending.action;
        let preview = pending.preview.unwrap_or_else(|| ApprovalPreview::Text {
            text: action.subject.clone(),
        });
        let request = ApprovalRequest {
            id: request_id,
            tool_use_id: pending.tool_use_id,
            tool: action.tool,
            effect: action.effect,
            subject: action.subject,
            preview,
        };
        match self.port.request(request).await {
            Ok(ApprovalResponse::Approve) => UserVerdict::Approved,
            Ok(ApprovalResponse::ApproveAlways { rule }) => {
                lock_permissions(&self.permissions).remember(rule);
                UserVerdict::Approved
            }
            Ok(ApprovalResponse::Deny { reason }) => UserVerdict::Rejected(match reason {
                Some(reason) => format!("denied by user: {reason}"),
                None => "denied by user".to_owned(),
            }),
            Err(err) => UserVerdict::Rejected(format!("not approved: {err}")),
        }
    }
}

/// Message for the model when the policy denies a call.
#[must_use]
pub fn policy_denial(reason: &domain::permission::DenyReason) -> String {
    format!("denied by permission policy: {reason}")
}

#[cfg(test)]
mod tests {
    use domain::permission::PermissionRule;
    use domain::permission::{DenyReason, PermissionMode, RuleDecision, RuleMatcher, ToolEffect};

    use super::*;
    use crate::testing::system::{RecordingSink, ScriptedApproval};

    fn gate(approval: ScriptedApproval) -> ApprovalGate {
        let permissions = Arc::new(Mutex::new(PermissionPolicy::new(PermissionMode::Ask)));
        ApprovalGate::new(Arc::new(approval), permissions)
    }

    fn pending(subject: &str) -> PendingApproval {
        PendingApproval {
            action: ActionRequest::new("run", ToolEffect::Execute, subject),
            tool_use_id: ToolUseId::new("toolu_1"),
            preview: None,
            tool_block: BlockId::new(),
            shown_in: BlockId::new(),
        }
    }

    #[tokio::test]
    async fn ask_emits_events_and_shows_the_subject_by_default() {
        let approval = Arc::new(ScriptedApproval::always(ApprovalResponse::Approve));
        let permissions = Arc::new(Mutex::new(PermissionPolicy::new(PermissionMode::Ask)));
        let gate = ApprovalGate::new(approval.clone(), permissions);
        let sink = RecordingSink::new();
        let call = pending("cargo test");
        let verdict = gate.ask(call.clone(), &sink).await;
        assert_eq!(verdict, UserVerdict::Approved);
        assert_eq!(
            sink.events(),
            [
                DomainEvent::ToolStatusChanged {
                    block_id: call.tool_block,
                    status: ToolStatus::AwaitingApproval,
                    output: None,
                },
                DomainEvent::ApprovalRequested {
                    request_id: ApprovalRequestId(1),
                    block_id: call.shown_in,
                },
            ]
        );
        let req = &approval.requests()[0];
        assert_eq!(
            req.preview,
            ApprovalPreview::Text {
                text: "cargo test".into()
            }
        );
        gate.ask(pending("x"), &sink).await;
        assert_eq!(approval.requests()[1].id, ApprovalRequestId(2));
    }

    #[tokio::test]
    async fn approve_always_adds_the_rule_to_the_policy() {
        let rule = PermissionRule::new(
            "run",
            RuleMatcher::Prefix("cargo".into()),
            RuleDecision::AllowAlways,
        );
        let gate = gate(ScriptedApproval::always(ApprovalResponse::ApproveAlways {
            rule: rule.clone(),
        }));
        let action = pending("cargo test").action;
        assert_eq!(gate.decide(&action), Decision::Ask);
        let verdict = gate.ask(pending("cargo test"), &RecordingSink::new()).await;
        assert_eq!(verdict, UserVerdict::Approved);
        assert_eq!(gate.decide(&action), Decision::Allow);
    }

    #[tokio::test]
    async fn deny_and_errors_are_rejections() {
        let approval = ScriptedApproval::new();
        approval.push(ApprovalResponse::Deny { reason: None });
        let gate = gate(approval);
        let sink = RecordingSink::new();
        assert_eq!(
            gate.ask(pending("a"), &sink).await,
            UserVerdict::Rejected("denied by user".into())
        );
        assert_eq!(
            gate.ask(pending("a"), &sink).await,
            UserVerdict::Rejected(
                "not approved: approval unavailable: no scripted approval answer".into()
            )
        );
        assert_eq!(
            policy_denial(&DenyReason::PlanMode),
            "denied by permission policy: plan mode is read-only"
        );
    }
}
