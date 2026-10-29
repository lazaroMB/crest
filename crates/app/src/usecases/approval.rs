//! Approval of tool calls: the permission policy decides, and when it says
//! `Ask` the user is asked through the [`ApprovalPort`] with the exact diff
//! or command.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use domain::block::{BlockId, ToolStatus};
use domain::conversation::ToolUseId;
use domain::permission::{ActionRequest, Decision, PermissionPolicy, PermissionRule, RuleScope};

use crate::ports::approval::{
    ApprovalPort, ApprovalPreview, ApprovalRequest, ApprovalRequestId, ApprovalResponse,
};
use crate::ports::events::{DomainEvent, EventSink, NoticeLevel};
use crate::usecases::permissions::ProjectRules;

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
    /// Where "always in project" rules are written, when the workspace has
    /// a project permissions file.
    project: Option<ProjectRules>,
    next_id: AtomicU64,
}

impl std::fmt::Debug for ApprovalGate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApprovalGate")
            .field("permissions", &self.permissions)
            .field("project", &self.project)
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
            project: None,
            next_id: AtomicU64::new(1),
        }
    }

    /// Returns the same gate writing "always in project" rules to
    /// `project`.
    #[must_use]
    pub fn with_project_rules(mut self, project: ProjectRules) -> Self {
        self.project = Some(project);
        self
    }

    /// Where project rules are written, if anywhere.
    #[must_use]
    pub fn project_rules(&self) -> Option<&ProjectRules> {
        self.project.as_ref()
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
    /// "Approve always" adds its rule to the shared policy; a project rule
    /// is also written to the project file, and when that fails it stays
    /// for the session and a warning notice says so. Session rules are
    /// saved with the session by the caller. A failed request (no UI,
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
                lock_permissions(&self.permissions).remember(rule.clone());
                if rule.scope == RuleScope::Project {
                    self.save_project_rule(&rule, events).await;
                }
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

impl ApprovalGate {
    /// Writes `rule` to the project file, warning when it cannot.
    async fn save_project_rule(&self, rule: &PermissionRule, events: &dyn EventSink) {
        let failure = match &self.project {
            Some(project) => project.add(rule).await.err().map(|err| err.to_string()),
            None => Some("this workspace has no project permissions file".to_owned()),
        };
        if let Some(reason) = failure {
            events.emit(DomainEvent::Notice {
                level: NoticeLevel::Warning,
                message: format!(
                    "`{rule}` applies to this session only: it could not be saved to the \
                     project ({reason})"
                ),
            });
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
    #[tokio::test]
    async fn approve_always_in_project_writes_the_project_file() {
        use std::path::Path;

        use crate::testing::permissions::MemoryPermissionStore;

        let rule = PermissionRule::new(
            "run",
            RuleMatcher::Command(vec!["cargo".into()]),
            RuleDecision::AllowAlways,
        )
        .with_scope(RuleScope::Project);
        let store = Arc::new(MemoryPermissionStore::new());
        let gate = gate(ScriptedApproval::always(ApprovalResponse::ApproveAlways {
            rule: rule.clone(),
        }))
        .with_project_rules(ProjectRules::new(store.clone(), "/w"));
        let sink = RecordingSink::new();
        let action = pending("cargo test").action;
        assert_eq!(
            gate.ask(pending("cargo test"), &sink).await,
            UserVerdict::Approved
        );
        assert_eq!(gate.decide(&action), Decision::Allow);
        assert_eq!(store.rules(Path::new("/w")), std::slice::from_ref(&rule));
        // Asked again (another session's rule), nothing is duplicated.
        gate.ask(pending("cargo build"), &sink).await;
        assert_eq!(store.rules(Path::new("/w")), [rule]);
        assert_eq!(store.saves(), 1);
        assert!(
            !sink
                .events()
                .iter()
                .any(|event| matches!(event, DomainEvent::Notice { .. }))
        );
    }

    #[tokio::test]
    async fn a_project_rule_that_cannot_be_saved_stays_for_the_session() {
        use crate::testing::permissions::MemoryPermissionStore;

        let rule = PermissionRule::new(
            "run",
            RuleMatcher::Command(vec!["cargo".into()]),
            RuleDecision::AllowAlways,
        )
        .with_scope(RuleScope::Project);
        let store = Arc::new(MemoryPermissionStore::new());
        store.fail_saves(true);
        let answer = ApprovalResponse::ApproveAlways { rule: rule.clone() };
        for gate in [
            gate(ScriptedApproval::always(answer.clone()))
                .with_project_rules(ProjectRules::new(store.clone(), "/w")),
            gate(ScriptedApproval::always(answer)),
        ] {
            let sink = RecordingSink::new();
            assert_eq!(
                gate.ask(pending("cargo test"), &sink).await,
                UserVerdict::Approved
            );
            assert_eq!(gate.decide(&pending("cargo test").action), Decision::Allow);
            let notice = sink.events().into_iter().find_map(|event| match event {
                DomainEvent::Notice { level, message } => Some((level, message)),
                _ => None,
            });
            let (level, message) = notice.expect("a warning notice");
            assert_eq!(level, NoticeLevel::Warning);
            assert!(
                message.starts_with("`run(cargo *)` applies to this session only"),
                "{message}"
            );
        }
    }

    #[tokio::test]
    async fn approve_always_for_the_session_never_writes_the_project_file() {
        use crate::testing::permissions::MemoryPermissionStore;

        let rule = PermissionRule::new(
            "run",
            RuleMatcher::Command(vec!["cargo".into()]),
            RuleDecision::AllowAlways,
        );
        let store = Arc::new(MemoryPermissionStore::new());
        let gate = gate(ScriptedApproval::always(ApprovalResponse::ApproveAlways {
            rule,
        }))
        .with_project_rules(ProjectRules::new(store.clone(), "/w"));
        let sink = RecordingSink::new();
        gate.ask(pending("cargo test"), &sink).await;
        assert_eq!(store.saves(), 0);
        assert_eq!(sink.events().len(), 2, "no notice");
    }
}
