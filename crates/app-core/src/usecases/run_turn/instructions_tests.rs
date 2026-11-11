//! The instructions files in the agent loop: the base ones after the
//! system prompt, the nested ones attached to tool results once.

use std::sync::Arc;

use async_trait::async_trait;
use domain::conversation::{
    ContentPart, Message, Role, StopReason, ToolResultContent, ToolUseId, Turn, Usage,
};
use domain::permission::{PermissionMode, ToolEffect};
use futures::FutureExt;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use super::tests::Harness;
use super::*;
use crate::testing::instructions::FixedInstructions;
use crate::testing::llm::ScriptedResponse;
use crate::tools::registry::{PreparedCall, ToolContext, ToolOutput, TypedTool};

#[derive(Debug, Deserialize, JsonSchema)]
struct PathInput {
    path: String,
}

/// A `read_file` that answers with the path it read.
struct FakeRead;

#[async_trait]
impl TypedTool for FakeRead {
    type Input = PathInput;

    fn name(&self) -> &str {
        "read_file"
    }

    fn description(&self) -> &str {
        "Reads a file."
    }

    fn effect(&self, _input: &PathInput) -> ToolEffect {
        ToolEffect::Read
    }

    async fn prepare(
        &self,
        input: PathInput,
        _ctx: &ToolContext,
    ) -> Result<PreparedCall, ToolError> {
        let path = input.path.clone();
        Ok(PreparedCall::new(
            path.clone(),
            async move { Ok(ToolOutput::text(format!("contents of {path}"))) }.boxed(),
        ))
    }
}

fn harness(script: Vec<ScriptedResponse>, port: &Arc<FixedInstructions>) -> Harness {
    let port: Arc<dyn InstructionsPort> = Arc::clone(port) as _;
    Harness::with(script, PermissionMode::Ask, |tools| {
        tools.register_typed(FakeRead).unwrap();
    })
    .configure(|s| s.with_instructions(port))
}

fn read(id: &str, path: &str) -> ScriptedResponse {
    ScriptedResponse::tool_use(id, "read_file", json!({ "path": path }))
}

/// The texts of the tool results in `message`.
fn result_texts(message: &Message) -> Vec<String> {
    message
        .content
        .iter()
        .filter_map(|part| match part {
            ContentPart::ToolResult { content, .. } => Some(content),
            _ => None,
        })
        .flatten()
        .filter_map(|item| match item {
            ToolResultContent::Text { text } => Some(text.clone()),
            ToolResultContent::Image { .. } => None,
        })
        .collect()
}

fn system_text(h: &Harness, request: usize) -> String {
    h.llm.requests()[request]
        .system
        .iter()
        .map(|block| block.text.clone())
        .collect()
}

#[tokio::test]
async fn the_base_files_follow_the_system_prompt_and_are_read_again_each_request() {
    let port = Arc::new(FixedInstructions::new().with_base("# Project instructions\nUse tabs."));
    let mut h = harness(
        vec![ScriptedResponse::text("ok"), ScriptedResponse::text("ok")],
        &port,
    );
    h.run("hola").await.unwrap();
    assert_eq!(
        system_text(&h, 0),
        "Eres crest.\n\n# Project instructions\nUse tabs."
    );
    port.set_base("# Project instructions\nUse spaces.");
    h.run("otra").await.unwrap();
    assert!(
        system_text(&h, 1).ends_with("Use spaces."),
        "{}",
        system_text(&h, 1)
    );
}

#[tokio::test]
async fn without_base_files_the_system_prompt_is_unchanged() {
    let port = Arc::new(FixedInstructions::new());
    let mut h = harness(vec![ScriptedResponse::text("ok")], &port);
    h.run("hola").await.unwrap();
    assert_eq!(system_text(&h, 0), "Eres crest.");
}

#[tokio::test]
async fn a_nested_file_is_attached_to_the_first_call_in_its_subtree_only() {
    let port = Arc::new(FixedInstructions::new().with_nested("pkg", "No unwrap in pkg."));
    let mut h = harness(
        vec![
            read("t1", "pkg/a.rs"),
            read("t2", "pkg/b.rs"),
            read("t3", "src/main.rs"),
            ScriptedResponse::text("listo"),
        ],
        &port,
    );
    let outcome = h.run("lee").await.unwrap();
    assert_eq!(outcome.stop_reason, StopReason::EndTurn);
    let turn = h.session.turns.last().unwrap();
    let results: Vec<_> = turn
        .messages
        .iter()
        .filter(|m| m.role == Role::User)
        .skip(1)
        .map(result_texts)
        .collect();
    assert_eq!(results.len(), 3);
    assert_eq!(results[0].len(), 2, "{results:?}");
    assert!(
        results[0][1]
            .contains("<instructions path=\"pkg/AGENTS.md\">\nNo unwrap in pkg.\n</instructions>")
    );
    assert_eq!(results[1], ["contents of pkg/b.rs"]);
    assert_eq!(results[2], ["contents of src/main.rs"]);

    // A later turn finds it in the history and does not attach it again.
    h.llm.push(read("t4", "pkg/c.rs"));
    h.llm.push(ScriptedResponse::text("ok"));
    h.run("más").await.unwrap();
    let turn = h.session.turns.last().unwrap();
    assert_eq!(result_texts(&turn.messages[2]), ["contents of pkg/c.rs"]);
}

#[tokio::test]
async fn parallel_calls_in_one_subtree_attach_the_file_once() {
    let port = Arc::new(FixedInstructions::new().with_nested("pkg", "x"));
    let mut h = harness(
        vec![
            read("t1", "pkg/a.rs").then_tool_use("t2", "read_file", json!({ "path": "pkg/b.rs" })),
            ScriptedResponse::text("listo"),
        ],
        &port,
    );
    h.run("lee").await.unwrap();
    let turn = h.session.turns.last().unwrap();
    let texts = result_texts(&turn.messages[2]);
    let attached = texts
        .iter()
        .filter(|t| t.contains("<instructions path="))
        .count();
    assert_eq!(attached, 1, "{texts:?}");
}

#[tokio::test]
async fn after_compaction_the_summary_names_the_file_and_it_is_attached_again() {
    // An old turn that had pkg/AGENTS.md attached, then a short one kept.
    let mut old = Turn::new(Message::user_text("lee"));
    old.messages
        .push(Message::assistant(vec![ContentPart::ToolUse {
            id: ToolUseId::new("t0"),
            name: "read_file".into(),
            input: json!({ "path": "pkg/a.rs" }),
        }]));
    old.messages.push(Message::new(
        Role::User,
        vec![ContentPart::ToolResult {
            tool_use_id: ToolUseId::new("t0"),
            content: vec![
                ToolResultContent::text("contents"),
                ToolResultContent::text(crate::usecases::instructions::attachment(&[
                    crate::ports::instructions::InstructionFile {
                        path: "pkg/AGENTS.md".into(),
                        scope: crate::ports::instructions::InstructionScope::Nested,
                        text: "x".into(),
                        imports: Vec::new(),
                    },
                ])),
            ],
            is_error: false,
        }],
    ));
    old.messages
        .push(Message::assistant(vec![ContentPart::text("hecho")]));
    old.stop_reason = Some(StopReason::EndTurn);
    let mut kept = Turn::new(Message::user_text("b"));
    kept.messages
        .push(Message::assistant(vec![ContentPart::text("c")]));
    kept.stop_reason = Some(StopReason::EndTurn);

    let port = Arc::new(FixedInstructions::new().with_nested("pkg", "x"));
    let big = Usage {
        input_tokens: 100,
        cache_read_input_tokens: 800,
        output_tokens: 10,
        ..Usage::default()
    };
    let mut h = harness(
        vec![
            read("t1", "pkg/b.rs").with_usage(big),
            ScriptedResponse::text("resumen"),
            read("t2", "pkg/c.rs"),
            ScriptedResponse::text("listo"),
        ],
        &port,
    )
    .configure(|s| {
        s.with_compaction(CompactionPolicy {
            context_window_tokens: 1_000,
            threshold: 0.8,
            keep_recent_turns: 1,
        })
    });
    h.session.push_turn(old, 1);
    h.session.push_turn(kept, 1);
    h.run("sigue").await.unwrap();

    let summary = match &h.session.turns[0].messages[0].content[0] {
        ContentPart::Text { text } => text.clone(),
        other => panic!("{other:?}"),
    };
    assert!(summary.ends_with("them): pkg/AGENTS.md."), "{summary}");
    // Before the compaction the history had it; after, the next call in
    // its subtree attaches it again.
    let turn = h.session.turns.last().unwrap();
    assert_eq!(result_texts(&turn.messages[2]), ["contents of pkg/b.rs"]);
    let again = result_texts(&turn.messages[4]);
    assert_eq!(again.len(), 2, "{again:?}");
    assert!(again[1].contains("<instructions path=\"pkg/AGENTS.md\">"));
}
