use std::sync::Arc;

use async_trait::async_trait;
use domain::block::{BlockKind, ToolStatus};
use domain::conversation::{ContentPart, Role, Session, StopReason, ToolResultContent};
use domain::model::{ModelPolicy, ModelRole};
use domain::permission::{PermissionMode, PermissionPolicy, ToolEffect};
use futures::FutureExt;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use super::*;
use crate::ports::events::DomainEvent;
use crate::ports::llm::LlmError;
use crate::testing::llm::{ScriptedLlm, ScriptedResponse};
use crate::testing::system::{
    RecordingSink, RecordingTelemetry, ScriptedApproval, TelemetryRecord,
};
use crate::tools::registry::{PreparedCall, ToolContext, ToolError, ToolOutput, TypedTool};

#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct TextInput {
    pub(super) text: String,
}

/// Read-only tool that echoes `text`, or fails when `text` is `fail`.
pub(super) struct Echo;

#[async_trait]
impl TypedTool for Echo {
    type Input = TextInput;

    fn name(&self) -> &str {
        "echo"
    }

    fn description(&self) -> &str {
        "Echoes text."
    }

    fn effect(&self, _input: &TextInput) -> ToolEffect {
        ToolEffect::Read
    }

    async fn prepare(
        &self,
        input: TextInput,
        _ctx: &ToolContext,
    ) -> Result<PreparedCall, ToolError> {
        let subject = input.text.clone();
        Ok(PreparedCall::new(
            subject,
            async move {
                if input.text == "fail" {
                    Err(ToolError::Execution("echo failed".into()))
                } else {
                    Ok(ToolOutput::text(format!("echo: {}", input.text)))
                }
            }
            .boxed(),
        ))
    }
}

pub(super) struct Harness {
    pub(super) llm: Arc<ScriptedLlm>,
    pub(super) sink: Arc<RecordingSink>,
    pub(super) telemetry: Arc<RecordingTelemetry>,
    pub(super) service: AgentService,
    pub(super) session: Session,
}

impl Harness {
    pub(super) fn new(script: Vec<ScriptedResponse>) -> Self {
        Self::with(script, PermissionMode::Ask, |_| {})
    }

    pub(super) fn with(
        script: Vec<ScriptedResponse>,
        mode: PermissionMode,
        register: impl FnOnce(&mut ToolRegistry),
    ) -> Self {
        let llm = Arc::new(ScriptedLlm::new(script));
        let sink = Arc::new(RecordingSink::new());
        let approval = Arc::new(ScriptedApproval::new());
        let telemetry = Arc::new(RecordingTelemetry::new());
        let mut tools = ToolRegistry::new();
        tools.register_typed(Echo).unwrap();
        register(&mut tools);
        let deps = AgentDeps {
            llm: llm.clone(),
            events: sink.clone(),
            approval: approval.clone(),
            telemetry: telemetry.clone(),
        };
        let permissions = Arc::new(Mutex::new(PermissionPolicy::new(mode)));
        let service = AgentService::new(deps, tools, ModelPolicy::default(), permissions)
            .with_system_prompt("Eres crest.")
            .with_clock(|| 42);
        Self {
            llm,
            sink,
            telemetry,
            service,
            session: Session::new("/ws", 0),
        }
    }

    /// Applies `f` to the service (iteration limit, prompt…).
    pub(super) fn configure(mut self, f: impl FnOnce(AgentService) -> AgentService) -> Self {
        self.service = f(self.service);
        self
    }

    pub(super) async fn run(&mut self, input: &str) -> Result<TurnOutcome, TurnError> {
        self.service
            .run_turn(&mut self.session, input, TurnOptions::default())
            .await
    }

    /// Kinds of the recorded events, with the status for tool changes.
    pub(super) fn trace(&self) -> Vec<String> {
        self.sink
            .events()
            .iter()
            .map(|event| match event {
                DomainEvent::BlockAdded { block } | DomainEvent::BlockUpdated { block } => {
                    let kind = serde_json::to_value(&block.kind).unwrap()["type"]
                        .as_str()
                        .unwrap()
                        .to_owned();
                    let verb = if matches!(event, DomainEvent::BlockAdded { .. }) {
                        "added"
                    } else {
                        "updated"
                    };
                    format!("{verb}:{kind}")
                }
                DomainEvent::ToolStatusChanged { status, .. } => {
                    format!(
                        "status:{}",
                        serde_json::to_value(status).unwrap().as_str().unwrap()
                    )
                }
                other => serde_json::to_value(other).unwrap()["type"]
                    .as_str()
                    .unwrap()
                    .to_owned(),
            })
            .collect()
    }

    /// Every `tool_result` of the last turn, as `(text, is_error)`.
    pub(super) fn tool_results(&self) -> Vec<(String, bool)> {
        let turn = self.session.turns.last().unwrap();
        turn.messages
            .iter()
            .flat_map(|m| &m.content)
            .filter_map(|part| match part {
                ContentPart::ToolResult {
                    content, is_error, ..
                } => {
                    let text = content
                        .iter()
                        .map(|c| match c {
                            ToolResultContent::Text { text } => text.clone(),
                            ToolResultContent::Image { .. } => "<image>".into(),
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    Some((text, *is_error))
                }
                _ => None,
            })
            .collect()
    }
}

fn two_tools_then_answer() -> Vec<ScriptedResponse> {
    vec![
        ScriptedResponse::thinking("pienso")
            .then_text("Busco.")
            .then_tool_use("toolu_1", "echo", json!({ "text": "a" }))
            .then_tool_use("toolu_2", "echo", json!({ "text": "fail" })),
        ScriptedResponse::text("Listo."),
    ]
}

#[tokio::test]
async fn two_tool_calls_then_end_turn_emit_events_in_order() {
    let mut h = Harness::new(two_tools_then_answer());
    let outcome = h.run("hola").await.unwrap();
    assert_eq!(outcome.stop_reason, StopReason::EndTurn);
    assert_eq!(outcome.model_calls, 2);

    let mut expected = vec!["turn_started", "added:thinking"];
    expected.extend(["thinking_delta"; 3]);
    expected.push("added:text");
    expected.extend(["text_delta"; 3]);
    expected.extend(["added:tool_call", "added:tool_call", "usage_updated"]);
    expected.extend(["updated:tool_call", "status:running", "status:succeeded"]);
    expected.extend(["updated:tool_call", "status:running", "status:failed"]);
    expected.push("added:text");
    expected.extend(["text_delta"; 3]);
    expected.extend(["usage_updated", "turn_finished"]);
    assert_eq!(h.trace(), expected);
}

#[tokio::test]
async fn deltas_and_statuses_point_to_their_blocks() {
    let mut h = Harness::new(two_tools_then_answer());
    h.run("hola").await.unwrap();
    let events = h.sink.events();
    let mut added = Vec::new();
    for event in &events {
        match event {
            DomainEvent::BlockAdded { block } => added.push(block.clone()),
            DomainEvent::TextDelta { block_id, .. } => {
                let block = added.iter().find(|b| b.id == *block_id).unwrap();
                assert!(matches!(block.kind, BlockKind::Text { .. }));
            }
            DomainEvent::ThinkingDelta { block_id, .. } => {
                let block = added.iter().find(|b| b.id == *block_id).unwrap();
                assert!(matches!(
                    block.kind,
                    BlockKind::Thinking {
                        collapsed: true,
                        ..
                    }
                ));
            }
            DomainEvent::ToolStatusChanged { block_id, .. }
            | DomainEvent::BlockUpdated {
                block: domain::block::Block { id: block_id, .. },
            } => {
                let block = added.iter().find(|b| b.id == *block_id).unwrap();
                assert!(matches!(block.kind, BlockKind::ToolCall { .. }));
            }
            _ => {}
        }
    }
    let updated: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            DomainEvent::BlockUpdated { block } => Some(block.kind.clone()),
            _ => None,
        })
        .collect();
    assert!(matches!(
        &updated[0],
        BlockKind::ToolCall { tool_use_id, summary, input, .. }
            if tool_use_id == "toolu_1" && summary == "a" && *input == json!({ "text": "a" })
    ));
    let outputs: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            DomainEvent::ToolStatusChanged {
                status,
                output: Some(output),
                ..
            } => Some((*status, output.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(
        outputs,
        [
            (ToolStatus::Succeeded, "echo: a".to_owned()),
            (ToolStatus::Failed, "echo failed".to_owned()),
        ]
    );
}

#[tokio::test]
async fn session_holds_the_whole_exchange() {
    let mut h = Harness::new(two_tools_then_answer());
    h.run("hola").await.unwrap();
    assert_eq!(h.session.turns.len(), 1);
    assert_eq!(h.session.updated_at, 42);
    let turn = &h.session.turns[0];
    let roles: Vec<_> = turn.messages.iter().map(|m| m.role).collect();
    assert_eq!(
        roles,
        [Role::User, Role::Assistant, Role::User, Role::Assistant]
    );
    assert_eq!(turn.stop_reason, Some(StopReason::EndTurn));
    assert_eq!(
        h.tool_results(),
        [
            ("echo: a".to_owned(), false),
            ("echo failed".to_owned(), true)
        ]
    );
    // The second request carries the tool results right after the tool uses.
    let second = &h.llm.requests()[1];
    assert_eq!(second.messages.len(), 3);
    assert_eq!(second.messages[2].content.len(), 2);
    assert_eq!(turn.usage, h.session.total_usage());
    assert!(turn.usage.input_tokens > 0);
}

#[tokio::test]
async fn request_caches_the_system_prompt_and_the_last_tool() {
    let mut h = Harness::new(vec![ScriptedResponse::text("ok")]);
    h.run("hola").await.unwrap();
    let req = h.llm.last_request().unwrap();
    assert_eq!(req.system, [SystemBlock::cached("Eres crest.")]);
    assert_eq!(req.tools.len(), 1);
    assert!(req.tools[0].cache);
    assert_eq!(req.model.role, ModelRole::Main);
    assert_eq!(req.metadata.session_id, Some(h.session.id));
}

#[tokio::test]
async fn unknown_tool_and_invalid_input_go_back_as_errors() {
    let mut h = Harness::new(vec![
        ScriptedResponse::tool_use("toolu_1", "nope", json!({})).then_tool_use(
            "toolu_2",
            "echo",
            json!({ "text": 1 }),
        ),
        ScriptedResponse::text("vale"),
    ]);
    h.run("hola").await.unwrap();
    let results = h.tool_results();
    assert_eq!(results[0], ("unknown tool `nope`".to_owned(), true));
    assert!(results[1].1);
    assert!(results[1].0.starts_with("invalid input for tool `echo`"));
}

#[tokio::test]
async fn second_turn_sends_the_previous_history() {
    let mut h = Harness::new(vec![
        ScriptedResponse::text("uno"),
        ScriptedResponse::text("dos"),
    ]);
    h.run("a").await.unwrap();
    h.run("b").await.unwrap();
    assert_eq!(h.session.turns.len(), 2);
    assert_eq!(h.llm.requests()[1].messages.len(), 3);
}

#[tokio::test]
async fn iteration_limit_stops_with_a_clear_error() {
    let call = || ScriptedResponse::tool_use("toolu_x", "echo", json!({ "text": "a" }));
    let mut h = Harness::new(vec![call(), call(), call()]).configure(|s| s.with_max_iterations(2));
    let err = h.run("hola").await.unwrap_err();
    assert_eq!(err, TurnError::TooManyIterations { limit: 2 });
    assert_eq!(h.llm.requests().len(), 2);
    assert_eq!(h.trace().last().unwrap(), "turn_failed");
    // The turn ends with tool results, so it can be sent again.
    let last = h.session.turns[0].messages.last().unwrap();
    assert_eq!(last.role, Role::User);
}

#[tokio::test]
async fn model_failure_fails_the_turn() {
    let mut h = Harness::new(vec![ScriptedResponse::open_error(LlmError::Overloaded)]);
    let err = h.run("hola").await.unwrap_err();
    assert_eq!(err, TurnError::Llm(LlmError::Overloaded));
    assert_eq!(h.trace(), ["turn_started", "turn_failed"]);
    assert_eq!(h.session.turns[0].messages.len(), 1);
    let records = h.telemetry.records();
    assert!(matches!(
        records.last(),
        Some(TelemetryRecord::TurnFinished {
            stop_reason: None,
            ..
        })
    ));
}

#[tokio::test]
async fn telemetry_records_model_and_tool_calls() {
    let mut h = Harness::new(two_tools_then_answer());
    h.run("hola").await.unwrap();
    let kinds: Vec<_> = h
        .telemetry
        .records()
        .into_iter()
        .map(|r| match r {
            TelemetryRecord::TurnStarted { .. } => "turn",
            TelemetryRecord::LlmCall(_) => "llm",
            TelemetryRecord::ToolCall(c) if c.ok => "tool-ok",
            TelemetryRecord::ToolCall(_) => "tool-err",
            TelemetryRecord::TurnFinished { .. } => "end",
        })
        .collect();
    assert_eq!(kinds, ["turn", "llm", "tool-ok", "tool-err", "llm", "end"]);
}
