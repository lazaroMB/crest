//! PostgreSQL EXPLAIN plans on fixture files.

use adapter_formats::Formats;
use application::ports::formats::FormatsPort;

fn fixture(name: &str) -> Vec<u8> {
    let path = format!(
        "{}/tests/fixtures/devops/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

#[test]
fn explain_analyze_buffers_gives_self_times_misestimates_and_spills() {
    let spec = Formats
        .parse_query_plan(&fixture("explain-analyze.json"), Some("plans/q.json"))
        .unwrap_or_else(|e| panic!("{e}"));
    spec.validate().unwrap();
    assert!(spec.analyzed && spec.buffers);
    assert_eq!(spec.planning_time, Some(0.42));
    assert_eq!(spec.execution_time, Some(191.3));
    let nodes: Vec<_> = spec.nodes.iter().map(|n| (n.label(), n.depth)).collect();
    assert_eq!(
        nodes,
        [
            ("Sort".to_owned(), 0),
            ("Aggregate".to_owned(), 1),
            ("Hash Join".to_owned(), 2),
            ("Seq Scan on orders o".to_owned(), 3),
            ("Hash".to_owned(), 3),
            ("Seq Scan on customers c".to_owned(), 4),
        ]
    );
    let selfs: Vec<f64> = spec
        .nodes
        .iter()
        .map(|n| (n.self_time.unwrap_or(-1.0) * 1000.0).round() / 1000.0)
        .collect();
    assert_eq!(selfs, [20.004, 20.0, 51.0, 95.0, 2.0, 2.0]);
    assert_eq!(spec.slowest(3), [3, 2, 0]);
    let worst = spec.misestimates(10.0, 5);
    let worst: Vec<_> = worst
        .iter()
        .map(|m| (m.node, m.factor.round(), m.under))
        .collect();
    assert_eq!(worst, [(0, 120.0, true), (1, 120.0, true), (2, 96.0, true)]);
    assert!(
        spec.nodes[0]
            .warnings
            .contains(&"sort spilled to disk (2048 kB)".to_owned()),
        "{:?}",
        spec.nodes[0].warnings
    );
    assert_eq!(
        spec.nodes[2].condition.as_deref(),
        Some("hash cond: o.customer_id = c.id")
    );
    assert_eq!(
        spec.nodes[0]
            .attrs
            .get("Sort Method")
            .and_then(|v| v.as_str()),
        Some("external merge")
    );
    let buffers = spec.nodes[3].buffers.unwrap_or_default();
    assert_eq!((buffers.shared_hit, buffers.shared_read), (0, 4800));
    let tree = spec.to_tree();
    tree.validate().unwrap();
    assert!(tree.columns.iter().any(|c| c.name == "temp"));
}

#[test]
fn psql_output_is_unwrapped_and_estimates_alone_work() {
    let spec = Formats
        .parse_query_plan(&fixture("explain-psql.txt"), None)
        .unwrap_or_else(|e| panic!("{e}"));
    spec.validate().unwrap();
    assert!(!spec.analyzed);
    assert_eq!(spec.nodes.len(), 1);
    // The alias equal to the table name is dropped.
    assert_eq!(
        spec.nodes[0].label(),
        "Index Scan using users_pkey on users"
    );
    assert_eq!(
        spec.nodes[0].condition.as_deref(),
        Some("index cond: id = 42")
    );
    assert_eq!(spec.nodes[0].self_time, None);
    let tree = spec.to_tree();
    let names: Vec<_> = tree.columns.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["est. rows", "cost", "note"]);
}

#[test]
fn client_exports_and_bad_input() {
    let row =
        br#"[{"QUERY PLAN": "[{\"Plan\": {\"Node Type\": \"Result\", \"Total Cost\": 0.01}}]"}]"#;
    let spec = Formats
        .parse_query_plan(row, None)
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(spec.nodes[0].node_type, "Result");
    let text = Formats
        .parse_query_plan(&fixture("explain.txt"), Some("q.txt"))
        .unwrap_err()
        .to_string();
    assert!(text.contains("FORMAT JSON"), "{text}");
    let none = Formats
        .parse_query_plan(br#"{"rows": []}"#, None)
        .unwrap_err()
        .to_string();
    assert!(none.contains("no \"Plan\""), "{none}");
}
