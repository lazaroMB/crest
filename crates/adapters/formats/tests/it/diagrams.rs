//! DOT graphs and ER diagrams from DDL on fixture files.

use adapter_formats::Formats;
use application::ports::formats::FormatsPort;
use domain::devops::diagram::DiagramSource;
use domain::widget::graph::FlowDirection;

fn fixture(name: &str) -> String {
    let path = format!(
        "{}/tests/fixtures/devops/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

#[test]
fn dot_nodes_edges_clusters_and_attributes() {
    let spec = Formats
        .parse_dot(&fixture("deps.dot"), Some("deps.dot"))
        .unwrap_or_else(|e| panic!("{e}"));
    spec.validate().unwrap();
    assert_eq!(spec.source, DiagramSource::Dot);
    let g = &spec.graph;
    assert!(g.directed);
    assert_eq!(g.direction, FlowDirection::Right);
    assert_eq!(g.title.as_deref(), Some("crate graph"));
    let nodes: Vec<_> = g
        .nodes
        .iter()
        .map(|n| {
            (
                n.id.as_str(),
                n.text().to_owned(),
                n.group.as_deref(),
                n.kind.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        nodes,
        [
            (
                "domain",
                "domain 0.14".to_owned(),
                Some("cluster_core"),
                Some("gray")
            ),
            (
                "application",
                "app".to_owned(),
                Some("cluster_core"),
                Some("blue")
            ),
            (
                "adapter-formats",
                "adapter-formats".to_owned(),
                Some("cluster_edge"),
                Some("gray")
            ),
            (
                "adapter-lua",
                "adapter-lua".to_owned(),
                Some("cluster_edge"),
                Some("blue")
            ),
            ("ui", "ui".to_owned(), Some("cluster_inner"), Some("gray")),
            ("bin", "crest bin".to_owned(), None, Some("gray")),
        ]
    );
    let edges: Vec<_> = g
        .edges
        .iter()
        .map(|e| (e.from.as_str(), e.to.as_str()))
        .collect();
    assert_eq!(
        edges,
        [
            ("application", "domain"),
            ("adapter-formats", "application"),
            ("adapter-formats", "domain"),
            ("adapter-lua", "application"),
            ("application", "domain"),
            ("bin", "ui"),
            ("ui", "application"),
            ("ui", "domain"),
        ]
    );
    assert_eq!(g.edges[0].label.as_deref(), Some("uses"));
    assert_eq!(g.edges[0].weight, Some(2.0));
    assert_eq!(g.edges[7].weight, Some(3.0));
    let groups: Vec<_> = g
        .groups
        .iter()
        .map(|gr| (gr.id.as_str(), gr.label.as_deref()))
        .collect();
    assert_eq!(
        groups,
        [
            ("cluster_core", Some("core layer")),
            ("cluster_edge", Some("adapters")),
            ("cluster_inner", Some("ui")),
        ]
    );
    assert_eq!(
        g.nodes[0].attrs.get("shape").and_then(|v| v.as_str()),
        Some("box")
    );
}

#[test]
fn undirected_graphs_and_errors() {
    let spec = Formats
        .parse_dot("strict graph { a -- b -- c; a -- c }", None)
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(!spec.graph.directed);
    assert_eq!(spec.graph.edges.len(), 3);
    let err = Formats
        .parse_dot("flowchart TD\n a --> b", None)
        .unwrap_err();
    assert!(err.to_string().contains("not a DOT graph"), "{err}");
    let err = Formats
        .parse_dot("digraph { a -> ", Some("x.dot"))
        .unwrap_err();
    assert!(err.to_string().contains("x.dot"), "{err}");
}

#[test]
fn ddl_tables_keys_and_foreign_keys() {
    let spec = Formats
        .parse_ddl(
            &fixture("schema.sql"),
            Some("postgresql"),
            Some("db/schema.sql"),
        )
        .unwrap_or_else(|e| panic!("{e}"));
    spec.validate().unwrap();
    assert_eq!(spec.dialect.as_deref(), Some("postgresql"));
    let names: Vec<_> = spec.tables.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(
        names,
        ["public.users", "orders", "order_items", "audit_log"]
    );
    let users = &spec.tables[0];
    assert_eq!(users.primary_key(), ["id"]);
    assert!(users.columns[1].unique && !users.columns[1].nullable);
    let orders = &spec.tables[1];
    assert_eq!(orders.primary_key(), ["id"]);
    assert_eq!(
        orders.columns[1].references.as_deref(),
        Some("public.users.id")
    );
    let items = &spec.tables[2];
    assert_eq!(items.primary_key(), ["order_id", "sku"]);
    assert_eq!(items.columns[0].references.as_deref(), Some("orders.id"));
    let edges: Vec<_> = spec
        .graph
        .edges
        .iter()
        .map(|e| {
            (
                e.from.as_str(),
                e.to.as_str(),
                e.label.as_deref().unwrap_or(""),
            )
        })
        .collect();
    assert_eq!(
        edges,
        [
            ("orders", "public.users", "user_id → id"),
            (
                "order_items",
                "orders",
                "order_id → id (order_items_order_fk)"
            ),
        ]
    );
    assert_eq!(spec.isolated_tables(), ["audit_log"]);
    assert_eq!(spec.graph.nodes[0].group.as_deref(), Some("public"));
    assert_eq!(spec.warnings.len(), 1, "{:?}", spec.warnings);
    assert!(
        spec.warnings[0].starts_with("skipped `CREATE TABLE broken"),
        "{:?}",
        spec.warnings
    );
    assert!(spec.to_mermaid().contains("orders }o--|| public_users"));
}

#[test]
fn ddl_errors() {
    let err = Formats.parse_ddl("select 1;", None, None).unwrap_err();
    assert!(err.to_string().contains("no CREATE TABLE"), "{err}");
    let err = Formats
        .parse_ddl("create table a (x int);", Some("cobol"), None)
        .unwrap_err();
    assert!(err.to_string().contains("unknown SQL dialect"), "{err}");
}
