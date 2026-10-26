//! The manager against the example echo server over stdio.

mod common;

use std::collections::BTreeMap;

use adapter_mcp::{McpEntry, McpManager, McpOptions};
use application::ports::mcp::{McpContent, McpError, McpPort, McpServerState};
use application::ports::plugin::{McpServerSpec, McpTransport};
use serde_json::json;

use common::echo_spec;

fn text(result: &application::ports::mcp::McpToolResult) -> &str {
    match result.content.as_slice() {
        [McpContent::Text { text, .. }] => text,
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
            cwd: None,
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

fn echo_with(name: &str, args: &[&str]) -> McpServerSpec {
    let mut spec = echo_spec(name);
    if let McpTransport::Stdio { args: a, .. } = &mut spec.transport {
        *a = args.iter().map(|s| (*s).to_owned()).collect();
    }
    spec
}

#[tokio::test]
async fn a_slow_server_becomes_ready_and_a_crash_shows_its_stderr() {
    let manager = McpManager::start(vec![
        echo_with("slow", &["--delay-ms", "1500"]),
        echo_with("crash", &["--crash"]),
    ]);
    let mut changes = manager.subscribe();
    manager.wait_ready().await;
    assert!(
        changes.has_changed().unwrap(),
        "state changes are announced"
    );
    let servers = manager.servers().await;
    assert_eq!(servers[0].state, McpServerState::Ready { tool_count: 2 });
    assert!(
        servers[0].transport.contains("--delay-ms"),
        "{}",
        servers[0].transport
    );
    let McpServerState::Failed { message } = &servers[1].state else {
        panic!("unexpected {:?}", servers[1].state);
    };
    assert!(
        message.contains("echo_server: missing configuration"),
        "{message}"
    );
    changes.mark_unchanged();
}

#[tokio::test]
async fn servers_restart_and_are_disabled_and_enabled() {
    let mut disabled = echo_spec("off");
    disabled.disabled = true;
    let manager = McpManager::start(vec![echo_spec("echo"), disabled]);
    manager.wait_ready().await;
    let state = |i: usize| {
        let manager = &manager;
        async move { manager.servers().await[i].state.clone() }
    };
    assert_eq!(state(1).await, McpServerState::Disabled);

    manager.restart("echo").await.unwrap();
    manager.wait_ready().await;
    assert_eq!(state(0).await, McpServerState::Ready { tool_count: 2 });

    manager.set_enabled("echo", false).await.unwrap();
    assert_eq!(state(0).await, McpServerState::Disabled);
    assert!(manager.list_tools().await.unwrap().is_empty());
    assert!(matches!(
        manager
            .call_tool("echo", "add", json!({ "a": 1, "b": 1 }))
            .await,
        Err(McpError::Unavailable { .. })
    ));

    manager.set_enabled("off", true).await.unwrap();
    manager.set_enabled("echo", true).await.unwrap();
    manager.wait_ready().await;
    assert_eq!(state(0).await, McpServerState::Ready { tool_count: 2 });
    assert_eq!(state(1).await, McpServerState::Ready { tool_count: 2 });
    assert!(matches!(
        manager.restart("nope").await,
        Err(McpError::UnknownServer(_))
    ));
}

#[tokio::test]
async fn untrusted_servers_wait_for_trust() {
    let manager = McpManager::start_entries(
        vec![McpEntry {
            spec: echo_spec("repo"),
            trusted: false,
        }],
        McpOptions::default(),
    );
    manager.wait_ready().await;
    assert_eq!(manager.servers().await[0].state, McpServerState::Untrusted);
    assert!(
        manager.restart("repo").await.is_err(),
        "restart does not bypass trust"
    );
    manager.trust("repo").unwrap();
    manager.wait_ready().await;
    assert_eq!(
        manager.servers().await[0].state,
        McpServerState::Ready { tool_count: 2 }
    );
}
