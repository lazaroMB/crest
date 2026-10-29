//! A network with its metrics (`show_network`).
//!
//! The adapter reads an edge list (or node-link JSON), builds the graph
//! and computes per node the degree, betweenness, closeness and PageRank,
//! the connected component and a community (label propagation), with the
//! whole-graph numbers (density, components, modularity) in a
//! [`Computed`] envelope. [`NetworkSpec::graph`] draws it on the graph
//! widget sized by a chosen metric and coloured by community, and
//! [`NetworkSpec::table`] lists the metrics.

use std::fmt::Write as _;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, json};

use crate::block::BlockError;
use crate::computed::Computed;
use crate::widget::graph::{GraphEdge, GraphNode, GraphSpec, LayoutKind};
use crate::widget::tree::{ColumnKind, TreeColumn, TreeNode, TreeSpec};

/// Most nodes read.
pub const MAX_NODES: usize = 20_000;
/// Most edges read.
pub const MAX_EDGES: usize = 200_000;
/// Rows of the metrics table.
pub const TABLE_ROWS: usize = 2_000;

/// A per-node metric.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NetworkMetric {
    /// Number of edges (in + out when directed).
    #[default]
    Degree,
    /// Share of shortest paths through the node (normalised).
    Betweenness,
    /// Inverse mean distance to the node (Wasserman–Faust scaled).
    Closeness,
    /// PageRank (damping 0.85).
    Pagerank,
}

impl NetworkMetric {
    /// Every metric, in table order.
    pub const ALL: [Self; 4] = [
        Self::Degree,
        Self::Betweenness,
        Self::Closeness,
        Self::Pagerank,
    ];

    /// Its key (`pagerank`).
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Self::Degree => "degree",
            Self::Betweenness => "betweenness",
            Self::Closeness => "closeness",
            Self::Pagerank => "pagerank",
        }
    }

    /// Its name for people (`PageRank`).
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Degree => "degree",
            Self::Betweenness => "betweenness",
            Self::Closeness => "closeness",
            Self::Pagerank => "PageRank",
        }
    }
}

/// One node with its metrics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct NetworkNode {
    /// Unique id.
    pub id: String,
    /// Label shown; the id when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Edges at the node (in + out when directed).
    pub degree: f64,
    /// Incoming edges (directed only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_degree: Option<f64>,
    /// Outgoing edges (directed only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub out_degree: Option<f64>,
    /// Sum of the weights of its edges.
    pub strength: f64,
    /// Normalised betweenness.
    pub betweenness: f64,
    /// Closeness (Wasserman–Faust scaled for disconnected graphs).
    pub closeness: f64,
    /// PageRank.
    pub pagerank: f64,
    /// Connected component (weak for directed graphs), largest first.
    pub component: usize,
    /// Community (label propagation), largest first.
    pub community: usize,
}

impl NetworkNode {
    /// The label, or the id.
    #[must_use]
    pub fn text(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.id)
    }

    /// The value of `metric`.
    #[must_use]
    pub fn metric(&self, metric: NetworkMetric) -> f64 {
        match metric {
            NetworkMetric::Degree => self.degree,
            NetworkMetric::Betweenness => self.betweenness,
            NetworkMetric::Closeness => self.closeness,
            NetworkMetric::Pagerank => self.pagerank,
        }
    }
}

/// One edge (duplicates merged, weights summed).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct NetworkEdge {
    /// Source node id.
    pub from: String,
    /// Target node id.
    pub to: String,
    /// Weight, when the input had weights.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight: Option<f64>,
}

/// A network with its metrics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct NetworkSpec {
    /// Title shown above the graph.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The workspace file read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Whether edges have a direction.
    pub directed: bool,
    /// Whether edges carry weights (used by PageRank and communities).
    pub weighted: bool,
    /// The nodes in input order.
    pub nodes: Vec<NetworkNode>,
    /// The edges.
    pub edges: Vec<NetworkEdge>,
    /// Connected components (weak when directed).
    pub components: usize,
    /// Strongly connected components (directed only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strong_components: Option<usize>,
    /// Communities found.
    pub communities: usize,
    /// Modularity of the communities.
    pub modularity: f64,
    /// Sources betweenness was sampled from, when not exact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub betweenness_sources: Option<usize>,
    /// Seed of the community search (and sampling, and layout).
    pub seed: u64,
    /// The metric nodes are sized by at first.
    #[serde(default)]
    pub size_by: NetworkMetric,
    /// The metrics as an envelope.
    pub metrics: Computed,
    /// Reader notes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

fn invalid(reason: impl Into<String>) -> BlockError {
    BlockError::Invalid {
        kind: "network",
        reason: reason.into(),
    }
}

/// `v` with up to 4 significant digits.
#[must_use]
pub fn metric_text(v: f64) -> String {
    if v == 0.0 || !v.is_finite() {
        return "0".into();
    }
    if v.fract() == 0.0 && v.abs() < 1e9 {
        return format!("{v:.0}");
    }
    let digits = (3 - v.abs().log10().floor() as i32).clamp(0, 10) as usize;
    format!("{v:.digits$}")
}

impl NetworkSpec {
    /// Checks sizes, ids, edges and metrics.
    ///
    /// # Errors
    ///
    /// [`BlockError::Invalid`] naming the first problem.
    pub fn validate(&self) -> Result<(), BlockError> {
        if self.nodes.is_empty() {
            return Err(invalid("the network has no nodes"));
        }
        if self.nodes.len() > MAX_NODES || self.edges.len() > MAX_EDGES {
            return Err(invalid(format!(
                "{} nodes and {} edges; at most {MAX_NODES} and {MAX_EDGES}",
                self.nodes.len(),
                self.edges.len()
            )));
        }
        let mut ids = std::collections::HashSet::new();
        for n in &self.nodes {
            if !ids.insert(n.id.as_str()) {
                return Err(invalid(format!("node id {:?} appears twice", n.id)));
            }
            let values = [n.degree, n.strength, n.betweenness, n.closeness, n.pagerank];
            if values.iter().any(|v| !v.is_finite()) {
                return Err(invalid(format!("node {:?} has a non-finite metric", n.id)));
            }
        }
        if let Some(e) = self
            .edges
            .iter()
            .find(|e| !ids.contains(e.from.as_str()) || !ids.contains(e.to.as_str()))
        {
            return Err(invalid(format!(
                "edge {} → {} joins an unknown node",
                e.from, e.to
            )));
        }
        self.metrics
            .validate()
            .map_err(|e| invalid(format!("metrics envelope: {e}")))
    }

    /// The title or the file name or `Network`.
    #[must_use]
    pub fn display_title(&self) -> String {
        if let Some(t) = self.title.as_ref().filter(|t| !t.trim().is_empty()) {
            return t.clone();
        }
        self.path
            .as_deref()
            .and_then(|p| p.rsplit('/').next())
            .map_or_else(|| "Network".to_owned(), str::to_owned)
    }

    /// Edge density: edges over possible edges.
    #[must_use]
    pub fn density(&self) -> f64 {
        #[allow(clippy::cast_precision_loss)]
        let (n, m) = (self.nodes.len() as f64, self.edges.len() as f64);
        if n < 2.0 {
            return 0.0;
        }
        let possible = n * (n - 1.0) / if self.directed { 1.0 } else { 2.0 };
        m / possible
    }

    /// Node indices by `metric`, highest first (ties by input order).
    #[must_use]
    pub fn ranked(&self, metric: NetworkMetric) -> Vec<usize> {
        let mut order: Vec<usize> = (0..self.nodes.len()).collect();
        order.sort_by(|a, b| {
            self.nodes[*b]
                .metric(metric)
                .total_cmp(&self.nodes[*a].metric(metric))
                .then(a.cmp(b))
        });
        order
    }

    /// The network on the graph widget: a seeded force layout, nodes
    /// sized by `metric` (relative to its largest value) and coloured by
    /// community, their metrics in the focus details.
    #[must_use]
    pub fn graph(&self, metric: NetworkMetric) -> GraphSpec {
        let max = self
            .nodes
            .iter()
            .map(|n| n.metric(metric))
            .fold(0.0, f64::max);
        let nodes = self
            .nodes
            .iter()
            .map(|n| {
                let mut attrs = Map::new();
                attrs.insert("degree".into(), json!(n.degree));
                if let (Some(i), Some(o)) = (n.in_degree, n.out_degree) {
                    attrs.insert("in / out".into(), json!(format!("{i} / {o}")));
                }
                if self.weighted {
                    attrs.insert("strength".into(), json!(n.strength));
                }
                attrs.insert("betweenness".into(), json!(metric_text(n.betweenness)));
                attrs.insert("closeness".into(), json!(metric_text(n.closeness)));
                attrs.insert("PageRank".into(), json!(metric_text(n.pagerank)));
                attrs.insert("component".into(), json!(n.component + 1));
                attrs.insert("community".into(), json!(n.community + 1));
                GraphNode {
                    id: n.id.clone(),
                    label: n.label.clone(),
                    group: None,
                    kind: Some(format!("community {}", n.community + 1)),
                    attrs,
                    size: (max > 0.0).then(|| (n.metric(metric) / max).clamp(0.0, 1.0)),
                }
            })
            .collect();
        let edges = self
            .edges
            .iter()
            .map(|e| GraphEdge {
                from: e.from.clone(),
                to: e.to.clone(),
                label: None,
                weight: e.weight,
            })
            .collect();
        GraphSpec {
            title: Some(self.display_title()),
            directed: self.directed,
            layout: LayoutKind::Force,
            direction: crate::widget::graph::FlowDirection::Down,
            seed: Some(self.seed),
            nodes,
            edges,
            groups: Vec::new(),
        }
    }

    /// The metrics table, sorted by `metric` (at most [`TABLE_ROWS`]).
    #[must_use]
    pub fn table(&self, metric: NetworkMetric) -> TreeSpec {
        let mut names: Vec<&str> = vec!["degree"];
        if self.weighted {
            names.push("strength");
        }
        names.extend([
            "betweenness",
            "closeness",
            "PageRank",
            "component",
            "community",
        ]);
        let columns = names
            .iter()
            .map(|name| TreeColumn {
                name: (*name).to_owned(),
                kind: ColumnKind::Number,
                unit: None,
            })
            .collect();
        let nodes = self
            .ranked(metric)
            .into_iter()
            .take(TABLE_ROWS)
            .map(|ix| {
                let n = &self.nodes[ix];
                let mut values = Map::new();
                values.insert("degree".into(), json!(n.degree));
                if self.weighted {
                    values.insert("strength".into(), json!(n.strength));
                }
                values.insert("betweenness".into(), json!(n.betweenness));
                values.insert("closeness".into(), json!(n.closeness));
                values.insert("PageRank".into(), json!(n.pagerank));
                values.insert("component".into(), json!(n.component + 1));
                values.insert("community".into(), json!(n.community + 1));
                TreeNode {
                    label: n.text().to_owned(),
                    badge: None,
                    tone: None,
                    values,
                    children: Vec::new(),
                }
            })
            .collect();
        TreeSpec {
            title: Some(format!("Metrics by {}", metric.label())),
            columns,
            nodes,
            expand_depth: 1,
        }
    }

    /// The node metrics as CSV.
    #[must_use]
    pub fn to_csv(&self) -> String {
        let mut out = String::from(
            "id,label,degree,in_degree,out_degree,strength,betweenness,closeness,pagerank,\
             component,community\n",
        );
        let opt = |v: Option<f64>| v.map(|v| v.to_string()).unwrap_or_default();
        for n in &self.nodes {
            let _ = writeln!(
                out,
                "{},{},{},{},{},{},{},{},{},{},{}",
                csv_field(&n.id),
                csv_field(n.label.as_deref().unwrap_or("")),
                n.degree,
                opt(n.in_degree),
                opt(n.out_degree),
                n.strength,
                n.betweenness,
                n.closeness,
                n.pagerank,
                n.component + 1,
                n.community + 1
            );
        }
        out
    }

    /// The edges as CSV (`source,target,weight`).
    #[must_use]
    pub fn edges_csv(&self) -> String {
        let mut out = String::from("source,target,weight\n");
        for e in &self.edges {
            let _ = writeln!(
                out,
                "{},{},{}",
                csv_field(&e.from),
                csv_field(&e.to),
                e.weight.map(|w| w.to_string()).unwrap_or_default()
            );
        }
        out
    }
}

fn csv_field(text: &str) -> String {
    if text.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::computed::Engine;

    fn node(id: &str, degree: f64, pr: f64, community: usize) -> NetworkNode {
        NetworkNode {
            id: id.into(),
            label: None,
            degree,
            in_degree: None,
            out_degree: None,
            strength: degree,
            betweenness: degree / 10.0,
            closeness: 0.5,
            pagerank: pr,
            component: 0,
            community,
        }
    }

    fn spec() -> NetworkSpec {
        NetworkSpec {
            title: None,
            path: Some("net/edges.csv".into()),
            directed: false,
            weighted: false,
            nodes: vec![
                node("a", 2.0, 0.4, 0),
                node("b", 1.0, 0.3, 0),
                node("c", 1.0, 0.3, 1),
            ],
            edges: vec![
                NetworkEdge {
                    from: "a".into(),
                    to: "b".into(),
                    weight: None,
                },
                NetworkEdge {
                    from: "a".into(),
                    to: "c".into(),
                    weight: None,
                },
            ],
            components: 1,
            strong_components: None,
            communities: 2,
            modularity: 0.0,
            betweenness_sources: None,
            seed: 7,
            size_by: NetworkMetric::Degree,
            metrics: Computed::new(
                "nw_1",
                "network.metrics",
                "m",
                Engine {
                    name: "crest-graph".into(),
                    version: "0".into(),
                    backend: None,
                    platform: None,
                },
            ),
            warnings: Vec::new(),
        }
    }

    #[test]
    fn graph_sizes_by_metric_and_colours_by_community() {
        let s = spec();
        s.validate().unwrap();
        assert!((s.density() - 2.0 / 3.0).abs() < 1e-12);
        let g = s.graph(NetworkMetric::Degree);
        g.validate().unwrap();
        assert_eq!(g.layout, LayoutKind::Force);
        assert_eq!(g.seed, Some(7));
        assert_eq!(g.nodes[0].size, Some(1.0));
        assert_eq!(g.nodes[1].size, Some(0.5));
        assert_eq!(g.nodes[2].kind.as_deref(), Some("community 2"));
        let t = s.table(NetworkMetric::Pagerank);
        assert_eq!(t.nodes[0].label, "a");
        assert_eq!(s.ranked(NetworkMetric::Degree), vec![0, 1, 2]);
        assert!(
            s.to_csv().contains("\nc,,1,,,1,0.1,0.5,0.3,1,2\n"),
            "{}",
            s.to_csv()
        );
        assert_eq!(s.edges_csv(), "source,target,weight\na,b,\na,c,\n");
        assert_eq!(metric_text(0.436_363_6), "0.4364");
        assert_eq!(metric_text(3.0), "3");
        let mut bad = spec();
        bad.edges[0].to = "zz".into();
        assert!(bad.validate().is_err());
    }
}
