//! An agent turn that calls a tool of the example server: the call asks
//! for approval like any other tool and its result reaches the model.

mod common;

use std::sync::{Arc, Mutex};

use adapter_mcp::McpManager;
use application::ports::approval::ApprovalResponse;
use application::testing::llm::{ScriptedLlm, ScriptedResponse};
use application::testing::system::{RecordingSink, RecordingTelemetry, ScriptedApproval};
use application::tools::mcp::register_mcp_tools;
use application::tools::registry::ToolRegistry;
use application::usecases::run_turn::{AgentDeps, AgentService, TurnOptions};
use domain::conversation::{ContentPart, Session, ToolResultContent};
use domain::model::ModelPolicy;
use domain::permission::{PermissionMode, PermissionPolicy, ToolEffect};
use serde_json::json;

use common::echo_spec;

#[tokio::test]
async fn a_turn_calls_an_mcp_tool_after_approval() {
    let manager = Arc::new(McpManager::start(vec![echo_spec("echo")]));
    manager.wait_ready().await;
    let mut tools = ToolRegistry::new();
    let skipped = register_mcp_tools(&mut tools, manager.clone())
        .await
        .unwrap();
    assert!(skipped.is_empty(), "{skipped:?}");

    let llm = Arc::new(ScriptedLlm::new([
        ScriptedResponse::tool_use("toolu_1", "mcp__echo__add", json!({ "a": 20, "b": 22 })),
        ScriptedResponse::text("Son 42."),
    ]));
    let approval = Arc::new(ScriptedApproval::new());
    approval.push(ApprovalResponse::Approve);
    let deps = AgentDeps {
        llm: llm.clone(),
        events: Arc::new(RecordingSink::new()),
        approval: approval.clone(),
        telemetry: Arc::new(RecordingTelemetry::new()),
    };
    let permissions = Arc::new(Mutex::new(PermissionPolicy::new(PermissionMode::Ask)));
    let service = AgentService::new(deps, tools, ModelPolicy::default(), permissions);
    let mut session = Session::new("/ws", 0);
    service
        .run_turn(&mut session, "¿cuánto es 20 + 22?", TurnOptions::default())
        .await
        .unwrap();

    // The model was offered both tools of the server.
    let offered: Vec<_> = llm.requests()[0]
        .tools
        .iter()
        .map(|t| t.name.clone())
        .collect();
    assert!(
        offered.contains(&"mcp__echo__add".to_owned()),
        "{offered:?}"
    );
    assert!(
        offered.contains(&"mcp__echo__echo".to_owned()),
        "{offered:?}"
    );

    // The call asked for approval.
    let requests = approval.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].tool, "mcp__echo__add");
    assert_eq!(requests[0].effect, ToolEffect::Execute);
    assert_eq!(approval.remaining(), 0);

    // And the sum came back as the tool_result.
    let results: Vec<_> = session.turns[0]
        .messages
        .iter()
        .flat_map(|m| &m.content)
        .filter_map(|part| match part {
            ContentPart::ToolResult {
                content, is_error, ..
            } => Some((content.clone(), *is_error)),
            _ => None,
        })
        .collect();
    assert_eq!(results, [(vec![ToolResultContent::text("42")], false)]);
}
