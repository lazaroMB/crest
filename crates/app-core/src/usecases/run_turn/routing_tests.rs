//! Routing of `main` turns with the decision model.

use std::sync::Arc;

use domain::decision::Answer;

use super::tests::Harness;
use super::*;
use crate::decide::DecisionOptions;
use crate::testing::decision::{ScriptedDecider, score};
use crate::testing::llm::ScriptedResponse;
use crate::testing::system::TelemetryRecord;

/// A harness whose decision model calls the prompt trivial and leaves the
/// `echo` tool out.
fn routed() -> (Harness, Arc<ScriptedDecider>) {
    let port = Arc::new(ScriptedDecider::answering(|id, _| match id {
        "difficulty" => score(0.1),
        _ => Answer::YesNo { p_yes: 0.05 },
    }));
    let decider = Decider::new(Arc::clone(&port) as _, DecisionOptions::default());
    let h = Harness::new(vec![ScriptedResponse::text("hola")])
        .configure(|service| service.with_decider(decider));
    (h, port)
}

#[tokio::test]
async fn a_routed_turn_gets_its_model_and_only_the_tools_it_needs() {
    let (mut h, port) = routed();
    h.run("hola").await.unwrap();
    let req = h.llm.last_request().unwrap();
    assert!(req.tools.is_empty(), "echo was left out: {:?}", req.tools);
    assert!(
        req.model.spec.id.as_str().contains("haiku"),
        "{}",
        req.model.spec.id
    );
    assert_eq!(port.requests().len(), 1);
    let notices: Vec<String> = h
        .sink
        .events()
        .into_iter()
        .filter_map(|e| match e {
            DomainEvent::Notice { message, .. } => Some(message),
            _ => None,
        })
        .collect();
    assert!(
        notices
            .iter()
            .any(|n| n.contains("haiku") && n.contains("echo")),
        "{notices:?}"
    );
    let decision_calls = h
        .telemetry
        .records()
        .into_iter()
        .filter(
            |r| matches!(r, TelemetryRecord::LlmCall(call) if call.model.as_str() == "scripted"),
        )
        .count();
    assert_eq!(decision_calls, 1);
}

#[tokio::test]
async fn an_override_skips_the_routing() {
    let (mut h, port) = routed();
    h.service
        .run_turn(
            &mut h.session,
            "hola",
            TurnOptions::default().with_override("sonnet"),
        )
        .await
        .unwrap();
    assert!(port.requests().is_empty());
    let req = h.llm.last_request().unwrap();
    assert_eq!(req.tools.len(), 1, "every tool without routing");
}

#[tokio::test]
async fn a_failing_decision_model_leaves_the_turn_as_before() {
    let decider = Decider::new(
        Arc::new(ScriptedDecider::failing()),
        DecisionOptions::default(),
    );
    let mut h = Harness::new(vec![ScriptedResponse::text("ok")])
        .configure(|service| service.with_decider(decider));
    h.run("hola").await.unwrap();
    let req = h.llm.last_request().unwrap();
    assert_eq!(req.tools.len(), 1);
    assert_eq!(req.model.role, ModelRole::Main);
}
