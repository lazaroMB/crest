use std::collections::BTreeMap;

use domain::computed::{ComputedValue, Engine};
use serde_json::json;

use super::*;

fn bundle(value: Value) -> MimeBundle {
    MimeBundle::from_value(&value).unwrap_or_else(|| panic!("not a bundle: {value}"))
}

fn node(view: &MimeView) -> &CustomNode {
    match &view.kind {
        Drawn::Node(node) => node,
        other => panic!("{other:?}"),
    }
}

fn envelope_value() -> Value {
    let mut envelope = Computed::new(
        "ext_1",
        "units.convert",
        "unit conversion",
        Engine {
            name: "crest-calc".into(),
            version: "1".into(),
            backend: None,
            platform: None,
        },
    );
    envelope
        .values
        .insert("to".into(), ComputedValue::new(25.4));
    envelope.verification.matched = Some(true);
    json!({
        "envelope": envelope,
        "body": {"type": "quantity", "from": {"value": 1.0, "unit": "in"},
                 "to": {"value": 25.4, "unit": "mm"}, "factor": 25.4, "exact": true,
                 "dimension": "length"}
    })
}

#[test]
fn the_crest_envelope_wins_and_is_marked_as_the_tools() {
    let view = render(
        &bundle(json!({"application/vnd.crest.computed+json": envelope_value(), "text/plain": "25.4 mm"})),
        "mcp__units__convert",
    )
    .unwrap();
    assert_eq!(view.media_type, types::CREST_COMPUTED);
    let Drawn::Computed { envelope, .. } = &view.kind else {
        panic!("{view:?}")
    };
    assert_eq!(
        envelope.verification.matched, None,
        "the claimed match is cleared"
    );
    assert_eq!(envelope.warnings[0].code, EXTERNAL_WARNING);
    assert!(envelope.warnings[0].text.contains("mcp__units__convert"));
    assert!(matches!(view.block("x"), BlockKind::Computed { .. }));
    assert!(matches!(view.node(), CustomNode::Table(_)));
}

#[test]
fn a_bad_envelope_falls_through_with_a_note() {
    let mut bad = envelope_value();
    bad["envelope"]["crest_computed"] = json!("9.0");
    let view = render(
        &bundle(json!({"application/vnd.crest.computed+json": bad, "text/markdown": "**ok**"})),
        "t",
    )
    .unwrap();
    assert_eq!(view.media_type, types::MARKDOWN);
    assert!(
        view.notes[0].starts_with("application/vnd.crest.computed+json not drawn"),
        "{:?}",
        view.notes
    );
    let BlockKind::Custom { name, tree, .. } = view.block("tool") else {
        panic!()
    };
    assert_eq!(name, "text/markdown · tool");
    assert!(
        matches!(tree, CustomNode::Col { .. }),
        "notes under the drawing"
    );
}

#[test]
fn vega_lite_charts_or_json_with_a_note() {
    let spec = json!({"data": {"values": [{"a": "x", "b": 1}]}, "mark": "bar",
        "encoding": {"x": {"field": "a", "type": "nominal"}, "y": {"field": "b", "type": "quantitative"}}});
    let view = render(
        &bundle(json!({"application/vnd.vegalite.v5+json": spec, "text/plain": "<Chart>"})),
        "t",
    )
    .unwrap();
    assert!(matches!(node(&view), CustomNode::Chart(_)));
    let layered = json!({"layer": [], "data": {"values": []}});
    let view = render(
        &bundle(json!({"application/vnd.vegalite.v5+json": layered, "application/json": {"a": 1}})),
        "t",
    )
    .unwrap();
    assert_eq!(view.media_type, types::JSON);
    assert!(view.notes[0].contains("`layer` views"), "{:?}", view.notes);
}

#[test]
fn images_text_latex_and_html() {
    let png = render(
        &bundle(json!({"image/png": "iVBORw0K\nGgo=", "text/plain": "x"})),
        "t",
    )
    .unwrap();
    assert!(
        matches!(node(&png), CustomNode::Image { source: ImageData::Base64 { data }, .. } if data == "iVBORw0KGgo=")
    );
    let not_b64 = render(&bundle(json!({"image/png": "%%%", "text/plain": "x"})), "t").unwrap();
    assert_eq!(not_b64.media_type, types::PLAIN);
    let svg = render(
        &bundle(json!({"image/svg+xml": ["<svg xmlns='http://www.w3.org/2000/svg'>", "</svg>"]})),
        "t",
    )
    .unwrap();
    assert_eq!(svg.media_type, types::SVG);
    let leaky = render(
        &bundle(
            json!({"image/svg+xml": "<svg><image href='/etc/x.png'/></svg>", "text/plain": "x"}),
        ),
        "t",
    )
    .unwrap();
    assert_eq!(leaky.media_type, types::PLAIN);
    let tex = render(&bundle(json!({"text/latex": "\\frac{a}{b}"})), "t").unwrap();
    assert_eq!(
        node(&tex),
        &CustomNode::Markdown {
            source: "$$\n\\frac{a}{b}\n$$".into()
        }
    );
    let inline = render(&bundle(json!({"text/latex": "$x^2$"})), "t").unwrap();
    assert_eq!(
        node(&inline),
        &CustomNode::Markdown {
            source: "$x^2$".into()
        }
    );
    let html = render(
        &bundle(json!({"text/html": "<script>alert(1)</script><b>x</b>"})),
        "t",
    )
    .unwrap();
    let mut code = None;
    node(&html).for_each(&mut |n| {
        if let CustomNode::Code { lang, source } = n {
            code = Some((lang.clone(), source.clone()));
        }
    });
    assert_eq!(
        code,
        Some((
            Some("html".into()),
            "<script>alert(1)</script><b>x</b>".into()
        ))
    );
    let mut has_html = false;
    node(&html).for_each(&mut |n| has_html |= matches!(n, CustomNode::Html { .. }));
    assert!(!has_html, "never rendered as HTML");
}

#[test]
fn cards_unknown_types_and_limits() {
    let a2ui = json!([
        {"version": "v0.9", "createSurface": {"surfaceId": "s", "catalogId": "basic"}},
        {"version": "v0.9", "updateComponents": {"surfaceId": "s", "components": [{"id": "root", "component": "Text", "text": "Hi"}]}}
    ]);
    let view = render(&bundle(json!({"application/a2ui+json": a2ui})), "t").unwrap();
    assert_eq!(view.media_type, types::A2UI);
    let card = json!({"type": "AdaptiveCard", "version": "1.5", "body": [{"type": "TextBlock", "text": "Hi"}]});
    let view = render(
        &bundle(json!({"application/vnd.microsoft.card.adaptive": card})),
        "t",
    )
    .unwrap();
    assert_eq!(view.media_type, types::ADAPTIVE_CARD);
    let view = render(
        &bundle(json!({"application/vnd.plotly.v1+json": {"data": []}})),
        "t",
    )
    .unwrap();
    assert!(matches!(node(&view), CustomNode::Placeholder { .. }));
    let long = "a".repeat(MAX_DOCUMENT + 10);
    let view = render(&bundle(json!({"text/markdown": long})), "t").unwrap();
    let CustomNode::Markdown { source } = node(&view) else {
        panic!()
    };
    assert!(source.len() < MAX_DOCUMENT + 100 && source.ends_with("KiB)"));
    let empty = MimeBundle {
        data: BTreeMap::new(),
        metadata: BTreeMap::new(),
    };
    assert!(render(&empty, "t").is_err());
}
