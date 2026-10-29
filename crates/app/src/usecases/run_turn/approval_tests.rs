//! Approvals inside `run_turn`, in the three permission modes.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use domain::block::{ApprovalState, BlockKind, DiffSpec};
use domain::permission::{PermissionMode, PermissionRule, RuleDecision, RuleMatcher, ToolEffect};
use futures::FutureExt;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use super::tests::Harness;
use super::*;
use crate::ports::approval::{ApprovalError, ApprovalPreview, ApprovalResponse};
use crate::testing::llm::ScriptedResponse;
use crate::tools::registry::{PreparedCall, ToolContext, ToolError, ToolOutput, TypedTool};

#[derive(Debug, Deserialize, JsonSchema)]
struct WriteInput {
    path: String,
    text: String,
}

/// Writes nothing, but behaves like `write_file`: a diff block and preview.
struct FakeWrite(Arc<AtomicUsize>);

#[async_trait]
impl TypedTool for FakeWrite {
    type Input = WriteInput;

    fn name(&self) -> &str {
        "write"
    }

    fn description(&self) -> &str {
        "Writes a file."
    }

    fn effect(&self, _input: &WriteInput) -> ToolEffect {
        ToolEffect::Write
    }

    async fn prepare(
        &self,
        input: WriteInput,
        _ctx: &ToolContext,
    ) -> Result<PreparedCall, ToolError> {
        let diff = DiffSpec {
            path: input.path.clone(),
            old_text: None,
            new_text: input.text.clone(),
            unified: format!("+{}", input.text),
            approval: None,
        };
        let runs = Arc::clone(&self.0);
        let path = input.path.clone();
        Ok(PreparedCall::new(
            input.path,
            async move {
                runs.fetch_add(1, Ordering::SeqCst);
                Ok(ToolOutput::text(format!("wrote {path}")))
            }
            .boxed(),
        )
        .with_preview(ApprovalPreview::Diff(diff.clone()))
        .with_block(BlockKind::Diff(diff)))
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
struct RunInput {
    command: String,
}

/// Behaves like `run_command`: an exact command preview and no block.
struct FakeRun;

#[async_trait]
impl TypedTool for FakeRun {
    type Input = RunInput;

    fn name(&self) -> &str {
        "run"
    }

    fn description(&self) -> &str {
        "Runs a command."
    }

    fn effect(&self, _input: &RunInput) -> ToolEffect {
        ToolEffect::Execute
    }

    async fn prepare(
        &self,
        input: RunInput,
        _ctx: &ToolContext,
    ) -> Result<PreparedCall, ToolError> {
        let preview = ApprovalPreview::Command {
            command: input.command.clone(),
            args: Vec::new(),
            cwd: ".".into(),
        };
        let command = input.command.clone();
        let input_command = input.command.clone();
        Ok(PreparedCall::new(
            input.command,
            async move { Ok(ToolOutput::text(format!("ran {command}"))) }.boxed(),
        )
        .with_preview(preview)
        .with_argv(vec![input_command]))
    }
}

fn write(id: &str, path: &str) -> ScriptedResponse {
    ScriptedResponse::tool_use(id, "write", json!({ "path": path, "text": "x" }))
}

fn harness(mode: PermissionMode, script: Vec<ScriptedResponse>) -> (Harness, Arc<AtomicUsize>) {
    let runs = Arc::new(AtomicUsize::new(0));
    let tool_runs = Arc::clone(&runs);
    let h = Harness::with(script, mode, move |tools| {
        tools.register_typed(FakeWrite(tool_runs)).unwrap();
        tools.register_typed(FakeRun).unwrap();
    });
    (h, runs)
}

fn diff_states(h: &Harness) -> Vec<Option<ApprovalState>> {
    h.sink
        .events()
        .iter()
        .filter_map(|event| match event {
            DomainEvent::BlockAdded { block } | DomainEvent::BlockUpdated { block } => {
                match &block.kind {
                    BlockKind::Diff(spec) => Some(spec.approval),
                    _ => None,
                }
            }
            _ => None,
        })
        .collect()
}

/// The trace of the tool calls only: from the first update of a tool block.
fn tool_trace(h: &Harness) -> Vec<String> {
    let trace = h.trace();
    let start = trace.iter().position(|e| e == "updated:tool_call").unwrap();
    let end = trace
        .iter()
        .rposition(|e| e.starts_with("status:"))
        .unwrap();
    trace[start..=end].to_vec()
}

#[tokio::test]
async fn ask_mode_asks_with_the_diff_and_runs_when_approved() {
    let (mut h, runs) = harness(
        PermissionMode::Ask,
        vec![
            write("toolu_1", "src/a.rs"),
            ScriptedResponse::text("hecho"),
        ],
    );
    h.approval.push(ApprovalResponse::Approve);
    h.run("escribe").await.unwrap();

    assert_eq!(runs.load(Ordering::SeqCst), 1);
    assert_eq!(h.tool_results(), [("wrote src/a.rs".to_owned(), false)]);
    assert_eq!(
        tool_trace(&h),
        [
            "updated:tool_call",
            "added:diff",
            "updated:diff",
            "status:awaiting_approval",
            "approval_requested",
            "updated:diff",
            "status:running",
            "status:succeeded",
        ]
    );
    assert_eq!(
        diff_states(&h),
        [
            None,
            Some(ApprovalState::Pending),
            Some(ApprovalState::Approved)
        ]
    );
    let requests = h.approval.requests();
    assert_eq!(requests.len(), 1);
    let req = &requests[0];
    assert_eq!(req.tool, "write");
    assert_eq!(req.subject, "src/a.rs");
    assert_eq!(req.effect, ToolEffect::Write);
    assert!(matches!(&req.preview, ApprovalPreview::Diff(d) if d.path == "src/a.rs"));
    // The approval request points to the diff block.
    let diff_id = h
        .sink
        .events()
        .iter()
        .find_map(|e| match e {
            DomainEvent::BlockAdded { block } if matches!(block.kind, BlockKind::Diff(_)) => {
                Some(block.id)
            }
            _ => None,
        })
        .unwrap();
    assert!(h.sink.events().contains(&DomainEvent::ApprovalRequested {
        request_id: req.id,
        block_id: diff_id,
    }));
}

#[tokio::test]
async fn ask_mode_denied_by_user_reports_the_reason() {
    let (mut h, runs) = harness(
        PermissionMode::Ask,
        vec![write("toolu_1", "src/a.rs"), ScriptedResponse::text("vale")],
    );
    h.approval.push(ApprovalResponse::Deny {
        reason: Some("no toques eso".into()),
    });
    h.run("escribe").await.unwrap();

    assert_eq!(runs.load(Ordering::SeqCst), 0);
    assert_eq!(
        h.tool_results(),
        [("denied by user: no toques eso".to_owned(), true)]
    );
    assert_eq!(tool_trace(&h).last().unwrap(), "status:denied");
    assert_eq!(
        diff_states(&h).last().copied().flatten(),
        Some(ApprovalState::Rejected)
    );
}

#[tokio::test]
async fn approve_always_remembers_the_rule() {
    let (mut h, runs) = harness(
        PermissionMode::Ask,
        vec![
            write("toolu_1", "src/a.rs"),
            write("toolu_2", "src/a.rs"),
            ScriptedResponse::text("hecho"),
        ],
    );
    let rule = PermissionRule::new(
        "write",
        RuleMatcher::Exact("src/a.rs".into()),
        RuleDecision::AllowAlways,
    );
    h.approval
        .push(ApprovalResponse::ApproveAlways { rule: rule.clone() });
    h.run("escribe dos veces").await.unwrap();

    assert_eq!(runs.load(Ordering::SeqCst), 2);
    assert_eq!(h.approval.requests().len(), 1);
    let permissions = h.service.permissions().lock().unwrap();
    assert_eq!(permissions.rules, [rule]);
}

#[tokio::test]
async fn command_rules_match_the_argv_of_the_call() {
    // The subject `my tool` is unquoted, so only the argv `["my tool"]`
    // matches the rule; the subject would be compared as `'my tool'`.
    let (mut h, _) = harness(
        PermissionMode::Ask,
        vec![
            ScriptedResponse::tool_use("toolu_1", "run", json!({ "command": "my tool" })),
            ScriptedResponse::text("hecho"),
        ],
    );
    h.service
        .permissions()
        .lock()
        .unwrap()
        .remember(PermissionRule::new(
            "run",
            RuleMatcher::Command(vec!["my tool".into()]),
            RuleDecision::AllowAlways,
        ));
    h.run("prueba").await.unwrap();
    assert!(h.approval.requests().is_empty());
    assert_eq!(h.tool_results(), [("ran my tool".to_owned(), false)]);
}

#[tokio::test]
async fn approval_failure_counts_as_a_denial() {
    let (mut h, runs) = harness(
        PermissionMode::Ask,
        vec![write("toolu_1", "a"), ScriptedResponse::text("vale")],
    );
    h.approval
        .push_error(ApprovalError::Unavailable("no tty".into()));
    h.run("escribe").await.unwrap();
    assert_eq!(runs.load(Ordering::SeqCst), 0);
    assert_eq!(
        h.tool_results(),
        [(
            "not approved: approval unavailable: no tty".to_owned(),
            true
        )]
    );
}

#[tokio::test]
async fn auto_edit_mode_writes_without_asking_but_asks_to_execute() {
    let (mut h, runs) = harness(
        PermissionMode::AutoEdit,
        vec![
            write("toolu_1", "src/a.rs").then_tool_use(
                "toolu_2",
                "run",
                json!({ "command": "cargo" }),
            ),
            ScriptedResponse::text("hecho"),
        ],
    );
    h.approval.push(ApprovalResponse::Approve);
    h.run("escribe y prueba").await.unwrap();

    assert_eq!(runs.load(Ordering::SeqCst), 1);
    assert_eq!(
        h.tool_results(),
        [
            ("wrote src/a.rs".to_owned(), false),
            ("ran cargo".to_owned(), false)
        ]
    );
    let requests = h.approval.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].tool, "run");
    assert_eq!(
        requests[0].preview,
        ApprovalPreview::Command {
            command: "cargo".into(),
            args: vec![],
            cwd: ".".into()
        }
    );
    // The diff of an allowed call keeps the tool's approval state.
    assert_eq!(diff_states(&h), [None]);
}

#[tokio::test]
async fn plan_mode_denies_writes_without_asking() {
    let (mut h, runs) = harness(
        PermissionMode::Plan,
        vec![
            write("toolu_1", "src/a.rs").then_tool_use("toolu_2", "echo", json!({ "text": "r" })),
            ScriptedResponse::text("solo leo"),
        ],
    );
    h.run("escribe").await.unwrap();

    assert_eq!(runs.load(Ordering::SeqCst), 0);
    assert!(h.approval.requests().is_empty());
    assert_eq!(
        h.tool_results(),
        [
            (
                "denied by permission policy: plan mode is read-only".to_owned(),
                true
            ),
            ("echo: r".to_owned(), false),
        ]
    );
    assert_eq!(
        tool_trace(&h),
        [
            "updated:tool_call",
            "status:denied",
            "updated:tool_call",
            "status:running",
            "status:succeeded",
        ]
    );
    assert!(diff_states(&h).is_empty());
}
