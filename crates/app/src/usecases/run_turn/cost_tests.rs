use domain::conversation::{ContentPart, Message, Role, StopReason, Turn, Usage};
use domain::model::ModelPricing;
use domain::permission::ToolEffect;
use serde_json::json;

use super::tests::Harness;
use super::*;
use crate::cost::{CostTracker, PricingTable, SPEND_CAP_TOOL};
use crate::ports::approval::{ApprovalPreview, ApprovalResponse};
use crate::ports::events::{DomainEvent, NoticeLevel};
use crate::testing::llm::ScriptedResponse;

/// $1 per million input tokens and $10 per million output tokens; cache
/// reads at $0.1 and writes at $2.
fn pricing() -> PricingTable {
    PricingTable::new().with(
        "m-",
        ModelPricing {
            input_per_mtok: 1.0,
            output_per_mtok: 10.0,
            cache_read_per_mtok: 0.1,
            cache_write_per_mtok: 2.0,
        },
    )
}

fn usage(input: u64, output: u64, cache_read: u64, cache_write: u64) -> Usage {
    Usage {
        input_tokens: input,
        output_tokens: output,
        cache_read_input_tokens: cache_read,
        cache_creation_input_tokens: cache_write,
    }
}

pub(super) fn costs(h: &Harness) -> Vec<Option<f64>> {
    h.sink
        .events()
        .into_iter()
        .filter_map(|e| match e {
            DomainEvent::UsageUpdated { cost_usd, .. } => Some(cost_usd),
            _ => None,
        })
        .collect()
}

fn assert_costs(got: &[Option<f64>], want: &[Option<f64>]) {
    assert_eq!(got.len(), want.len(), "{got:?}");
    for (g, w) in got.iter().zip(want) {
        match (g, w) {
            (Some(g), Some(w)) => assert!((g - w).abs() < 1e-9, "{got:?} != {want:?}"),
            _ => assert_eq!(g, w, "{got:?} != {want:?}"),
        }
    }
}

#[tokio::test]
async fn usage_updated_carries_the_accumulated_session_cost() {
    let mut h = Harness::new(vec![
        ScriptedResponse::tool_use("toolu_1", "echo", json!({ "text": "a" }))
            .with_model("m-1")
            .with_usage(usage(1_000_000, 100_000, 0, 0)),
        ScriptedResponse::text("Listo.")
            .with_model("m-1")
            .with_usage(usage(0, 0, 1_000_000, 500_000)),
        ScriptedResponse::text("Otra.")
            .with_model("m-2")
            .with_usage(usage(500_000, 0, 0, 0)),
    ])
    .configure(|s| s.with_cost_tracker(CostTracker::new(pricing())));
    h.run("hola").await.unwrap();
    h.run("otra").await.unwrap();
    // 1 + 1; then 0.1 + 1; then 0.5 in the next turn of the same session.
    assert_costs(&costs(&h), &[Some(2.0), Some(3.1), Some(3.6)]);
}

#[tokio::test]
async fn without_a_price_the_cost_is_unknown() {
    let script = || {
        vec![
            ScriptedResponse::text("a").with_usage(usage(10, 10, 0, 0)),
            ScriptedResponse::text("b")
                .with_model("otro")
                .with_usage(usage(10, 10, 0, 0)),
        ]
    };
    let mut plain = Harness::new(script());
    plain.run("hola").await.unwrap();
    assert_costs(&costs(&plain), &[None]);

    let mut empty = Harness::new(script())
        .configure(|s| s.with_cost_tracker(CostTracker::new(PricingTable::new())));
    empty.run("hola").await.unwrap();
    assert_costs(&costs(&empty), &[None]);

    let mut unpriced =
        Harness::new(script()).configure(|s| s.with_cost_tracker(CostTracker::new(pricing())));
    unpriced.run("hola").await.unwrap();
    unpriced.run("otra").await.unwrap();
    assert_costs(&costs(&unpriced), &[None, None]);
}

fn old_turn(n: u32) -> Turn {
    let mut turn = Turn::new(Message::user_text(format!("pregunta {n}")));
    turn.messages
        .push(Message::assistant(vec![ContentPart::text("respuesta")]));
    turn.stop_reason = Some(StopReason::EndTurn);
    turn
}

#[tokio::test]
async fn the_compaction_call_is_priced_too() {
    let policy = CompactionPolicy {
        context_window_tokens: 1_000,
        threshold: 0.8,
        keep_recent_turns: 1,
    };
    let pricing = pricing().with(
        "claude-haiku",
        ModelPricing {
            input_per_mtok: 4.0,
            ..ModelPricing::default()
        },
    );
    let mut h = Harness::new(vec![
        ScriptedResponse::tool_use("toolu_1", "echo", json!({ "text": "a" }))
            .with_model("m-1")
            .with_usage(usage(900, 0, 0, 0)),
        ScriptedResponse::text("resumen").with_usage(usage(250_000, 0, 0, 0)),
        ScriptedResponse::text("Listo.")
            .with_model("m-1")
            .with_usage(usage(100_000, 0, 0, 0)),
    ])
    .configure(|s| {
        s.with_compaction(policy)
            .with_cost_tracker(CostTracker::new(pricing))
    });
    for n in 0..3 {
        h.session.push_turn(old_turn(n), 1);
    }
    h.run("sigue").await.unwrap();
    assert_eq!(h.llm.requests()[1].model.role, ModelRole::Compact);
    // 900 tokens at $1; the summary at $4; the answer at $1.
    assert_costs(&costs(&h), &[Some(0.0009), Some(1.0009), Some(1.1009)]);
}

/// A session that pauses once it spent $1.
fn capped(script: Vec<ScriptedResponse>) -> Harness {
    Harness::new(script)
        .configure(|s| s.with_cost_tracker(CostTracker::new(pricing()).with_cap(1.0)))
}

/// An answer by `m-1` costing `usd`, all in input tokens.
fn costing(answer: ScriptedResponse, usd: f64) -> ScriptedResponse {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let tokens = (usd * 1_000_000.0) as u64;
    answer.with_model("m-1").with_usage(usage(tokens, 0, 0, 0))
}

fn echo(id: &str) -> ScriptedResponse {
    ScriptedResponse::tool_use(id, "echo", json!({ "text": "a" }))
}

fn warnings(h: &Harness) -> Vec<String> {
    h.sink
        .events()
        .into_iter()
        .filter_map(|e| match e {
            DomainEvent::Notice {
                level: NoticeLevel::Warning,
                message,
            } => Some(message),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn at_the_cap_the_turn_pauses_and_asks_before_the_next_call() {
    let mut h = capped(vec![
        costing(echo("toolu_1"), 1.0),
        costing(ScriptedResponse::text("Listo."), 0.5),
    ]);
    h.approval.push(ApprovalResponse::Approve);
    let outcome = h.run("hola").await.unwrap();
    assert_eq!(outcome.stop_reason, StopReason::EndTurn);
    assert_eq!(outcome.model_calls, 2);

    let asked = h.approval.requests();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].tool, SPEND_CAP_TOOL);
    assert_eq!(asked[0].effect, ToolEffect::Network);
    assert_eq!(
        asked[0].preview,
        ApprovalPreview::Text {
            text: "The session spent $1.00 of $1.00; continue?".into()
        }
    );
    assert_eq!(
        warnings(&h),
        ["spend cap reached ($1.00 of $1.00); waiting for confirmation"]
    );
}

#[tokio::test]
async fn approving_raises_the_cap_by_its_amount() {
    let mut h = capped(vec![
        costing(ScriptedResponse::text("a"), 1.0),
        costing(ScriptedResponse::text("b"), 0.5),
        costing(ScriptedResponse::text("c"), 0.6),
        costing(ScriptedResponse::text("d"), 0.1),
    ]);
    h.approval.push(ApprovalResponse::Approve);
    h.approval.push(ApprovalResponse::Approve);
    for input in ["1", "2", "3", "4"] {
        h.run(input).await.unwrap();
    }
    // Asked before "2" (spent 1.0 of 1.0) and before "4" (2.1 of 2.0).
    let asked: Vec<_> = h
        .approval
        .requests()
        .into_iter()
        .map(|r| match r.preview {
            ApprovalPreview::Text { text } => text,
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(
        asked,
        [
            "The session spent $1.00 of $1.00; continue?",
            "The session spent $2.10 of $2.00; continue?",
        ]
    );
}

#[tokio::test]
async fn approving_always_lifts_the_cap_for_the_session() {
    let mut h = capped(vec![
        costing(ScriptedResponse::text("a"), 1.0),
        costing(ScriptedResponse::text("b"), 5.0),
        costing(ScriptedResponse::text("c"), 5.0),
    ]);
    h.approval.push(ApprovalResponse::ApproveAlways {
        rule: domain::permission::PermissionRule::new(
            SPEND_CAP_TOOL,
            domain::permission::RuleMatcher::Any,
            domain::permission::RuleDecision::AllowAlways,
        ),
    });
    for input in ["1", "2", "3"] {
        h.run(input).await.unwrap();
    }
    assert_eq!(h.approval.requests().len(), 1);
    assert_eq!(h.llm.requests().len(), 3);
}

#[tokio::test]
async fn denying_stops_the_turn_cleanly_and_the_session_goes_on() {
    let mut h = capped(vec![
        costing(echo("toolu_1"), 1.0),
        costing(ScriptedResponse::text("Listo."), 0.1),
    ]);
    h.approval.push(ApprovalResponse::Deny { reason: None });
    let outcome = h.run("hola").await.unwrap();
    assert_eq!(outcome.stop_reason, StopReason::Interrupted);
    assert_eq!(outcome.model_calls, 1);
    assert_eq!(h.llm.requests().len(), 1);
    assert_eq!(h.tool_results(), [("echo: a".to_owned(), false)]);
    let last = h.session.turns[0].messages.last().unwrap();
    assert_eq!(last.role, Role::User, "no tool_use left unanswered");
    assert_eq!(
        warnings(&h).last().map(String::as_str),
        Some("turn stopped by the spend cap ($1.00 of $1.00)")
    );
    assert!(
        h.trace()
            .ends_with(&["notice".to_owned(), "turn_finished".to_owned()])
    );

    // Still at the cap: the next turn asks again and goes on when approved.
    h.approval.push(ApprovalResponse::Approve);
    let outcome = h.run("sigue").await.unwrap();
    assert_eq!(outcome.stop_reason, StopReason::EndTurn);
    assert_eq!(h.approval.requests().len(), 2);
    let sent = &h.llm.requests()[1].messages;
    assert_eq!(sent.len(), 4);
}

#[tokio::test]
async fn a_failed_request_counts_as_a_denial() {
    let mut h = capped(vec![costing(ScriptedResponse::text("a"), 1.0)]);
    h.run("1").await.unwrap();
    let outcome = h.run("2").await.unwrap();
    assert_eq!(outcome.stop_reason, StopReason::Interrupted);
    assert_eq!(outcome.model_calls, 0);
    assert_eq!(h.llm.requests().len(), 1);
}
