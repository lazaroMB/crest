//! `show_network`: a network from an edge list or node-link JSON with
//! the metrics the engine computed, on the graph widget.
//!
//! The model gets the size, density, components, communities and
//! modularity, and the top nodes by each metric with the envelope id,
//! never the whole metric table.

use std::fmt::Write as _;
use std::sync::Arc;

use async_trait::async_trait;
use domain::block::BlockKind;
use domain::permission::ToolEffect;
use domain::science::network::{MAX_EDGES, NetworkMetric, NetworkSpec, metric_text};
use futures::FutureExt;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{FormatToolPorts, invalid, one_line, read};
use crate::ports::formats::NetworkRequest;
use crate::tools::registry::{PreparedCall, ToolContext, ToolError, ToolOutput, TypedTool};
use crate::usecases::compute::offloaded;

/// Top nodes listed per metric.
const TOP: usize = 5;

/// One edge given inline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EdgeInput {
    /// Source node id.
    pub source: String,
    /// Target node id.
    pub target: String,
    /// Weight (a count, a strength).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight: Option<f64>,
}

/// Input of `show_network`.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShowNetwork {
    /// An edge list (CSV/TSV with a header: source, target and optional
    /// weight columns) or node-link JSON (`nodes`, `links`/`edges`) of the
    /// workspace.
    #[serde(default)]
    pub path: Option<String>,
    /// The edges inline instead of `path`.
    #[serde(default)]
    pub edges: Option<Vec<EdgeInput>>,
    /// Whether edges have a direction (default: the JSON's `directed`, else
    /// false).
    #[serde(default)]
    pub directed: Option<bool>,
    /// Source column of a CSV (default `source`/`from`/... or the first).
    #[serde(default)]
    pub source: Option<String>,
    /// Target column of a CSV (default `target`/`to`/... or the second).
    #[serde(default)]
    pub target: Option<String>,
    /// Weight column of a CSV (default `weight` when present).
    #[serde(default)]
    pub weight: Option<String>,
    /// Metric the nodes are sized by at first: `degree` (default),
    /// `betweenness`, `closeness` or `pagerank`.
    #[serde(default)]
    pub size_by: Option<NetworkMetric>,
    /// Seed of the community search, sampling and layout.
    #[serde(default)]
    pub seed: Option<u64>,
    /// Title shown above the graph.
    #[serde(default)]
    pub title: Option<String>,
}

/// `show_network`: a network with its metrics.
#[derive(Clone)]
pub struct ShowNetworkTool {
    ports: FormatToolPorts,
}

impl ShowNetworkTool {
    /// The tool over `ports`.
    #[must_use]
    pub fn new(ports: FormatToolPorts) -> Self {
        Self { ports }
    }
}

/// Name of `show_network`.
pub const SHOW_NETWORK: &str = "show_network";

/// Inline edges as node-link JSON bytes.
#[must_use]
pub fn edges_json(edges: &[EdgeInput]) -> Vec<u8> {
    json!({ "links": edges }).to_string().into_bytes()
}

#[async_trait]
impl TypedTool for ShowNetworkTool {
    type Input = ShowNetwork;

    fn name(&self) -> &str {
        SHOW_NETWORK
    }

    fn description(&self) -> &str {
        "Shows a network (an edge-list CSV or node-link JSON `path`, or \
         inline `edges`) in the user's timeline on a force layout, nodes \
         sized by a metric and coloured by community, with a metrics table. \
         The engine computes per node the degree, betweenness (Brandes), \
         closeness and PageRank, the connected components, and communities \
         by seeded label propagation with their modularity. Returns the \
         whole-graph numbers and the top nodes by each metric with the \
         envelope id; cite these instead of judging centrality yourself."
    }

    fn effect(&self, _input: &ShowNetwork) -> ToolEffect {
        ToolEffect::Read
    }

    async fn prepare(
        &self,
        input: ShowNetwork,
        _ctx: &ToolContext,
    ) -> Result<PreparedCall, ToolError> {
        let path = input.path.filter(|p| !p.trim().is_empty());
        let edges = input.edges.filter(|e| !e.is_empty());
        match (&path, &edges) {
            (Some(_), Some(_)) => {
                return Err(invalid(SHOW_NETWORK, "give `path` or `edges`, not both"));
            }
            (None, None) => return Err(invalid(SHOW_NETWORK, "give `path` or `edges`")),
            (None, Some(e)) if e.len() > MAX_EDGES => {
                return Err(invalid(SHOW_NETWORK, format!("at most {MAX_EDGES} edges")));
            }
            _ => {}
        }
        let ports = self.ports.clone();
        let subject = path.clone().unwrap_or_else(|| "inline edges".to_owned());
        let title = input.title.filter(|t| !t.trim().is_empty());
        let mut request = NetworkRequest {
            path: path.clone(),
            bytes: Vec::new(),
            directed: input.directed,
            source: input.source.filter(|s| !s.trim().is_empty()),
            target: input.target.filter(|s| !s.trim().is_empty()),
            weight: input.weight.filter(|s| !s.trim().is_empty()),
            size_by: input.size_by.unwrap_or_default(),
            seed: input.seed,
        };
        let run = async move {
            request.bytes = match (&path, edges) {
                (Some(path), _) => match read(ports.fs.as_ref(), path).await {
                    Ok(bytes) => bytes,
                    Err(message) => return Ok(ToolOutput::error(message)),
                },
                (None, edges) => edges_json(&edges.unwrap_or_default()),
            };
            let formats = Arc::clone(&ports.formats);
            let analysed =
                offloaded(&ports.offload, move || formats.analyse_network(&request)).await;
            let name = path.as_deref().unwrap_or("the edges");
            let mut spec = match analysed {
                Some(Ok(spec)) => spec,
                Some(Err(err)) => return Ok(ToolOutput::error(format!("{name}: {err}"))),
                None => return Ok(ToolOutput::error("the network engine did not run")),
            };
            spec.title = title;
            if let Err(err) = spec.validate() {
                return Ok(ToolOutput::error(format!("{name}: {err}")));
            }
            let summary = network_summary(&spec);
            Ok(ToolOutput::text(summary).with_block_update(BlockKind::Network(Box::new(spec))))
        };
        Ok(PreparedCall::new(subject, run.boxed()))
    }
}

/// What the model gets back from `show_network`.
#[must_use]
pub fn network_summary(spec: &NetworkSpec) -> String {
    let mut out = format!(
        "Network `{}` shown to the user: {} nodes, {} edges ({}{}), density {}, {} \
         components",
        one_line(&spec.display_title(), 80),
        spec.nodes.len(),
        spec.edges.len(),
        if spec.directed {
            "directed"
        } else {
            "undirected"
        },
        if spec.weighted { ", weighted" } else { "" },
        metric_text(spec.density()),
        spec.components
    );
    if let Some(s) = spec.strong_components {
        let _ = write!(out, " ({s} strongly connected)");
    }
    let _ = write!(
        out,
        ", {} communities (label propagation, seed {}, modularity {}).",
        spec.communities,
        spec.seed,
        metric_text(spec.modularity)
    );
    let _ = write!(
        out,
        "\nMetrics envelope `{}` (keys `nodes`, `edges`, `density`, `components`, \
         `communities`, `modularity`, `<node id>.degree|betweenness|closeness|pagerank`).",
        spec.metrics.id
    );
    for metric in NetworkMetric::ALL {
        let top: Vec<String> = spec
            .ranked(metric)
            .into_iter()
            .take(TOP)
            .map(|ix| {
                let n = &spec.nodes[ix];
                format!(
                    "{} {}",
                    one_line(n.text(), 40),
                    metric_text(n.metric(metric))
                )
            })
            .collect();
        let _ = write!(out, "\nTop by {}: {}.", metric.label(), top.join(", "));
    }
    for w in spec.warnings.iter().take(5) {
        let _ = write!(out, "\nNote: {}", one_line(w, 200));
    }
    out
}

#[cfg(test)]
mod tests {
    use domain::conversation::{SessionId, ToolUseId, TurnId};
    use serde_json::Value;

    use super::*;
    use crate::testing::formats::FakeFormats;
    use crate::testing::fs::{InMemoryFs, test_root};
    use crate::tools::registry::{Tool, Typed};
    use crate::usecases::compute::inline_offload;

    fn ctx() -> ToolContext {
        ToolContext::new(SessionId::new(), TurnId::new(), ToolUseId::new("toolu_1"))
    }

    fn ports() -> FormatToolPorts {
        let fs = InMemoryFs::new(test_root()).with_file("net/edges.csv", "source,target\n");
        FormatToolPorts {
            formats: Arc::new(FakeFormats::new()),
            fs: Arc::new(fs),
            offload: inline_offload(),
        }
    }

    async fn call(tool: &dyn Tool, input: Value) -> Result<ToolOutput, ToolError> {
        let prepared = tool.prepare(input, &ctx()).await?;
        prepared.run.await
    }

    #[test]
    fn definition() {
        let tool = Typed(ShowNetworkTool::new(ports()));
        insta::assert_json_snapshot!(
            "show_network_definition",
            json!({
                "name": tool.name(),
                "description": tool.description(),
                "input_schema": tool.input_schema(),
            })
        );
        let bytes = edges_json(&[EdgeInput {
            source: "a".into(),
            target: "b".into(),
            weight: None,
        }]);
        assert_eq!(
            String::from_utf8(bytes).unwrap(),
            r#"{"links":[{"source":"a","target":"b"}]}"#
        );
    }

    #[tokio::test]
    async fn the_model_gets_the_top_nodes() {
        let tool = Typed(ShowNetworkTool::new(ports()));
        let out = call(&tool, json!({ "path": "net/edges.csv", "seed": 3 }))
            .await
            .unwrap();
        assert!(!out.is_error, "{}", out.text_content());
        assert_eq!(
            out.text_content(),
            "Network `edges.csv` shown to the user: 3 nodes, 2 edges (undirected), density \
             0.6667, 1 components, 1 communities (label propagation, seed 3, modularity 0).\n\
             Metrics envelope `nw_fake` (keys `nodes`, `edges`, `density`, `components`, \
             `communities`, `modularity`, `<node id>.degree|betweenness|closeness|pagerank`).\n\
             Top by degree: b 2, a 1, c 1.\n\
             Top by betweenness: b 1, a 0, c 0.\n\
             Top by closeness: b 1, a 0.6670, c 0.6670.\n\
             Top by PageRank: b 0.4860, a 0.2570, c 0.2570."
        );
        assert!(matches!(out.block_update, Some(BlockKind::Network(_))));
        let inline = call(
            &tool,
            json!({ "edges": [{ "source": "a", "target": "b" }], "size_by": "pagerank" }),
        )
        .await
        .unwrap();
        assert!(!inline.is_error);
        assert!(call(&tool, json!({})).await.is_err());
        assert!(
            call(&tool, json!({ "path": "x", "size_by": "nope" }))
                .await
                .is_err()
        );
    }
}
