//! Interrupting a turn: the model stream, a running tool or a pending
//! approval.

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use domain::block::ToolStatus;
use domain::conversation::{ContentPart, Role, Session, StopReason};
use domain::permission::{PermissionMode, ToolEffect};
use futures::FutureExt;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use super::tests::Harness;
use super::*;
use crate::ports::shell::{RunCommandRequest, ShellPort};
use crate::testing::fs::ScriptedShell;
use crate::testing::llm::ScriptedResponse;
use crate::testing::system::ScriptedAnswer;
use crate::tools::registry::{PreparedCall, ToolContext, ToolError, ToolOutput, TypedTool};

#[derive(Debug, Deserialize, JsonSchema)]
struct ShInput {
    command: String,
}

/// Runs a command through a [`ScriptedShell`] with the given effect.
struct Sh(Arc<ScriptedShell>, ToolEffect);

#[async_trait]
impl TypedTool for Sh {
    type Input = ShInput;

    fn name(&self) -> &str {
        "sh"
    }

    fn description(&self) -> &str {
        "Runs a command."
    }

    fn effect(&self, _input: &ShInput) -> ToolEffect {
        self.1
    }

    async fn prepare(&self, input: ShInput, _ctx: &ToolContext) -> Result<PreparedCall, ToolError> {
        let shell = Arc::clone(&self.0);
        let subject = input.command.clone();
        Ok(PreparedCall::new(
            subject,
            async move {
                let out = shell
                    .run(RunCommandRequest::new(input.command, Vec::<String>::new()))
                    .await
                    .map_err(|err| ToolError::Execution(err.to_string()))?;
                Ok(ToolOutput::text(out.stdout))
            }
            .boxed(),
        ))
    }
}

/// Runs the turn and cancels it once `ready` holds.
async fn run_and_cancel(h: &mut Harness, ready: impl Fn() -> bool) -> TurnOutcome {
    let token = CancelToken::new();
    let opts = TurnOptions::default().with_cancel(token.clone());
    let Harness {
        service, session, ..
    } = h;
    let turn = service.run_turn(session, "hola", opts);
    let mut turn = std::pin::pin!(turn);
    // Drive the turn until it blocks and `ready` holds, then cancel.
    loop {
        futures::select_biased! {
            outcome = turn.as_mut().fuse() => return outcome.unwrap(),
            () = tokio::task::yield_now().fuse() => {}
        }
        if ready() {
            break;
        }
    }
    token.cancel();
    turn.await.unwrap()
}

/// Every tool use in the history is answered by the next message, so the
/// session can be sent to the API again.
fn assert_resendable(session: &Session) {
    let messages: Vec<_> = session.history().collect();
    for (i, message) in messages.iter().enumerate() {
        let uses: HashSet<_> = message
            .content
            .iter()
            .filter_map(|p| match p {
                ContentPart::ToolUse { id, .. } => Some(id.clone()),
                _ => None,
            })
            .collect();
        if uses.is_empty() {
            continue;
        }
        assert_eq!(message.role, Role::Assistant);
        let next = messages.get(i + 1).expect("tool uses without results");
        let results: HashSet<_> = next
            .content
            .iter()
            .filter_map(|p| match p {
                ContentPart::ToolResult { tool_use_id, .. } => Some(tool_use_id.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(uses, results);
    }
}

fn statuses(h: &Harness) -> Vec<ToolStatus> {
    h.sink
        .events()
        .iter()
        .filter_map(|e| match e {
            DomainEvent::ToolStatusChanged { status, .. } => Some(*status),
            _ => None,
        })
        .collect()
}

/// A closure counting the recorded events of `kind`.
fn counter(h: &Harness, kind: &'static str) -> impl Fn() -> usize + use<> {
    let sink = Arc::clone(&h.sink);
    move || sink.kinds().iter().filter(|k| *k == kind).count()
}

fn sh_harness(
    shell: &Arc<ScriptedShell>,
    effect: ToolEffect,
    script: Vec<ScriptedResponse>,
) -> Harness {
    let shell = Arc::clone(shell);
    Harness::with(script, PermissionMode::Ask, move |tools| {
        tools.register_typed(Sh(shell, effect)).unwrap();
    })
}

#[tokio::test]
async fn cancelling_the_stream_keeps_the_partial_text() {
    // MessageStart, Usage and two of the three text deltas, then silence.
    let mut h = Harness::new(vec![
        ScriptedResponse::text("hola mundo cruel").hang_after(4),
    ]);
    let deltas = counter(&h, "text_delta");
    let outcome = run_and_cancel(&mut h, || deltas() == 2).await;

    assert_eq!(outcome.stop_reason, StopReason::Interrupted);
    assert_eq!(h.llm.open_streams(), 0, "the stream must be dropped");
    let turn = &h.session.turns[0];
    assert_eq!(turn.stop_reason, Some(StopReason::Interrupted));
    assert_eq!(turn.messages.len(), 2);
    let ContentPart::Text { text } = &turn.messages[1].content[0] else {
        panic!("expected text");
    };
    assert!("hola mundo cruel".starts_with(text.as_str()) && !text.is_empty());
    assert_eq!(h.sink.kinds().last().unwrap(), "turn_finished");
    assert!(h.sink.events().contains(&DomainEvent::TurnFinished {
        turn_id: outcome.turn_id,
        stop_reason: StopReason::Interrupted,
    }));
    assert_resendable(&h.session);
}

#[tokio::test]
async fn cancelling_while_a_tool_use_streams_drops_it() {
    let mut h = Harness::new(vec![
        ScriptedResponse::text("voy")
            .then_tool_use("toolu_1", "echo", json!({ "text": "a" }))
            .hang_after(8),
    ]);
    let added = counter(&h, "block_added");
    let outcome = run_and_cancel(&mut h, || added() == 2).await;

    assert_eq!(outcome.stop_reason, StopReason::Interrupted);
    assert_eq!(statuses(&h), [ToolStatus::Cancelled]);
    let turn = &h.session.turns[0];
    assert_eq!(turn.messages.len(), 2);
    assert_eq!(turn.messages[1].content, [ContentPart::text("voy")]);
    assert_resendable(&h.session);
    // The interrupted session can be continued.
    h.llm.push(ScriptedResponse::text("sigo"));
    h.run("sigue").await.unwrap();
    assert_resendable(&h.session);
}

#[tokio::test]
async fn cancelling_a_running_tool_drops_it_and_answers_every_tool_use() {
    let shell = Arc::new(ScriptedShell::new().on_hang("sleep", Vec::<String>::new()));
    let mut h = sh_harness(
        &shell,
        ToolEffect::Read,
        vec![
            ScriptedResponse::tool_use("toolu_1", "sh", json!({ "command": "sleep" }))
                .then_tool_use("toolu_2", "echo", json!({ "text": "b" })),
        ],
    );
    let outcome = run_and_cancel(&mut h, || shell.calls().len() == 1).await;

    assert_eq!(outcome.stop_reason, StopReason::Interrupted);
    assert_eq!(shell.cancelled(), 1, "the tool future must be dropped");
    assert_eq!(
        statuses(&h),
        [
            ToolStatus::Running,
            ToolStatus::Cancelled,
            ToolStatus::Cancelled
        ]
    );
    assert_eq!(
        h.tool_results(),
        [
            ("cancelled by user".to_owned(), true),
            ("cancelled by user".to_owned(), true)
        ]
    );
    assert_eq!(h.session.turns[0].messages.last().unwrap().role, Role::User);
    assert_resendable(&h.session);
}

#[tokio::test]
async fn cancelling_a_pending_approval_cancels_the_call() {
    let shell = Arc::new(ScriptedShell::new());
    let mut h = sh_harness(
        &shell,
        ToolEffect::Execute,
        vec![ScriptedResponse::tool_use(
            "toolu_1",
            "sh",
            json!({ "command": "rm" }),
        )],
    );
    h.approval.set_default(Some(ScriptedAnswer::Pending));
    let approval = Arc::clone(&h.approval);
    let outcome = run_and_cancel(&mut h, || approval.requests().len() == 1).await;

    assert_eq!(outcome.stop_reason, StopReason::Interrupted);
    assert!(shell.calls().is_empty());
    assert_eq!(
        statuses(&h),
        [ToolStatus::AwaitingApproval, ToolStatus::Cancelled]
    );
    assert_eq!(h.tool_results(), [("cancelled by user".to_owned(), true)]);
    assert_resendable(&h.session);
}

#[tokio::test]
async fn a_cancelled_token_skips_the_model_call() {
    let mut h = Harness::new(vec![ScriptedResponse::text("nunca")]);
    let token = CancelToken::new();
    token.cancel();
    let outcome = h
        .service
        .run_turn(
            &mut h.session,
            "hola",
            TurnOptions::default().with_cancel(token),
        )
        .await
        .unwrap();
    assert_eq!(outcome.stop_reason, StopReason::Interrupted);
    assert_eq!(outcome.model_calls, 0);
    assert!(h.llm.requests().is_empty());
    assert_eq!(h.sink.kinds(), ["turn_started", "turn_finished"]);
    assert_eq!(h.session.turns[0].messages.len(), 1);
}

/// A read-only tool that cancels the turn while it runs and then
/// finishes normally.
struct CancelsTurn(CancelToken);

#[async_trait]
impl TypedTool for CancelsTurn {
    type Input = ShInput;

    fn name(&self) -> &str {
        "cancels"
    }

    fn description(&self) -> &str {
        "Cancels the turn."
    }

    fn effect(&self, _input: &ShInput) -> ToolEffect {
        ToolEffect::Read
    }

    async fn prepare(
        &self,
        _input: ShInput,
        _ctx: &ToolContext,
    ) -> Result<PreparedCall, ToolError> {
        let token = self.0.clone();
        Ok(PreparedCall::new(
            "x",
            async move {
                token.cancel();
                Ok(ToolOutput::text("done"))
            }
            .boxed(),
        ))
    }
}

#[tokio::test]
async fn cancelling_at_the_iteration_limit_is_an_interruption() {
    let token = CancelToken::new();
    let tool_token = token.clone();
    let mut h = Harness::with(
        vec![ScriptedResponse::tool_use(
            "toolu_1",
            "cancels",
            json!({ "command": "x" }),
        )],
        PermissionMode::Ask,
        move |tools| tools.register_typed(CancelsTurn(tool_token)).unwrap(),
    )
    .configure(|s| s.with_max_iterations(1));
    let outcome = h
        .service
        .run_turn(
            &mut h.session,
            "hola",
            TurnOptions::default().with_cancel(token),
        )
        .await
        .unwrap();
    assert_eq!(outcome.stop_reason, StopReason::Interrupted);
    assert_eq!(h.tool_results(), [("done".to_owned(), false)]);
    assert_resendable(&h.session);
}
