//! The manager against the example echo server over stdio.

mod common;

use std::collections::BTreeMap;

use adapter_mcp::McpManager;
use application::ports::mcp::{McpContent, McpError, McpPort, McpServerState};
use application::ports::plugin::{McpServerSpec, McpTransport};
use serde_json::json;

use common::echo_spec;

fn text(result: &application::ports::mcp::McpToolResult) -> &str {
    match result.content.as_slice() {
        [McpContent::Text { text }] => text,
        other => panic!("unexpected content {other:?}"),
    }
}

#[tokio::test]
async fn lists_the_tools_with_their_schemas() {
    let manager = McpManager::start(vec![echo_spec("echo")]);
    manager.wait_ready().await;
    let mut tools = manager.list_tools().await.unwrap();
    tools.sort_by(|a, b| a.name.cmp(&b.name));
    let names: Vec<_> = tools.iter().map(|t| t.qualified_name()).collect();
    assert_eq!(names, ["mcp__echo__add", "mcp__echo__echo"]);
    let add = &tools[0];
    assert_eq!(add.server, "echo");
    assert_eq!(add.description, "Adds two integers");
    assert_eq!(add.input_schema["type"], "object");
    assert_eq!(add.input_schema["properties"]["a"]["type"], "integer");
    assert_eq!(add.input_schema["required"], json!(["a", "b"]));
    assert_eq!(
        tools[1].input_schema["properties"]["text"]["type"],
        "string"
    );
    assert_eq!(
        manager.servers().await[0].state,
        McpServerState::Ready { tool_count: 2 }
    );
}

#[tokio::test]
async fn calls_tools_and_reports_errors() {
    let manager = McpManager::start(vec![echo_spec("echo")]);
    manager.wait_ready().await;
    let sum = manager
        .call_tool("echo", "add", json!({ "a": 2, "b": 40 }))
        .await
        .unwrap();
    assert_eq!(text(&sum), "42");
    assert!(!sum.is_error);
    let echoed = manager
        .call_tool("echo", "echo", json!({ "text": "hola" }))
        .await
        .unwrap();
    assert_eq!(text(&echoed), "hola");

    assert_eq!(
        manager.call_tool("echo", "nope", json!({})).await,
        Err(McpError::UnknownTool {
            server: "echo".into(),
            name: "nope".into()
        })
    );
    assert_eq!(
        manager.call_tool("other", "add", json!({})).await,
        Err(McpError::UnknownServer("other".into()))
    );
    assert!(matches!(
        manager.call_tool("echo", "add", json!([1, 2])).await,
        Err(McpError::InvalidInput { .. })
    ));
    // Bad arguments are rejected by the server.
    let bad = manager
        .call_tool("echo", "add", json!({ "a": "dos" }))
        .await;
    assert!(
        matches!(&bad, Err(McpError::Protocol { .. })) || bad.as_ref().is_ok_and(|r| r.is_error),
        "{bad:?}"
    );
}

#[tokio::test]
async fn a_failing_server_does_not_affect_the_others() {
    let missing = McpServerSpec::new(
        "missing",
        McpTransport::Stdio {
            command: "/nonexistent/crest-mcp-server".into(),
            args: Vec::new(),
            env: BTreeMap::new(),
        },
    );
    let manager = McpManager::start(vec![missing, echo_spec("echo"), echo_spec("echo")]);
    manager.wait_ready().await;
    let servers = manager.servers().await;
    assert_eq!(servers.len(), 3);
    assert_eq!(servers[0].name, "missing");
    let McpServerState::Failed { message } = &servers[0].state else {
        panic!("unexpected {:?}", servers[0].state);
    };
    assert!(message.contains("cannot start"), "{message}");
    assert_eq!(servers[1].state, McpServerState::Ready { tool_count: 2 });
    let McpServerState::Failed { message } = &servers[2].state else {
        panic!("unexpected {:?}", servers[2].state);
    };
    assert!(message.contains("duplicate"), "{message}");

    assert_eq!(manager.list_tools().await.unwrap().len(), 2);
    assert!(matches!(
        manager.call_tool("missing", "add", json!({})).await,
        Err(McpError::Unavailable { .. })
    ));
    let sum = manager
        .call_tool("echo", "add", json!({ "a": 1, "b": 1 }))
        .await
        .unwrap();
    assert_eq!(text(&sum), "2");
}

#[tokio::test]
async fn shutdown_stops_every_server() {
    let manager = McpManager::start(vec![echo_spec("echo")]);
    manager.wait_ready().await;
    manager.shutdown().await;
    assert_eq!(manager.servers().await[0].state, McpServerState::Stopped);
    assert!(manager.list_tools().await.unwrap().is_empty());
    assert!(matches!(
        manager
            .call_tool("echo", "add", json!({ "a": 1, "b": 1 }))
            .await,
        Err(McpError::Unavailable { .. })
    ));
}
