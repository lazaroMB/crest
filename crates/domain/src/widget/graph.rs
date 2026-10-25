//! A node-link graph: nodes, edges between them and optional groups.
//!
//! DOT files, ER diagrams, CI DAGs, netlists and network graphs are all
//! nodes joined by edges. The spec says which layout draws it
//! ([`LayoutKind`]): layered for flows and dependencies, force for
//! networks without a direction, grid for loose collections. The layouts
//! themselves are pure functions of the spec and the node sizes
//! ([`layout`]), so the UI runs them off its thread and tests check them
//! without a window.

pub mod force;
pub mod layered;
pub mod layout;

use std::collections::{BTreeSet, HashMap};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::{invalid, unique};
use crate::block::BlockError;

/// Nodes a spec may have.
pub const MAX_NODES: usize = 50_000;
/// Edges a spec may have.
pub const MAX_EDGES: usize = 200_000;
/// Nodes laid out and drawn; larger graphs are only summarised.
pub const DRAW_LIMIT: usize = 2_000;

const KIND: &str = "graph";

/// How the nodes are placed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LayoutKind {
    /// Layers in the edges' direction (Sugiyama): flows, dependencies,
    /// plans.
    #[default]
    Layered,
    /// A seeded spring embedding: networks without a direction.
    Force,
    /// Rows and columns in node order (grouped): loose collections.
    Grid,
}

impl LayoutKind {
    /// The name in the spec (`layered`).
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Layered => "layered",
            Self::Force => "force",
            Self::Grid => "grid",
        }
    }
}

/// Which way a layered layout flows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FlowDirection {
    /// Top to bottom.
    #[default]
    Down,
    /// Left to right.
    Right,
}

/// One node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct GraphNode {
    /// Unique id, referenced by edges.
    pub id: String,
    /// Text in the box; the id when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Id of the group (cluster) the node belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// Class of the node (`table`, `service`, `resistor`); nodes of one
    /// kind share a color.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Extra fields shown when the node is focused.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    #[schemars(with = "std::collections::BTreeMap<String, Value>")]
    pub attrs: Map<String, Value>,
}

impl GraphNode {
    /// The text drawn in the box: the label, or the id.
    #[must_use]
    pub fn text(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.id)
    }
}

/// One edge, from node id to node id.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct GraphEdge {
    /// Id of the source node.
    pub from: String,
    /// Id of the target node.
    pub to: String,
    /// Text drawn along the edge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Strength (a count, a cost); thicker edges are heavier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight: Option<f64>,
}

/// A named group of nodes, drawn as a box around them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct GraphGroup {
    /// Id the nodes' `group` refers to.
    pub id: String,
    /// Caption of the box; the id when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

fn yes() -> bool {
    true
}

fn is_true(value: &bool) -> bool {
    *value
}

/// Nodes joined by edges, with a layout.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct GraphSpec {
    /// Caption above the graph.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Edges have arrowheads (true by default).
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub directed: bool,
    /// How the nodes are placed.
    #[serde(default)]
    pub layout: LayoutKind,
    /// Flow of a layered layout.
    #[serde(default)]
    pub direction: FlowDirection,
    /// Seed of the force layout (the same seed gives the same picture).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    /// The nodes.
    pub nodes: Vec<GraphNode>,
    /// The edges.
    #[serde(default)]
    pub edges: Vec<GraphEdge>,
    /// Captions of the groups nodes refer to; a group not listed is
    /// captioned with its id.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<GraphGroup>,
}

impl GraphSpec {
    /// Checks sizes, that node and group ids are set and distinct, that
    /// edges join existing nodes and that weights are finite.
    ///
    /// # Errors
    ///
    /// [`BlockError::Invalid`] naming the first problem.
    pub fn validate(&self) -> Result<(), BlockError> {
        if self.nodes.len() > MAX_NODES {
            return Err(invalid(
                KIND,
                format!("{} nodes; at most {MAX_NODES}", self.nodes.len()),
            ));
        }
        if self.edges.len() > MAX_EDGES {
            return Err(invalid(
                KIND,
                format!("{} edges; at most {MAX_EDGES}", self.edges.len()),
            ));
        }
        if self.nodes.iter().any(|n| n.id.is_empty()) {
            return Err(invalid(KIND, "a node id is empty"));
        }
        unique(KIND, "node id", self.nodes.iter().map(|n| n.id.as_str()))?;
        if self.groups.iter().any(|g| g.id.is_empty()) {
            return Err(invalid(KIND, "a group id is empty"));
        }
        unique(KIND, "group id", self.groups.iter().map(|g| g.id.as_str()))?;
        let index = self.index();
        for (ix, edge) in self.edges.iter().enumerate() {
            for end in [&edge.from, &edge.to] {
                if !index.contains_key(end.as_str()) {
                    return Err(invalid(
                        KIND,
                        format!("edge {} refers to unknown node {end:?}", ix + 1),
                    ));
                }
            }
            if edge.weight.is_some_and(|w| !w.is_finite()) {
                return Err(invalid(
                    KIND,
                    format!("edge {} has a non-finite weight", ix + 1),
                ));
            }
        }
        Ok(())
    }

    /// Node index by id.
    #[must_use]
    pub fn index(&self) -> HashMap<&str, usize> {
        self.nodes
            .iter()
            .enumerate()
            .map(|(ix, node)| (node.id.as_str(), ix))
            .collect()
    }

    /// The edges as `(from, to)` node indices, one per edge (an edge to
    /// an unknown node, which [`Self::validate`] rejects, joins node 0
    /// to itself so indices stay aligned).
    #[must_use]
    pub fn ends(&self) -> Vec<(usize, usize)> {
        let index = self.index();
        let at = |id: &str| index.get(id).copied().unwrap_or(0);
        self.edges
            .iter()
            .map(|edge| (at(&edge.from), at(&edge.to)))
            .collect()
    }

    /// Nodes joined to `node` by an edge either way, without itself.
    #[must_use]
    pub fn neighbours(&self, node: usize) -> BTreeSet<usize> {
        self.ends()
            .into_iter()
            .filter_map(|(a, b)| match (a == node, b == node) {
                (true, false) => Some(b),
                (false, true) => Some(a),
                _ => None,
            })
            .collect()
    }

    /// Caption of group `id`.
    #[must_use]
    pub fn group_label<'a>(&'a self, id: &'a str) -> &'a str {
        self.groups
            .iter()
            .find(|g| g.id == id)
            .and_then(|g| g.label.as_deref())
            .unwrap_or(id)
    }

    /// Ids of the groups nodes are in, in order of first use.
    #[must_use]
    pub fn groups_in_use(&self) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::new();
        for group in self.nodes.iter().filter_map(|n| n.group.as_deref()) {
            if !out.contains(&group) {
                out.push(group);
            }
        }
        out
    }

    /// The graph in Graphviz DOT: nodes with labels, groups as clusters,
    /// edges with labels and weights.
    #[must_use]
    pub fn to_dot(&self) -> String {
        let mut out = String::new();
        let keyword = if self.directed { "digraph" } else { "graph" };
        let arrow = if self.directed { "->" } else { "--" };
        match &self.title {
            Some(title) => out.push_str(&format!("{keyword} {} {{\n", dot_id(title))),
            None => out.push_str(&format!("{keyword} {{\n")),
        }
        if self.direction == FlowDirection::Right {
            out.push_str("  rankdir=LR;\n");
        }
        let node_line = |node: &GraphNode| {
            let mut attrs = Vec::new();
            if let Some(label) = &node.label {
                attrs.push(format!("label={}", dot_id(label)));
            }
            if attrs.is_empty() {
                format!("{};", dot_id(&node.id))
            } else {
                format!("{} [{}];", dot_id(&node.id), attrs.join(", "))
            }
        };
        for (ix, group) in self.groups_in_use().into_iter().enumerate() {
            out.push_str(&format!("  subgraph cluster_{ix} {{\n"));
            out.push_str(&format!("    label={};\n", dot_id(self.group_label(group))));
            for node in self
                .nodes
                .iter()
                .filter(|n| n.group.as_deref() == Some(group))
            {
                out.push_str(&format!("    {}\n", node_line(node)));
            }
            out.push_str("  }\n");
        }
        for node in self.nodes.iter().filter(|n| n.group.is_none()) {
            out.push_str(&format!("  {}\n", node_line(node)));
        }
        for edge in &self.edges {
            let mut attrs = Vec::new();
            if let Some(label) = &edge.label {
                attrs.push(format!("label={}", dot_id(label)));
            }
            if let Some(weight) = edge.weight {
                attrs.push(format!("weight={weight}"));
            }
            let attrs = if attrs.is_empty() {
                String::new()
            } else {
                format!(" [{}]", attrs.join(", "))
            };
            out.push_str(&format!(
                "  {} {arrow} {}{attrs};\n",
                dot_id(&edge.from),
                dot_id(&edge.to)
            ));
        }
        out.push_str("}\n");
        out
    }
}

/// `text` as a quoted DOT id.
fn dot_id(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    pub(super) fn spec(nodes: &[&str], edges: &[(&str, &str)]) -> GraphSpec {
        serde_json::from_value(json!({
            "nodes": nodes.iter().map(|id| json!({ "id": id })).collect::<Vec<_>>(),
            "edges": edges.iter().map(|(a, b)| json!({ "from": a, "to": b })).collect::<Vec<_>>(),
        }))
        .unwrap()
    }

    #[test]
    fn parses_with_defaults() {
        let spec = spec(&["a", "b"], &[("a", "b")]);
        spec.validate().unwrap();
        assert!(spec.directed);
        assert_eq!(spec.layout, LayoutKind::Layered);
        assert_eq!(spec.direction, FlowDirection::Down);
        assert_eq!(spec.ends(), [(0, 1)]);
        assert_eq!(spec.neighbours(1), BTreeSet::from([0]));
        assert_eq!(spec.nodes[0].text(), "a");
    }

    #[test]
    fn rejects_unknown_ends_duplicates_and_bad_weights() {
        let mut bad = spec(&["a"], &[("a", "zz")]);
        let err = bad.validate().unwrap_err().to_string();
        assert!(err.contains("unknown node \"zz\""), "{err}");
        bad = spec(&["a", "a"], &[]);
        assert!(
            bad.validate()
                .unwrap_err()
                .to_string()
                .contains("appears twice")
        );
        bad = spec(&["a"], &[("a", "a")]);
        bad.edges[0].weight = Some(f64::NAN);
        assert!(bad.validate().unwrap_err().to_string().contains("weight"));
        bad = spec(&[""], &[]);
        assert!(bad.validate().unwrap_err().to_string().contains("empty"));
    }

    #[test]
    fn dot_has_clusters_labels_and_weights() {
        let mut spec = spec(&["a", "b", "c"], &[("a", "b"), ("b", "c")]);
        spec.title = Some("deps".into());
        spec.nodes[0].label = Some("A \"one\"".into());
        spec.nodes[0].group = Some("g".into());
        spec.nodes[1].group = Some("g".into());
        spec.groups = vec![GraphGroup {
            id: "g".into(),
            label: Some("core".into()),
        }];
        spec.edges[0].label = Some("uses".into());
        spec.edges[1].weight = Some(2.0);
        spec.direction = FlowDirection::Right;
        assert_eq!(
            spec.to_dot(),
            "digraph \"deps\" {\n  rankdir=LR;\n  subgraph cluster_0 {\n    label=\"core\";\n    \
             \"a\" [label=\"A \\\"one\\\"\"];\n    \"b\";\n  }\n  \"c\";\n  \
             \"a\" -> \"b\" [label=\"uses\"];\n  \"b\" -> \"c\" [weight=2];\n}\n"
        );
        spec.directed = false;
        assert!(spec.to_dot().starts_with("graph \"deps\""));
        assert!(spec.to_dot().contains("\"a\" -- \"b\""));
    }
}
