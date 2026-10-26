use serde_json::json;

use super::*;

const BOOKING: &str = include_str!("../../../tests/fixtures/interop/a2ui_booking.jsonl");
const V08: &str = include_str!("../../../tests/fixtures/interop/a2ui_v08.json");

fn booking(interactive: bool) -> Rendered {
    map(&Value::String(BOOKING.into()), interactive).unwrap_or_else(|e| panic!("{e}"))
}

fn texts(node: &CustomNode) -> Vec<String> {
    plain_text(node).lines().map(str::to_owned).collect()
}

#[test]
fn a_v09_surface_becomes_a_form_with_bound_fields() {
    let rendered = booking(true);
    let card = &rendered.card;
    assert!(card.is_interactive());
    let names: Vec<&str> = card.fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "booking.name",
            "booking.date",
            "booking.time",
            "booking.terrace",
            "booking.guests"
        ]
    );
    assert!(card.fields[0].required);
    assert_eq!(
        card.fields[2].kind,
        FieldKind::Enum {
            options: vec![
                FormOption {
                    value: "20:00".into(),
                    label: Some("8 pm".into())
                },
                FormOption {
                    value: "21:00".into(),
                    label: Some("9 pm".into())
                },
            ],
            default: Some("20:00".into()),
        }
    );
    assert_eq!(
        card.fields[3].kind,
        FieldKind::Boolean {
            default: Some(true)
        }
    );
    assert_eq!(
        card.fields[4].kind,
        FieldKind::Number {
            default: Some(2.0),
            min: Some(1.0),
            max: Some(12.0)
        }
    );
    let labels: Vec<(&str, bool)> = card
        .actions
        .iter()
        .map(|a| (a.label.as_str(), a.primary))
        .collect();
    assert_eq!(labels, [("Book", true), ("Not now", false)]);
    let lines = texts(&card.body);
    assert_eq!(lines[0], "## Book a table");
    assert!(lines.contains(&"Casa Lola".to_owned()), "{lines:?}");
    assert!(lines.contains(&"Gazpacho".to_owned()) && lines.contains(&"Tortilla".to_owned()));
    assert_eq!(card.body.placeholder_count(), 1, "the unknown PieChart");
    let mut images = 0;
    card.body
        .for_each(&mut |n| images += usize::from(matches!(n, CustomNode::Image { .. })));
    assert_eq!(images, 1);
    let form = card.form(None);
    assert_eq!(form.validate(), Ok(()));
    assert_eq!(form.title, "A2UI");
}

#[test]
fn the_answer_comes_back_as_the_action_message() {
    let rendered = booking(true);
    let answer: BTreeMap<String, Value> = serde_json::from_value(json!({
        "booking.name": "Ada", "booking.date": "2026-11-02", "booking.time": "21:00",
        "booking.terrace": false, "booking.guests": 4
    }))
    .unwrap();
    let book = rendered.card.actions[0].id.clone();
    assert_eq!(rendered.check(&answer, Some(&book)), Ok(()));
    let message = rendered.response(&answer, Some(&book));
    assert_eq!(
        message,
        json!({
            "version": "v0.9",
            "action": {
                "name": "book",
                "surfaceId": "booking",
                "sourceComponentId": "book",
                "context": {"who": "Ada", "guests": 4, "source": "card"}
            }
        })
    );
    let mut bad = answer.clone();
    bad.insert("booking.date".into(), json!("next tuesday"));
    assert!(rendered.check(&bad, None).is_err());
}

#[test]
fn read_only_cards_show_inputs_as_values_and_say_buttons_do_nothing() {
    let rendered = booking(false);
    let card = &rendered.card;
    assert!(card.fields.is_empty() && card.actions.is_empty());
    let lines = texts(&card.body);
    assert!(lines.contains(&"Guests: 2".to_owned()), "{lines:?}");
    assert!(lines.contains(&"Time: 20:00".to_owned()), "{lines:?}");
    assert!(lines.contains(&"[button: Book]".to_owned()), "{lines:?}");
    assert!(
        card.notes.iter().any(|n| n.contains("buttons do nothing")),
        "{:?}",
        card.notes
    );
}

#[test]
fn v08_streams_are_normalised() {
    let payload: Value = serde_json::from_str(V08).unwrap();
    let rendered = map(&payload, true).unwrap();
    let card = &rendered.card;
    assert_eq!(texts(&card.body), ["# Hello"]);
    assert_eq!(card.fields.len(), 2);
    assert_eq!(
        card.fields[0].kind,
        FieldKind::String {
            default: Some("Ada".into()),
            min_length: None,
            max_length: None
        }
    );
    assert!(matches!(&card.fields[1].kind, FieldKind::Enum { default: Some(d), .. } if d == "red"));
    assert_eq!(card.actions.len(), 1);
    assert!(card.actions[0].primary);
    let answer = BTreeMap::from([("user.name".to_owned(), json!("Grace"))]);
    let message = rendered.response(&answer, Some(&card.actions[0].id));
    assert_eq!(message["action"]["name"], "submit");
    assert_eq!(message["action"]["context"], json!({"name": "Grace"}));
}

#[test]
fn bad_payloads_are_errors() {
    let big = json!([{"version": "v0.9", "updateDataModel": {"surfaceId": "s", "value": {"x": "a".repeat(MAX_PAYLOAD_BYTES)}}}]);
    assert!(map(&big, true).is_err());
    assert!(map(&json!("not json {"), true).is_err());
    assert!(map(&json!(42), true).is_err());
    assert!(map(&json!([{"hello": 1}]), true).is_err(), "no surface");
    let no_root = json!([{"updateComponents": {"surfaceId": "s", "components": [{"id": "x", "component": "Text", "text": "a"}]}}]);
    let err = map(&no_root, true).unwrap_err();
    assert!(err.0.contains("root"), "{err}");
    let ids: Vec<String> = (0..=MAX_NODES).map(|i| format!("c{i}")).collect();
    let mut many = vec![json!({"id": "root", "component": "Column", "children": ids})];
    many.extend(
        (0..=MAX_NODES).map(|i| json!({"id": format!("c{i}"), "component": "Text", "text": "a"})),
    );
    let too_many = json!([{"updateComponents": {"surfaceId": "s", "components": many}}]);
    let card = map(&too_many, true).unwrap().card;
    assert!(
        card.notes
            .iter()
            .any(|n| n.contains("components of a surface are read")),
        "{:?}",
        card.notes
    );
    assert!(card.components <= MAX_NODES);
}

fn surface(components: Value, data: Value) -> Value {
    json!([
        {"version": "v0.9", "createSurface": {"surfaceId": "s", "catalogId": "basic"}},
        {"version": "v0.9", "updateDataModel": {"surfaceId": "s", "value": data}},
        {"version": "v0.9", "updateComponents": {"surfaceId": "s", "components": components}}
    ])
}

#[test]
fn loops_depth_and_templates_are_bounded() {
    let looped = surface(
        json!([
            {"id": "root", "component": "Column", "children": ["a"]},
            {"id": "a", "component": "Card", "child": "root"}
        ]),
        json!({}),
    );
    let card = map(&looped, true).unwrap().card;
    assert!(
        card.notes.iter().any(|n| n.contains("contains itself")),
        "{:?}",
        card.notes
    );
    let mut chain: Vec<Value> = (0..60)
        .map(|i| json!({"id": format!("n{i}"), "component": "Column", "children": [format!("n{}", i + 1)]}))
        .collect();
    chain.push(json!({"id": "root", "component": "Column", "children": ["n0"]}));
    let card = map(&surface(json!(chain), json!({})), true).unwrap().card;
    assert!(
        card.notes.iter().any(|n| n.contains("nested deeper")),
        "{:?}",
        card.notes
    );
    let items: Vec<Value> = (0..MAX_ITEMS + 50)
        .map(|i| json!({"t": i.to_string()}))
        .collect();
    let listed = surface(
        json!([
            {"id": "root", "component": "List", "children": {"path": "/items", "componentId": "item"}},
            {"id": "item", "component": "Text", "text": {"path": "t"}}
        ]),
        json!({"items": items}),
    );
    let card = map(&listed, true).unwrap().card;
    let CustomNode::Col { children } = &card.body else {
        panic!("{:?}", card.body)
    };
    assert_eq!(children.len(), MAX_ITEMS);
    assert!(
        card.notes.iter().any(|n| n.contains("first 500")),
        "{:?}",
        card.notes
    );
}

#[test]
fn nothing_is_fetched_or_run() {
    let card = map(
        &surface(
            json!([
                {"id": "root", "component": "Column", "children": ["remote", "js", "go", "fn", "vid"]},
                {"id": "remote", "component": "Image", "url": "https://tracker.test/pixel.png"},
                {"id": "js", "component": "Image", "url": "javascript:alert(1)"},
                {"id": "go", "component": "Button", "child": "t", "action": {"functionCall": {"call": "openUrl", "args": {"url": "javascript:alert(1)"}}}},
                {"id": "t", "component": "Text", "text": "Open"},
                {"id": "fn", "component": "Text", "text": {"call": "formatString", "args": {"value": "${/x}"}}},
                {"id": "vid", "component": "Video", "url": "file:///etc/passwd"}
            ]),
            json!({}),
        ),
        true,
    )
    .unwrap()
    .card;
    assert_eq!(card.tree().validate(), Ok(()));
    let mut links = Vec::new();
    card.body.for_each(&mut |n| {
        if let CustomNode::Link { url, .. } = n {
            links.push(url.clone());
        }
    });
    assert_eq!(links, ["https://tracker.test/pixel.png"]);
    assert!(
        card.notes.iter().any(|n| n.contains("not evaluated")),
        "{:?}",
        card.notes
    );
}

#[test]
fn unknown_versions_and_messages_are_noted() {
    let payload = json!([
        {"version": "v2.0", "createSurface": {"surfaceId": "s"}},
        {"version": "v2.0", "updateComponents": {"surfaceId": "s", "components": [{"id": "root", "component": "Text", "text": "hi"}]}},
        {"version": "v2.0", "teleport": {"surfaceId": "s"}}
    ]);
    let card = map(&payload, true).unwrap().card;
    assert!(
        card.notes.iter().any(|n| n.contains("read as v0.9")),
        "{:?}",
        card.notes
    );
    assert!(
        card.notes.iter().any(|n| n.contains("teleport")),
        "{:?}",
        card.notes
    );
    let deleted = json!([
        {"createSurface": {"surfaceId": "s"}},
        {"deleteSurface": {"surfaceId": "s"}}
    ]);
    assert!(map(&deleted, true).is_err());
}
