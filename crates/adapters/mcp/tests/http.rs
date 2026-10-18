//! The manager against the example echo server over streamable HTTP.

mod common;

use std::collections::BTreeMap;

use adapter_mcp::McpManager;
use application::ports::mcp::{McpContent, McpPort, McpServerState};
use application::ports::plugin::{McpServerSpec, McpTransport};
use serde_json::json;

use common::http_server;

#[tokio::test]
async fn lists_and_calls_tools_over_http() {
    let (_server, url) = http_server().await;
    assert!(url.starts_with("http://127.0.0.1:"), "{url}");
    let spec = McpServerSpec::new(
        "remote",
        McpTransport::Http {
            url,
            headers: BTreeMap::from([("x-crest-test".to_owned(), "1".to_owned())]),
        },
    );
    let manager = McpManager::start(vec![spec]);
    manager.wait_ready().await;
    assert_eq!(
        manager.servers().await[0].state,
        McpServerState::Ready { tool_count: 2 }
    );
    let mut names: Vec<_> = manager
        .list_tools()
        .await
        .unwrap()
        .iter()
        .map(|t| t.qualified_name())
        .collect();
    names.sort();
    assert_eq!(names, ["mcp__remote__add", "mcp__remote__echo"]);
    let sum = manager
        .call_tool("remote", "add", json!({ "a": 5, "b": 7 }))
        .await
        .unwrap();
    assert_eq!(sum.content, [McpContent::Text { text: "12".into() }]);
    manager.shutdown().await;
}

#[tokio::test]
async fn an_unreachable_url_fails() {
    let spec = McpServerSpec::new(
        "gone",
        McpTransport::Http {
            // Port 9 (discard) is not served on the loopback.
            url: "http://127.0.0.1:9/mcp".into(),
            headers: BTreeMap::new(),
        },
    );
    let manager = McpManager::start(vec![spec]);
    manager.wait_ready().await;
    let state = &manager.servers().await[0].state;
    assert!(matches!(state, McpServerState::Failed { .. }), "{state:?}");
}

#[tokio::test]
async fn invalid_headers_fail_before_connecting() {
    let spec = McpServerSpec::new(
        "bad",
        McpTransport::Http {
            url: "http://127.0.0.1:9/mcp".into(),
            headers: BTreeMap::from([("bad header".to_owned(), "x".to_owned())]),
        },
    );
    let manager = McpManager::start(vec![spec]);
    manager.wait_ready().await;
    let McpServerState::Failed { message } = &manager.servers().await[0].state else {
        panic!("not failed");
    };
    assert!(message.contains("invalid header name"), "{message}");
}
