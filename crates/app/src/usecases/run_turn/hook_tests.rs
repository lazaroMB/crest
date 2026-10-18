//! Plugin hooks around tool calls inside `run_turn`.

use std::sync::Arc;

use domain::block::{BlockKind, ToolStatus};
use serde_json::json;

use super::tests::Harness;
use super::*;
use crate::ports::events::{DomainEvent, NoticeLevel};
use crate::ports::plugin::{HookKind, HookOutcome, PluginError};
use crate::testing::llm::ScriptedResponse;
use crate::testing::system::{FakePluginHost, PluginInvocation};

fn echo(text: &str) -> Vec<ScriptedResponse> {
    vec![
        ScriptedResponse::tool_use("toolu_1", "echo", json!({ "text": text })),
        ScriptedResponse::text("Listo."),
    ]
}

fn harness(script: Vec<ScriptedResponse>, host: FakePluginHost) -> (Harness, Arc<FakePluginHost>) {
    let host = Arc::new(host);
    let port = Arc::clone(&host);
    let h = Harness::new(script).configure(|s| s.with_plugin_host(port));
    (h, host)
}

fn tool_statuses(h: &Harness) -> Vec<ToolStatus> {
    h.sink
        .events()
        .iter()
        .filter_map(|event| match event {
            DomainEvent::ToolStatusChanged { status, .. } => Some(*status),
            _ => None,
        })
        .collect()
}

fn tool_inputs(h: &Harness) -> Vec<Value> {
    h.sink
        .events()
        .iter()
        .filter_map(|event| match event {
            DomainEvent::BlockUpdated { block } => match &block.kind {
                BlockKind::ToolCall { input, .. } => Some(input.clone()),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn a_denying_hook_answers_the_model_without_running_or_asking() {
    let host = FakePluginHost::new().deny_tool("echo", Some("no echo".into()));
    let (mut h, host) = harness(echo("a"), host);
    h.run("hola").await.unwrap();

    assert_eq!(
        h.tool_results(),
        [("denied by hook: no echo".to_owned(), true)]
    );
    assert_eq!(tool_statuses(&h), [ToolStatus::Denied]);
    assert!(h.approval.requests().is_empty());
    assert_eq!(tool_inputs(&h), [json!({ "text": "a" })]);
    assert_eq!(
        host.invocations(),
        [PluginInvocation::Hook(HookEvent::BeforeTool {
            name: "echo".into(),
            input: json!({ "text": "a" }),
        })]
    );
}

#[tokio::test]
async fn a_denial_without_reason_says_so() {
    let (mut h, _) = harness(echo("a"), FakePluginHost::new().deny_tool("echo", None));
    h.run("hola").await.unwrap();
    assert_eq!(h.tool_results(), [("denied by hook".to_owned(), true)]);
}

#[tokio::test]
async fn a_hook_can_rewrite_the_input() {
    let host = FakePluginHost::new().on_hook(HookKind::BeforeTool, |_| {
        Ok(HookOutcome::ReplaceInput(json!({ "text": "b" })))
    });
    let (mut h, _) = harness(echo("a"), host);
    h.run("hola").await.unwrap();

    assert_eq!(h.tool_results(), [("echo: b".to_owned(), false)]);
    assert_eq!(tool_inputs(&h), [json!({ "text": "b" })]);
}

#[tokio::test]
async fn a_failing_hook_is_a_notice_and_the_call_goes_on() {
    let host =
        FakePluginHost::new().on_hook(HookKind::BeforeTool, |_| Err(PluginError::InstructionLimit));
    let (mut h, _) = harness(echo("a"), host);
    h.run("hola").await.unwrap();

    assert_eq!(h.tool_results(), [("echo: a".to_owned(), false)]);
    assert!(h.sink.events().contains(&DomainEvent::Notice {
        level: NoticeLevel::Warning,
        message:
            "before_tool hook failed for `echo`: lua call exceeded the instruction limit".into(),
    }));
}

#[tokio::test]
async fn without_a_plugin_host_no_hook_runs() {
    let mut h = Harness::new(echo("a"));
    h.run("hola").await.unwrap();
    assert_eq!(h.tool_results(), [("echo: a".to_owned(), false)]);
}
