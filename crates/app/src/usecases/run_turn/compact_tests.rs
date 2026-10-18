use std::collections::HashSet;
use std::sync::Arc;

use domain::conversation::{ContentPart, Message, Role, StopReason, ToolUseId, Turn, Usage};
use domain::model::ModelRole;
use futures::FutureExt;
use serde_json::json;

use super::tests::Harness;
use super::*;
use crate::ports::events::{DomainEvent, NoticeLevel};
use crate::ports::llm::LlmError;
use crate::testing::llm::ScriptedResponse;
use crate::testing::system::TelemetryRecord;
use crate::usecases::compact::{SUMMARY_ACK, SUMMARY_PREFIX};

fn text_turn(question: &str, answer: &str) -> Turn {
    let mut turn = Turn::new(Message::user_text(question));
    turn.messages
        .push(Message::assistant(vec![ContentPart::text(answer)]));
    turn.usage.input_tokens = 10;
    turn.stop_reason = Some(StopReason::EndTurn);
    turn
}

fn tool_turn(id: &str) -> Turn {
    let mut turn = Turn::new(Message::user_text("usa echo"));
    turn.messages
        .push(Message::assistant(vec![ContentPart::ToolUse {
            id: ToolUseId::new(id),
            name: "echo".into(),
            input: json!({ "text": "x" }),
        }]));
    turn.messages.push(Message::new(
        Role::User,
        vec![ContentPart::tool_result_text(ToolUseId::new(id), "echo: x")],
    ));
    turn.messages
        .push(Message::assistant(vec![ContentPart::text("hecho")]));
    turn.usage.input_tokens = 10;
    turn.stop_reason = Some(StopReason::EndTurn);
    turn
}

/// Small window: 800 tokens trigger compaction, the last turn is kept.
fn policy() -> CompactionPolicy {
    CompactionPolicy {
        context_window_tokens: 1_000,
        threshold: 0.8,
        keep_recent_turns: 1,
    }
}

fn harness(script: Vec<ScriptedResponse>, old: Vec<Turn>) -> Harness {
    let mut h = Harness::new(script).configure(|s| s.with_compaction(policy()));
    for turn in old {
        h.session.push_turn(turn, 1);
    }
    h
}

fn big_usage() -> Usage {
    Usage {
        input_tokens: 100,
        cache_read_input_tokens: 800,
        output_tokens: 10,
        ..Usage::default()
    }
}

fn summary_text(message: &Message) -> Option<&str> {
    match message.content.first() {
        Some(ContentPart::Text { text }) => Some(text),
        _ => None,
    }
}

/// Every `tool_use` is answered by the next message, and every
/// `tool_result` answers the message before it.
fn assert_tool_pairs(messages: &[Message]) {
    for (i, message) in messages.iter().enumerate() {
        let uses: HashSet<_> = message
            .content
            .iter()
            .filter_map(|p| match p {
                ContentPart::ToolUse { id, .. } => Some(id.clone()),
                _ => None,
            })
            .collect();
        let results = |m: &Message| -> HashSet<ToolUseId> {
            m.content
                .iter()
                .filter_map(|p| match p {
                    ContentPart::ToolResult { tool_use_id, .. } => Some(tool_use_id.clone()),
                    _ => None,
                })
                .collect()
        };
        if !uses.is_empty() {
            assert_eq!(results(&messages[i + 1]), uses, "message {i}");
        }
        let answered = results(message);
        if !answered.is_empty() {
            assert!(i > 0, "tool_result without tool_use");
            let before: HashSet<_> = messages[i - 1]
                .content
                .iter()
                .filter_map(|p| match p {
                    ContentPart::ToolUse { id, .. } => Some(id.clone()),
                    _ => None,
                })
                .collect();
            assert_eq!(answered, before, "message {i}");
        }
    }
}

fn notices(h: &Harness) -> Vec<(NoticeLevel, String)> {
    h.sink
        .events()
        .into_iter()
        .filter_map(|e| match e {
            DomainEvent::Notice { level, message } => Some((level, message)),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn past_the_threshold_the_history_is_compacted_and_the_turn_goes_on() {
    let old = vec![
        tool_turn("toolu_a"),
        text_turn("b", "c"),
        tool_turn("toolu_d"),
    ];
    let recent = old[2].clone();
    let mut h = harness(
        vec![
            ScriptedResponse::tool_use("toolu_1", "echo", json!({ "text": "y" }))
                .with_usage(big_usage()),
            ScriptedResponse::text("resumen X"),
            ScriptedResponse::text("Listo."),
        ],
        old,
    );
    let total_before = h.session.total_usage();
    let outcome = h.run("sigue").await.unwrap();
    assert_eq!(outcome.stop_reason, StopReason::EndTurn);
    assert_eq!(outcome.model_calls, 2);

    let requests = h.llm.requests();
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[0].model.role, ModelRole::Main);
    assert_eq!(requests[0].messages.len(), 4 + 2 + 4 + 1);
    assert_eq!(requests[1].model.role, ModelRole::Compact);
    assert_eq!(requests[1].model.spec.id.as_str(), "claude-haiku-5-5");
    assert!(requests[1].tools.is_empty());
    assert_eq!(requests[2].model.role, ModelRole::Main);

    // [summary, ack] + the kept turn + the running turn.
    let sent = &requests[2].messages;
    assert_eq!(sent.len(), 2 + 4 + 3);
    assert_eq!(
        summary_text(&sent[0]),
        Some(format!("{SUMMARY_PREFIX}\n\nresumen X").as_str())
    );
    assert_eq!(sent[1].role, Role::Assistant);
    assert_eq!(summary_text(&sent[1]), Some(SUMMARY_ACK));
    assert_eq!(&sent[2..6], recent.messages.as_slice());
    assert_eq!(summary_text(&sent[6]), Some("sigue"));
    assert_tool_pairs(sent);
    for pair in sent.windows(2) {
        assert_ne!(pair[0].role, pair[1].role, "roles alternate");
    }

    assert_eq!(h.session.turns.len(), 3);
    assert_eq!(h.session.turns[1], recent);
    let summary_usage = h.session.turns[0].usage;
    assert!(
        summary_usage.input_tokens > 20,
        "keeps old usage plus the summary call"
    );
    assert!(h.session.total_usage().input_tokens > total_before.input_tokens);
    assert_eq!(
        notices(&h),
        vec![(
            NoticeLevel::Info,
            "history compacted: 2 turns → summary".to_owned()
        )]
    );
    let llm_calls: Vec<_> = h
        .telemetry
        .records()
        .into_iter()
        .filter_map(|r| match r {
            TelemetryRecord::LlmCall(call) => Some(call.model),
            _ => None,
        })
        .collect();
    assert_eq!(llm_calls.len(), 3);
    assert_eq!(llm_calls[1].as_str(), "claude-haiku-5-5");
}

#[tokio::test]
async fn a_context_too_long_error_compacts_and_retries_once() {
    let mut h = harness(
        vec![
            ScriptedResponse::open_error(LlmError::ContextTooLong),
            ScriptedResponse::text("resumen"),
            ScriptedResponse::text("ok"),
        ],
        vec![text_turn("a", "b"), text_turn("c", "d")],
    );
    let outcome = h.run("hola").await.unwrap();
    assert_eq!(outcome.stop_reason, StopReason::EndTurn);
    let requests = h.llm.requests();
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[1].model.role, ModelRole::Compact);
    let sent = &requests[2].messages;
    assert_eq!(sent.len(), 2 + 2 + 1);
    assert!(summary_text(&sent[0]).unwrap().starts_with(SUMMARY_PREFIX));
    assert_eq!(h.session.turns.len(), 3);
}

#[tokio::test]
async fn context_too_long_twice_fails_the_turn() {
    let mut h = harness(
        vec![
            ScriptedResponse::open_error(LlmError::ContextTooLong),
            ScriptedResponse::text("resumen"),
            ScriptedResponse::open_error(LlmError::ContextTooLong),
        ],
        vec![text_turn("a", "b"), text_turn("c", "d")],
    );
    let err = h.run("hola").await.unwrap_err();
    assert_eq!(err, TurnError::Llm(LlmError::ContextTooLong));
    assert_eq!(h.llm.requests().len(), 3);
}

#[tokio::test]
async fn context_too_long_with_nothing_to_compact_fails_the_turn() {
    let mut h = harness(
        vec![ScriptedResponse::open_error(LlmError::ContextTooLong)],
        vec![text_turn("a", "b")],
    );
    let err = h.run("hola").await.unwrap_err();
    assert_eq!(err, TurnError::Llm(LlmError::ContextTooLong));
    assert_eq!(h.llm.requests().len(), 1);
}

#[tokio::test]
async fn without_a_policy_context_too_long_is_not_retried() {
    let mut h = Harness::new(vec![ScriptedResponse::open_error(LlmError::ContextTooLong)]);
    h.session.push_turn(text_turn("a", "b"), 1);
    h.session.push_turn(text_turn("c", "d"), 1);
    let err = h.run("hola").await.unwrap_err();
    assert_eq!(err, TurnError::Llm(LlmError::ContextTooLong));
    assert_eq!(h.llm.requests().len(), 1);
}

#[tokio::test]
async fn under_the_threshold_nothing_is_compacted() {
    let old = vec![
        text_turn("a", "b"),
        text_turn("c", "d"),
        text_turn("e", "f"),
    ];
    let mut h = harness(
        vec![
            ScriptedResponse::tool_use("toolu_1", "echo", json!({ "text": "y" })),
            ScriptedResponse::text("Listo."),
        ],
        old.clone(),
    );
    let outcome = h.run("hola").await.unwrap();
    assert_eq!(outcome.stop_reason, StopReason::EndTurn);
    let requests = h.llm.requests();
    assert_eq!(requests.len(), 2);
    assert!(requests.iter().all(|r| r.model.role == ModelRole::Main));
    assert_eq!(&h.session.turns[..3], old.as_slice());
    assert!(notices(&h).is_empty());
}

#[tokio::test]
async fn a_failed_summary_is_a_warning_and_the_turn_goes_on() {
    let old = vec![text_turn("a", "b"), text_turn("c", "d")];
    let mut h = harness(
        vec![
            ScriptedResponse::tool_use("toolu_1", "echo", json!({ "text": "y" }))
                .with_usage(big_usage()),
            ScriptedResponse::open_error(LlmError::Overloaded),
            ScriptedResponse::text("Listo."),
        ],
        old.clone(),
    );
    let outcome = h.run("hola").await.unwrap();
    assert_eq!(outcome.stop_reason, StopReason::EndTurn);
    assert_eq!(&h.session.turns[..2], old.as_slice());
    let notices = notices(&h);
    assert_eq!(notices.len(), 1);
    assert_eq!(notices[0].0, NoticeLevel::Warning);
    assert!(notices[0].1.contains("could not compact"));
}

#[tokio::test]
async fn cancelling_during_the_summary_interrupts_the_turn_and_keeps_the_history() {
    let old = vec![text_turn("a", "b"), text_turn("c", "d")];
    let mut h = harness(
        vec![
            ScriptedResponse::tool_use("toolu_1", "echo", json!({ "text": "y" }))
                .with_usage(big_usage()),
            ScriptedResponse::text("resumen").hang_after(0),
        ],
        old.clone(),
    );
    let token = CancelToken::new();
    let opts = TurnOptions::default().with_cancel(token.clone());
    let llm = Arc::clone(&h.llm);
    let outcome = {
        let Harness {
            service, session, ..
        } = &mut h;
        let mut turn = std::pin::pin!(service.run_turn(session, "hola", opts));
        loop {
            futures::select_biased! {
                outcome = turn.as_mut().fuse() => panic!("finished early: {outcome:?}"),
                () = tokio::task::yield_now().fuse() => {}
            }
            if llm.requests().len() == 2 && llm.open_streams() == 1 {
                break;
            }
        }
        token.cancel();
        turn.await.unwrap()
    };
    assert_eq!(outcome.stop_reason, StopReason::Interrupted);
    assert_eq!(llm.open_streams(), 0);
    assert_eq!(&h.session.turns[..2], old.as_slice());
    assert_tool_pairs(&h.session.history().cloned().collect::<Vec<_>>());
}
