use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use domain::block::{BlockKind, ToolStatus};
use domain::conversation::{
    ContentPart, MAX_IMAGE_BYTES, Role, Session, StopReason, ToolResultContent,
};
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
    pub(super) approval: Arc<ScriptedApproval>,
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
            approval,
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
async fn turn_options_lower_the_iteration_limit() {
    let call = || ScriptedResponse::tool_use("toolu_x", "echo", json!({ "text": "a" }));
    let mut h = Harness::new(vec![call(), call(), call()]);
    let err = h
        .service
        .run_turn(
            &mut h.session,
            "hola",
            TurnOptions::default().with_max_iterations(1),
        )
        .await
        .unwrap_err();
    assert_eq!(err, TurnError::TooManyIterations { limit: 1 });
    assert_eq!(h.llm.requests().len(), 1);

    // A higher limit than the service's does not raise it.
    let mut h = Harness::new(vec![call(), call(), call()]).configure(|s| s.with_max_iterations(2));
    let err = h
        .service
        .run_turn(
            &mut h.session,
            "hola",
            TurnOptions::default().with_max_iterations(9),
        )
        .await
        .unwrap_err();
    assert_eq!(err, TurnError::TooManyIterations { limit: 2 });
}

#[tokio::test]
async fn model_failure_fails_the_turn() {
    let mut h = Harness::new(vec![ScriptedResponse::open_error(LlmError::Overloaded)]);
    let err = h.run("hola").await.unwrap_err();
    assert_eq!(err, TurnError::Llm(LlmError::Overloaded));
    // The timeline keeps the failure, not only the status bar.
    assert_eq!(h.trace(), ["turn_started", "added:error", "turn_failed"]);
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

#[test]
fn run_turn_future_is_send() {
    fn assert_send<T: Send>(_: &T) {}
    let mut h = Harness::new(vec![]);
    let fut = h
        .service
        .run_turn(&mut h.session, "hola", TurnOptions::default());
    assert_send(&fut);
}

#[tokio::test]
async fn tool_results_are_truncated_to_the_budget() {
    let long = (1..=50).map(|i| format!("línea {i}")).collect::<Vec<_>>();
    let mut h = Harness::with(
        vec![
            ScriptedResponse::tool_use("toolu_1", "echo", json!({ "text": long.join("\n") })),
            ScriptedResponse::text("ok"),
        ],
        PermissionMode::Ask,
        |tools| {
            *tools = std::mem::take(tools).with_budget(crate::tools::budget::ResultBudget::new(10));
        },
    );
    h.run("hola").await.unwrap();
    let (text, is_error) = &h.tool_results()[0];
    assert!(!is_error);
    assert!(text.starts_with("echo: línea 1\nlínea 2\n"), "{text}");
    assert!(text.contains("more lines omitted"), "{text}");
    assert!(text.len() < 200, "{text}");
}

fn png() -> ImageAttachment {
    ImageAttachment {
        media_type: "image/png".into(),
        data: "iVBORw0KGgo=".into(),
        width: 1,
        height: 1,
    }
}

fn image_service(llm: ScriptedLlm) -> (Arc<ScriptedLlm>, AgentService) {
    let (llm, _, service) = image_service_with_sink(llm);
    (llm, service)
}

fn image_service_with_sink(
    llm: ScriptedLlm,
) -> (Arc<ScriptedLlm>, Arc<RecordingSink>, AgentService) {
    let llm = Arc::new(llm);
    let sink = Arc::new(RecordingSink::new());
    let deps = AgentDeps {
        llm: llm.clone(),
        events: sink.clone(),
        approval: Arc::new(ScriptedApproval::new()),
        telemetry: Arc::new(RecordingTelemetry::new()),
    };
    let permissions = Arc::new(Mutex::new(PermissionPolicy::new(PermissionMode::Ask)));
    let service = AgentService::new(
        deps,
        ToolRegistry::new(),
        ModelPolicy::default(),
        permissions,
    );
    (llm, sink, service)
}

#[tokio::test]
async fn images_go_after_the_prompt_in_the_user_message() {
    let (llm, service) = image_service(ScriptedLlm::new([ScriptedResponse::text("a cat")]));
    let mut session = Session::new("/ws", 0);
    let input = UserInput::new("what is this?", vec![png(), png()]);
    service
        .run_turn(&mut session, input, TurnOptions::default())
        .await
        .unwrap();
    let sent = &llm.last_request().unwrap().messages[0];
    assert_eq!(sent.role, Role::User);
    assert_eq!(sent.content.len(), 3);
    assert_eq!(sent.content[0], ContentPart::text("what is this?"));
    assert!(matches!(
        &sent.content[1],
        ContentPart::Image { media_type, .. } if media_type == "image/png"
    ));
    assert_eq!(session.turns[0].messages[0].content, sent.content);
}

#[tokio::test]
async fn without_a_selected_model_nothing_is_sent() {
    use domain::model::{BuiltinDefaults, ModelId};
    let llm = Arc::new(ScriptedLlm::new([ScriptedResponse::text("hi")]));
    let sink = Arc::new(RecordingSink::new());
    let deps = AgentDeps {
        llm: llm.clone(),
        events: sink.clone(),
        approval: Arc::new(ScriptedApproval::new()),
        telemetry: Arc::new(RecordingTelemetry::new()),
    };
    let permissions = Arc::new(Mutex::new(PermissionPolicy::new(PermissionMode::Ask)));
    let unselected = ModelPolicy::default().with_selection(None, BuiltinDefaults::None);
    let service = AgentService::new(
        deps.clone(),
        ToolRegistry::new(),
        unselected,
        permissions.clone(),
    );
    let mut session = Session::new("/ws", 0);
    let err = service
        .run_turn(&mut session, "hello", TurnOptions::default())
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            TurnError::NoModelSelected {
                role: ModelRole::Main
            }
        ),
        "{err}"
    );
    assert!(llm.requests().is_empty());
    assert!(session.turns.is_empty());
    assert!(sink.events().iter().any(|event| matches!(
        event,
        DomainEvent::BlockAdded { block } if matches!(
            &block.kind,
            BlockKind::Error { message, .. } if message.contains("/model")
        )
    )));
    // Once a model is selected, it is the one the turn uses.
    let selected = ModelPolicy::default().with_selection(
        Some(ModelId::new("opencode-go:glm-5.3")),
        BuiltinDefaults::None,
    );
    let service = AgentService::new(deps, ToolRegistry::new(), selected, permissions);
    service
        .run_turn(&mut session, "hello", TurnOptions::default())
        .await
        .unwrap();
    assert_eq!(
        llm.last_request().unwrap().model.spec.id.as_str(),
        "opencode-go:glm-5.3"
    );
}

#[tokio::test]
async fn images_for_a_text_only_model_are_refused_before_sending() {
    let (llm, sink, service) =
        image_service_with_sink(ScriptedLlm::new([ScriptedResponse::text("never")]).text_only());
    let mut session = Session::new("/ws", 0);
    let err = service
        .run_turn(
            &mut session,
            UserInput::new("look", vec![png()]),
            TurnOptions::default(),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, TurnError::ImagesUnsupported { .. }), "{err}");
    assert!(err.to_string().contains("nothing was sent"));
    assert!(llm.requests().is_empty());
    assert!(session.turns.is_empty());
    // The use case says why, for every front end.
    assert!(sink.events().iter().any(|event| matches!(
        event,
        DomainEvent::BlockAdded { block } if matches!(
            &block.kind,
            BlockKind::Error { message, .. } if message.contains("does not accept images")
        )
    )));
    // Text alone still works with that model.
    service
        .run_turn(&mut session, "hello", TurnOptions::default())
        .await
        .unwrap();
    assert_eq!(llm.requests().len(), 1);
}

#[tokio::test]
async fn images_that_break_the_limits_are_refused() {
    let (llm, service) = image_service(ScriptedLlm::new([]));
    let mut session = Session::new("/ws", 0);
    let bmp = ImageAttachment {
        media_type: "image/bmp".into(),
        data: "Qk0=".into(),
        width: 1,
        height: 1,
    };
    let huge = ImageAttachment {
        media_type: "image/png".into(),
        data: "A".repeat(MAX_IMAGE_BYTES / 3 * 4 + 400),
        width: 1,
        height: 1,
    };
    let many = vec![png(); MAX_IMAGES_PER_MESSAGE + 1];
    for (images, needle) in [
        (vec![bmp], "image/bmp"),
        (vec![huge], "over the limit"),
        (many, "at most"),
    ] {
        let err = service
            .run_turn(
                &mut session,
                UserInput::new("x", images),
                TurnOptions::default(),
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains(needle), "{err}");
    }
    assert!(llm.requests().is_empty());
}

#[tokio::test]
async fn secrets_are_masked_in_prompts_and_tool_results() {
    let mut h = Harness::new(vec![
        ScriptedResponse::tool_use(
            "toolu_1",
            "echo",
            json!({ "text": "key AKIAIOSFODNN7EXAMPLE of ana@acme.io" }),
        ),
        ScriptedResponse::text("ok"),
    ]);
    h.run("use token=a1b2c3d4e5f6 please").await.unwrap();

    let sent = &h.llm.requests()[0].messages[0];
    let prompt = serde_json::to_string(&sent.content).unwrap();
    assert!(prompt.contains("token=[REDACTED:secret]"), "{prompt}");
    assert!(!prompt.contains("a1b2c3d4e5f6"), "{prompt}");
    assert!(h.sink.events().iter().any(|e| matches!(
        e,
        DomainEvent::Notice { message, .. } if message.contains("1 secret")
    )));

    let (text, _) = &h.tool_results()[0];
    assert!(
        text.starts_with("echo: key [REDACTED:aws_key] of [REDACTED:email]\n[crest redacted 2"),
        "{text}"
    );
    let shown: Vec<_> = h
        .sink
        .events()
        .into_iter()
        .filter_map(|e| match e {
            DomainEvent::ToolStatusChanged {
                output: Some(output),
                ..
            } => Some(output),
            _ => None,
        })
        .collect();
    assert!(shown.iter().all(|o| !o.contains("AKIA")), "{shown:?}");
}

#[tokio::test]
async fn a_disabled_redactor_masks_nothing() {
    let mut h = Harness::with(
        vec![
            ScriptedResponse::tool_use(
                "toolu_1",
                "echo",
                json!({ "text": "AKIAIOSFODNN7EXAMPLE" }),
            ),
            ScriptedResponse::text("ok"),
        ],
        PermissionMode::Ask,
        |tools| {
            *tools = std::mem::take(tools).with_redactor(crate::privacy::Redactor::disabled());
        },
    );
    h.run("hola").await.unwrap();
    assert_eq!(h.tool_results()[0].0, "echo: AKIAIOSFODNN7EXAMPLE");
}

#[tokio::test]
async fn ask_user_waits_in_its_form_block_and_shows_the_answer_there() {
    use crate::ports::form::FormResponse;
    use crate::testing::form::ScriptedForm;
    use crate::tools::form::AskUserTool;
    use domain::form::FormState;

    let form = Arc::new(ScriptedForm::new([FormResponse::submitted(
        [("ok".to_owned(), json!(true))].into(),
    )]));
    let port = Arc::clone(&form);
    let script = vec![
        ScriptedResponse::tool_use(
            "toolu_1",
            "ask_user",
            json!({"title": "Go on?", "schema": {"properties": {"ok": {"type": "boolean"}}}}),
        ),
        ScriptedResponse::text("Bien."),
    ];
    let mut h = Harness::with(script, PermissionMode::Ask, move |tools| {
        tools.register_typed(AskUserTool::new(port)).unwrap();
    });
    h.run("hola").await.unwrap();
    let asked = form.requests();
    assert_eq!(asked.len(), 1);
    let id = asked[0].block_id;
    let forms: Vec<_> = h
        .sink
        .events()
        .into_iter()
        .filter_map(|event| match event {
            DomainEvent::BlockAdded { block } | DomainEvent::BlockUpdated { block } => {
                match block.kind {
                    BlockKind::Form(spec) => Some((block.id, spec.state)),
                    _ => None,
                }
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        forms,
        [(id, FormState::Pending), (id, FormState::Submitted)]
    );
    let results = h.tool_results();
    assert!(
        !results[0].1 && results[0].0.contains("\"ok\": true"),
        "{results:?}"
    );
}
