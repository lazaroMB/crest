use std::collections::BTreeMap;

use domain::conversation::{SessionId, ToolUseId, TurnId};
use domain::form::FormState;
use domain::interop::INTEROP_PLUGIN;
use serde_json::json;

use super::*;
use crate::ports::form::FormResponse;
use crate::testing::form::ScriptedForm;
use crate::tools::registry::{Tool, Typed};

const BOOKING: &str = include_str!("../../../tests/fixtures/interop/a2ui_booking.jsonl");

fn ctx() -> ToolContext {
    ToolContext::new(
        SessionId::new(),
        TurnId::new(),
        ToolUseId::new("toolu_card"),
    )
}

fn tool(port: &Arc<ScriptedForm>) -> Typed<ShowCardTool> {
    Typed(ShowCardTool::new(Arc::clone(port) as Arc<dyn FormPort>))
}

#[test]
fn definition() {
    let tool = tool(&Arc::new(ScriptedForm::default()));
    insta::assert_json_snapshot!(
        "show_card_definition",
        json!({
            "name": tool.name(),
            "description": tool.description(),
            "input_schema": tool.input_schema(),
        })
    );
    assert_eq!(
        tool.effect(&json!({"format": "a2ui", "card": BOOKING})),
        ToolEffect::Read
    );
}

#[tokio::test]
async fn a_card_with_buttons_waits_and_returns_the_a2ui_action() {
    let port = Arc::new(ScriptedForm::new([FormResponse::Submitted {
        answer: serde_json::from_value(json!({"booking.name": "Ada", "booking.guests": 3}))
            .unwrap(),
        action: Some("1:book".into()),
    }]));
    let input = json!({"format": "a2ui", "card": BOOKING, "title": "Table"});
    let call = tool(&port).prepare(input, &ctx()).await.unwrap();
    let Some(BlockKind::Form(pending)) = &call.block else {
        panic!("{call:?}");
    };
    assert_eq!(pending.title, "Table");
    assert_eq!(pending.actions.len(), 2);
    assert!(pending.body.is_some());
    let output = call.run.await.unwrap();
    assert!(!output.is_error, "{}", output.text_content());
    let text = output.text_content();
    assert!(text.contains("\"pressed\": \"Book\""), "{text}");
    assert!(text.contains("\"who\": \"Ada\""), "{text}");
    assert!(text.contains("\"guests\": 3"), "{text}");
    let Some(BlockKind::Form(done)) = &output.block_update else {
        panic!("{output:?}");
    };
    assert_eq!(done.state, FormState::Submitted);
    assert_eq!(done.action.as_deref(), Some("1:book"));
}

#[tokio::test]
async fn a_bad_date_is_asked_again_and_an_unknown_action_rejected() {
    let bad_date = FormResponse::Submitted {
        answer: BTreeMap::from([
            ("booking.name".to_owned(), json!("Ada")),
            ("booking.date".to_owned(), json!("soon")),
        ]),
        action: Some("1:book".into()),
    };
    let bad_action = FormResponse::Submitted {
        answer: BTreeMap::from([("booking.name".to_owned(), json!("Ada"))]),
        action: Some("9:nope".into()),
    };
    let port = Arc::new(ScriptedForm::new([
        bad_date,
        bad_action,
        FormResponse::cancelled("closed"),
    ]));
    let call = tool(&port)
        .prepare(json!({"format": "a2ui", "card": BOOKING}), &ctx())
        .await
        .unwrap();
    let output = call.run.await.unwrap();
    assert!(output.is_error);
    let requests = port.requests();
    assert_eq!(requests.len(), 3);
    assert!(
        requests[1].error.as_deref().unwrap().contains("date"),
        "{:?}",
        requests[1].error
    );
    assert!(
        requests[2]
            .error
            .as_deref()
            .unwrap()
            .contains("unknown action"),
        "{:?}",
        requests[2].error
    );
}

#[tokio::test]
async fn a_card_without_buttons_is_shown_as_a_block() {
    let port = Arc::new(ScriptedForm::default());
    let card = json!([
        {"version": "v0.9", "createSurface": {"surfaceId": "s", "catalogId": "basic"}},
        {"version": "v0.9", "updateComponents": {"surfaceId": "s", "components": [
            {"id": "root", "component": "Column", "children": ["t", "v"]},
            {"id": "t", "component": "Text", "text": "Hi"},
            {"id": "v", "component": "Hologram"}
        ]}}
    ]);
    let call = tool(&port)
        .prepare(json!({"format": "a2ui", "card": card}), &ctx())
        .await
        .unwrap();
    assert!(call.block.is_none());
    let output = call.run.await.unwrap();
    let text = output.text_content();
    assert!(text.contains("1 not drawn"), "{text}");
    let Some(BlockKind::Custom { plugin, tree, .. }) = &output.block_update else {
        panic!("{output:?}");
    };
    assert_eq!(plugin, INTEROP_PLUGIN);
    assert_eq!(tree.placeholder_count(), 1);
    assert!(port.requests().is_empty());
}

#[tokio::test]
async fn invalid_cards_are_input_errors() {
    let port = Arc::new(ScriptedForm::default());
    for card in [
        json!("{ nope"),
        json!([{"version": "v0.9", "deleteSurface": {"surfaceId": "s"}}]),
    ] {
        let err = tool(&port)
            .prepare(json!({"format": "a2ui", "card": card}), &ctx())
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput { .. }), "{err:?}");
    }
}

const EXPENSE: &str = include_str!("../../../tests/fixtures/interop/adaptive_expense.json");

#[tokio::test]
async fn an_adaptive_card_executes_with_its_verb_and_data() {
    let port = Arc::new(ScriptedForm::new([FormResponse::Submitted {
        answer: serde_json::from_value(json!({"code": "CC-0042", "notify": true})).unwrap(),
        action: Some("1".into()),
    }]));
    let card: Value = serde_json::from_str(EXPENSE).unwrap();
    let call = tool(&port)
        .prepare(json!({"format": "adaptive_card", "card": card}), &ctx())
        .await
        .unwrap();
    let Some(BlockKind::Form(pending)) = &call.block else {
        panic!("{call:?}");
    };
    assert_eq!(pending.title, "Expense report #42");
    let output = call.run.await.unwrap();
    let text = output.text_content();
    assert!(text.contains("\"verb\": \"approve\""), "{text}");
    assert!(text.contains("\"notify\": \"yes\""), "{text}");
    assert!(text.contains("\"requestId\": 42"), "{text}");
}
