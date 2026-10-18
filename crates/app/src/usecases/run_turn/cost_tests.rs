use domain::conversation::{ContentPart, Message, StopReason, Turn, Usage};
use domain::model::ModelPricing;
use serde_json::json;

use super::tests::Harness;
use super::*;
use crate::cost::{CostTracker, PricingTable};
use crate::ports::events::DomainEvent;
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
