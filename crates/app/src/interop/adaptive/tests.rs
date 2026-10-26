use serde_json::json;

use super::*;
use crate::interop::plain_text;

const EXPENSE: &str = include_str!("../../../tests/fixtures/interop/adaptive_expense.json");

fn expense(interactive: bool) -> Rendered {
    let card: Value = serde_json::from_str(EXPENSE).unwrap();
    map(&card, interactive).unwrap_or_else(|e| panic!("{e}"))
}

fn answer(value: Value) -> BTreeMap<String, Value> {
    serde_json::from_value(value).unwrap()
}

#[test]
fn elements_map_onto_nodes() {
    let rendered = expense(true);
    let card = &rendered.card;
    assert_eq!(card.title.as_deref(), Some("Expense report #42"));
    let text = plain_text(&card.body);
    for needle in [
        "### Expense report #42",
        "Submitted by **Ada** for the Lisbon trip.",
        "[table: 2 columns, 2 rows]",
        "Policy: within limits",
        "Image: receipt (opens in the browser) <https://cdn.example.test/receipt.png>",
        "policy <https://intranet.example.test/policy>",
        "Receipts were scanned on 2026-10-20.",
        "Rated 4 of 5",
        "Open in the app <https://expenses.example.test/42>",
        "## Ask a question",
    ] {
        assert!(text.contains(needle), "{needle:?} not in\n{text}");
    }
    assert!(!text.contains("never shown"), "{text}");
    assert_eq!(card.body.placeholder_count(), 1, "the Carousel");
    let mut tables = Vec::new();
    card.body.for_each(&mut |n| {
        if let CustomNode::Table(t) = n {
            tables.push(t.columns.iter().map(|c| c.name.clone()).collect::<Vec<_>>());
        }
    });
    assert_eq!(tables, [vec!["Fact", "Value"], vec!["Item", "Amount"]]);
    assert!(
        card.notes
            .iter()
            .any(|n| n.contains("\"Chart.Pie\" was left out")),
        "{:?}",
        card.notes
    );
    assert!(
        card.notes.iter().any(|n| n.contains("toggle buttons")),
        "{:?}",
        card.notes
    );
    let form = card.form(None);
    assert_eq!(form.validate(), Ok(()));
    assert_eq!(form.title, "Expense report #42");
}

#[test]
fn inputs_become_fields_and_actions_buttons() {
    let card = expense(true).card;
    let names: Vec<&str> = card.fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "note", "code", "approved", "pay_on", "pay_at", "notify", "tags", "priority",
            "question"
        ]
    );
    assert!(matches!(
        card.fields[0].kind,
        FieldKind::Multiline {
            max_length: Some(200),
            ..
        }
    ));
    assert!(card.fields[1].required);
    assert_eq!(
        card.fields[2].kind,
        FieldKind::Number {
            default: Some(1240.5),
            min: Some(0.0),
            max: Some(1240.5)
        }
    );
    assert_eq!(
        card.fields[5].kind,
        FieldKind::Boolean {
            default: Some(true)
        }
    );
    assert_eq!(card.fields[5].label.as_deref(), Some("Notify Ada"));
    assert!(
        matches!(&card.fields[6].kind, FieldKind::MultiEnum { default, .. } if default == &["travel", "food"])
    );
    assert!(matches!(
        &card.fields[7].kind,
        FieldKind::Enum { default: None, .. }
    ));
    let actions: Vec<(&str, &str, bool)> = card
        .actions
        .iter()
        .map(|a| (a.id.as_str(), a.label.as_str(), a.primary))
        .collect();
    assert_eq!(
        actions,
        [
            ("1", "Approve", true),
            ("2", "Reject", false),
            ("3", "Send question", false)
        ]
    );
}

#[test]
fn execute_merges_inputs_into_data_and_none_leaves_them_out() {
    let rendered = expense(true);
    let given = answer(json!({
        "code": "CC-1234", "approved": 1000, "notify": false, "tags": ["food", "other"],
        "pay_on": "2026-11-15"
    }));
    assert_eq!(rendered.check(&given, Some("1")), Ok(()));
    assert_eq!(
        rendered.response(&given, Some("1")),
        json!({
            "type": "Action.Execute",
            "verb": "approve",
            "data": {
                "requestId": 42, "code": "CC-1234", "approved": 1000, "notify": "no",
                "tags": "food,other", "pay_on": "2026-11-15"
            }
        })
    );
    assert_eq!(
        rendered.response(&given, Some("2")),
        json!({"type": "Action.Submit", "data": {"decision": "reject"}})
    );
}

#[test]
fn patterns_dates_and_times_are_checked() {
    let rendered = expense(true);
    let bad_code = answer(json!({"code": "1234"}));
    assert_eq!(rendered.check(&bad_code, None), Err("Use CC-1234".into()));
    let late = answer(json!({"code": "CC-0001", "pay_on": "2027-01-02"}));
    assert!(rendered.check(&late, None).unwrap_err().contains("pay_on"));
    let bad_time = answer(json!({"code": "CC-0001", "pay_at": "25:00"}));
    assert!(
        rendered
            .check(&bad_time, None)
            .unwrap_err()
            .contains("pay_at")
    );
}

#[test]
fn read_only_cards_keep_inputs_as_text() {
    let card = expense(false).card;
    assert!(card.fields.is_empty() && card.actions.is_empty());
    let text = plain_text(&card.body);
    assert!(text.contains("Approved amount: 1240.5"), "{text}");
    assert!(text.contains("[button: Approve]"), "{text}");
    assert!(card.notes.iter().any(|n| n.contains("buttons do nothing")));
}

#[test]
fn attachments_text_and_versions_are_read() {
    let attachment = json!({
        "contentType": "application/vnd.microsoft.card.adaptive",
        "content": {"type": "AdaptiveCard", "version": "1.7", "body": [{"type": "TextBlock", "text": "hi"}]}
    });
    let card = map(&attachment, true).unwrap().card;
    assert!(
        card.notes.iter().any(|n| n.contains("newer than 1.6")),
        "{:?}",
        card.notes
    );
    let text = json!(r#"{"type": "AdaptiveCard", "body": []}"#);
    let card = map(&text, true).unwrap().card;
    assert!(card.notes.iter().any(|n| n.contains("no version")));
}

#[test]
fn bad_and_hostile_cards() {
    assert!(map(&json!({"type": "MessageCard"}), true).is_err());
    assert!(map(&json!("{ not json"), true).is_err());
    assert!(map(&json!({"body": []}), true).is_err());
    let big = json!({"type": "AdaptiveCard", "body": [{"type": "TextBlock", "text": "x".repeat(MAX_PAYLOAD_BYTES)}]});
    assert!(map(&big, true).is_err());
    let mut deep = json!({"type": "TextBlock", "text": "bottom"});
    for _ in 0..100 {
        deep = json!({"type": "Container", "items": [deep]});
    }
    let card = map(
        &json!({"type": "AdaptiveCard", "version": "1.5", "body": [deep]}),
        true,
    )
    .unwrap()
    .card;
    assert!(
        card.notes.iter().any(|n| n.contains("nested deeper")),
        "{:?}",
        card.notes
    );
    let wide: Vec<Value> = (0..3000)
        .map(|i| json!({"type": "TextBlock", "text": i.to_string()}))
        .collect();
    let card = map(
        &json!({"type": "AdaptiveCard", "version": "1.5", "body": wide}),
        true,
    )
    .unwrap()
    .card;
    assert!(
        card.notes.iter().any(|n| n.contains("first 2000")),
        "{:?}",
        card.notes
    );
    let hostile = json!({"type": "AdaptiveCard", "version": "1.5",
        "body": [
            {"type": "Image", "url": "javascript:alert(1)"},
            {"type": "Input.Text", "id": "x", "regex": "(a+)+$"},
            {"type": "Input.Text", "id": "x", "label": "twice"},
            {"type": "Input.Text", "label": "no id"}
        ],
        "actions": [{"type": "Action.OpenUrl", "title": "Run", "url": "javascript:alert(1)"}]
    });
    let rendered = map(&hostile, true).unwrap();
    assert_eq!(rendered.card.tree().validate(), Ok(()));
    assert_eq!(rendered.card.fields.len(), 1);
    assert!(
        rendered
            .card
            .notes
            .iter()
            .any(|n| n.contains("appears twice"))
    );
    assert!(rendered.card.notes.iter().any(|n| n.contains("no id")));
    let slow = answer(json!({"x": format!("{}!", "a".repeat(5000))}));
    assert!(
        rendered.check(&slow, None).is_err(),
        "linear-time regex, no match"
    );
    let mut links = 0;
    rendered
        .card
        .body
        .for_each(&mut |n| links += usize::from(matches!(n, CustomNode::Link { .. })));
    assert_eq!(links, 0);
}
