//! `show_query_plan`: a SQL query plan (PostgreSQL `EXPLAIN (FORMAT
//! JSON)`) on the tree table.
//!
//! The plan comes from a workspace file or inline (pasted from `psql`).
//! The model gets the timings, the slowest nodes by self time, the worst
//! row misestimates and the warnings (spills, filtering sequential
//! scans), never the plan itself.

use std::fmt::Write as _;
use std::sync::Arc;

use async_trait::async_trait;
use domain::block::BlockKind;
use domain::devops::query_plan::{QueryPlanSpec, round_factor};
use domain::permission::ToolEffect;
use domain::widget::timeline::{TimeUnit, format_duration};
use domain::widget::tree::format_number;
use futures::FutureExt;
use schemars::JsonSchema;
use serde::Deserialize;

use super::devops::MAX_TOP;
use super::{FormatToolPorts, MAX_LISTED, invalid, one_line, read};
use crate::tools::registry::{PreparedCall, ToolContext, ToolError, ToolOutput, TypedTool};
use crate::usecases::compute::offloaded;

/// Largest inline plan, in bytes.
pub const MAX_INLINE_PLAN: usize = 4 * 1024 * 1024;
/// Smallest misestimate factor listed to the model.
pub const MISESTIMATE_LISTED: f64 = 10.0;

/// Input of `show_query_plan`.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShowQueryPlan {
    /// A file of the workspace with the output of PostgreSQL's
    /// `EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) ...` (or without ANALYZE /
    /// BUFFERS). Give this or `plan`.
    #[serde(default)]
    pub path: Option<String>,
    /// The same JSON plan inline, e.g. pasted from psql (header and `+`
    /// marks are fine). Give this or `path`.
    #[serde(default)]
    pub plan: Option<String>,
    /// Title shown above the plan; defaults to the file name.
    #[serde(default)]
    pub title: Option<String>,
}

/// `show_query_plan`: a query plan on the tree table.
#[derive(Clone)]
pub struct ShowQueryPlanTool {
    ports: FormatToolPorts,
}

impl ShowQueryPlanTool {
    /// The tool over `ports`.
    #[must_use]
    pub fn new(ports: FormatToolPorts) -> Self {
        Self { ports }
    }
}

/// Name of `show_query_plan`.
pub const SHOW_QUERY_PLAN: &str = "show_query_plan";

#[async_trait]
impl TypedTool for ShowQueryPlanTool {
    type Input = ShowQueryPlan;

    fn name(&self) -> &str {
        SHOW_QUERY_PLAN
    }

    fn description(&self) -> &str {
        "Shows a PostgreSQL query plan (EXPLAIN (FORMAT JSON), best with \
         ANALYZE and BUFFERS) as a tree table in the user's timeline: per \
         node its time, self time and rows as bars, estimated vs actual \
         rows with the misestimate factor, loops, buffers and cost, hot \
         nodes badged, spills and filtering sequential scans noted. Give a \
         workspace file in `path` or the JSON in `plan`. Returns the \
         timings, the slowest nodes by self time, the worst row \
         misestimates and the warnings: reason from those, and do not \
         repeat the plan in your reply."
    }

    fn effect(&self, _input: &ShowQueryPlan) -> ToolEffect {
        ToolEffect::Read
    }

    async fn prepare(
        &self,
        input: ShowQueryPlan,
        _ctx: &ToolContext,
    ) -> Result<PreparedCall, ToolError> {
        let path = input.path.clone().filter(|p| !p.trim().is_empty());
        let plan = input.plan.clone().filter(|p| !p.trim().is_empty());
        let subject = match (&path, &plan) {
            (Some(_), Some(_)) => {
                return Err(invalid(SHOW_QUERY_PLAN, "give `path` or `plan`, not both"));
            }
            (None, None) => return Err(invalid(SHOW_QUERY_PLAN, "give `path` or `plan`")),
            (Some(path), None) => path.clone(),
            (None, Some(plan)) => {
                if plan.len() > MAX_INLINE_PLAN {
                    return Err(invalid(
                        SHOW_QUERY_PLAN,
                        "`plan` is too large: save it to a file and pass `path`",
                    ));
                }
                "inline plan".to_owned()
            }
        };
        let ports = self.ports.clone();
        let run = async move {
            let bytes = match (&path, plan) {
                (Some(path), _) => match read(ports.fs.as_ref(), path).await {
                    Ok(bytes) => bytes,
                    Err(message) => return Ok(ToolOutput::error(message)),
                },
                (None, plan) => plan.unwrap_or_default().into_bytes(),
            };
            let formats = Arc::clone(&ports.formats);
            let at = path.clone();
            let parsed = offloaded(&ports.offload, move || {
                formats.parse_query_plan(&bytes, at.as_deref())
            })
            .await;
            let mut spec = match parsed {
                Some(Ok(spec)) => spec,
                Some(Err(err)) => return Ok(ToolOutput::error(err.to_string())),
                None => return Ok(ToolOutput::error("the plan parser did not run")),
            };
            spec.title = input.title.filter(|t| !t.trim().is_empty());
            let name = path.as_deref().unwrap_or("inline plan");
            if let Err(err) = spec.validate() {
                return Ok(ToolOutput::error(format!("`{name}`: {err}")));
            }
            let summary = query_plan_summary(path.as_deref(), &spec);
            Ok(ToolOutput::text(summary).with_block_update(BlockKind::QueryPlan(Box::new(spec))))
        };
        Ok(PreparedCall::new(subject, run.boxed()))
    }
}

fn ms(value: f64) -> String {
    format_duration(value, TimeUnit::Ms)
}

/// What the model gets back from `show_query_plan`.
#[must_use]
pub fn query_plan_summary(path: Option<&str>, spec: &QueryPlanSpec) -> String {
    let what = path.map_or_else(|| "Query plan".to_owned(), |p| format!("Query plan `{p}`"));
    let mut options = vec!["EXPLAIN"];
    if spec.analyzed {
        options.push("ANALYZE");
    }
    if spec.buffers {
        options.push("BUFFERS");
    }
    let mut out = format!(
        "{what} ({}, {}) shown to the user: {} nodes",
        spec.engine.name(),
        options.join(" "),
        spec.nodes.len()
    );
    if let Some(t) = spec.planning_time {
        let _ = write!(out, ", planning {}", ms(t));
    }
    if let Some(t) = spec.execution_time {
        let _ = write!(out, ", execution {}", ms(t));
    }
    if let Some(t) = spec.trigger_time {
        let _ = write!(out, ", triggers {}", ms(t));
    }
    if let Some(t) = spec.jit_time {
        let _ = write!(out, ", JIT {}", ms(t));
    }
    if let Some(cost) = spec.nodes.first().and_then(|n| n.total_cost) {
        let _ = write!(out, ", total cost {}", format_number(cost));
    }
    out.push('.');
    if !spec.analyzed {
        out.push_str(
            " Estimates only: run EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) for actual times and \
             rows.",
        );
    }
    let slowest = spec.slowest(MAX_TOP);
    if !slowest.is_empty() {
        out.push_str("\nSlowest nodes by self time:");
        for ix in slowest {
            let node = &spec.nodes[ix];
            let _ = write!(out, "\n- {}", one_line(&node.label(), 120));
            if let Some(t) = node.self_time {
                let _ = write!(out, ": {} self", ms(t));
            }
            if let Some(share) = spec.self_share(ix) {
                let _ = write!(out, " ({:.1}%)", share * 100.0);
            }
            if let Some(t) = node.inclusive_time {
                let _ = write!(out, ", {} with children", ms(t));
            }
            if let Some(rows) = node.total_rows() {
                let _ = write!(out, ", {} rows", format_number(rows));
            }
            if let Some(loops) = node.loops.filter(|l| *l > 1.0) {
                let _ = write!(out, ", {} loops", format_number(loops));
            }
            if spec.is_hot(ix) {
                out.push_str(" [hot]");
            }
        }
    }
    let worst = spec.misestimates(MISESTIMATE_LISTED, MAX_TOP);
    if !worst.is_empty() {
        out.push_str("\nWorst row estimates:");
        for m in worst {
            let node = &spec.nodes[m.node];
            let _ = write!(
                out,
                "\n- {}: estimated {}, actual {} per loop (×{} {})",
                one_line(&node.label(), 120),
                format_number(node.plan_rows.unwrap_or(0.0)),
                format_number(node.actual_rows.unwrap_or(0.0)),
                format_number(round_factor(m.factor)),
                if m.under { "under" } else { "over" }
            );
        }
    }
    let warnings: Vec<(usize, &String)> = spec
        .nodes
        .iter()
        .enumerate()
        .flat_map(|(ix, n)| {
            n.warnings
                .iter()
                .filter(|w| !w.starts_with("rows misestimated"))
                .map(move |w| (ix, w))
        })
        .collect();
    if !warnings.is_empty() {
        out.push_str("\nWarnings:");
        for (ix, warning) in warnings.iter().take(MAX_LISTED) {
            let node = &spec.nodes[*ix];
            let _ = write!(out, "\n- {}: {warning}", one_line(&node.label(), 120));
            if let Some(condition) = &node.condition {
                let _ = write!(out, " ({})", one_line(condition, 120));
            }
        }
        if warnings.len() > MAX_LISTED {
            let _ = write!(out, "\n… and {} more", warnings.len() - MAX_LISTED);
        }
    }
    let never = spec.nodes.iter().filter(|n| n.never_executed()).count();
    if never > 0 {
        let _ = write!(out, "\n{never} nodes never executed.");
    }
    out
}

#[cfg(test)]
mod tests {
    use domain::conversation::{SessionId, ToolUseId, TurnId};
    use serde_json::{Value, json};

    use super::*;
    use crate::testing::formats::FakeFormats;
    use crate::testing::fs::{InMemoryFs, test_root};
    use crate::tools::registry::{Tool, Typed};
    use crate::usecases::compute::inline_offload;

    fn ctx() -> ToolContext {
        ToolContext::new(SessionId::new(), TurnId::new(), ToolUseId::new("toolu_1"))
    }

    fn ports(formats: Arc<FakeFormats>) -> FormatToolPorts {
        let fs = InMemoryFs::new(test_root()).with_file("plans/q.json", "[]");
        FormatToolPorts {
            formats,
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
        let tool = Typed(ShowQueryPlanTool::new(ports(Arc::new(FakeFormats::new()))));
        insta::assert_json_snapshot!(
            "show_query_plan_definition",
            json!({
                "name": tool.name(),
                "description": tool.description(),
                "input_schema": tool.input_schema(),
            })
        );
    }

    #[tokio::test]
    async fn a_plan_file_is_a_block_and_the_model_gets_the_hot_spots() {
        let formats = Arc::new(FakeFormats::new());
        let tool = Typed(ShowQueryPlanTool::new(ports(Arc::clone(&formats))));
        let out = call(&tool, json!({ "path": "plans/q.json" }))
            .await
            .unwrap();
        assert!(!out.is_error, "{}", out.text_content());
        assert_eq!(
            out.text_content(),
            "Query plan `plans/q.json` (PostgreSQL, EXPLAIN ANALYZE) shown to the user: 3 nodes, \
             planning 500 µs, execution 100 ms.\n\
             Slowest nodes by self time:\n\
             - Seq Scan on orders: 70 ms self (70.0%), 70 ms with children, 10000 rows [hot]\n\
             - Hash: 20 ms self (20.0%), 20 ms with children, 10 rows [hot]\n\
             - Hash Join: 10 ms self (10.0%), 100 ms with children, 5000 rows [hot]\n\
             Worst row estimates:\n\
             - Hash Join: estimated 50, actual 5000 per loop (×100 under)"
        );
        assert!(matches!(out.block_update, Some(BlockKind::QueryPlan(_))));
        assert_eq!(
            formats.calls(),
            [("parse_query_plan".to_owned(), "plans/q.json".to_owned())]
        );
    }

    #[tokio::test]
    async fn inline_plans_and_bad_inputs() {
        let formats = Arc::new(FakeFormats::new());
        let tool = Typed(ShowQueryPlanTool::new(ports(Arc::clone(&formats))));
        let out = call(&tool, json!({ "plan": "[{\"Plan\": {}}]", "title": "q1" }))
            .await
            .unwrap();
        assert!(
            out.text_content().starts_with("Query plan (PostgreSQL"),
            "{}",
            out.text_content()
        );
        let Some(BlockKind::QueryPlan(spec)) = out.block_update else {
            panic!("no plan block");
        };
        assert_eq!(spec.title.as_deref(), Some("q1"));
        let both = tool
            .prepare(json!({ "path": "a", "plan": "b" }), &ctx())
            .await;
        assert!(matches!(both, Err(ToolError::InvalidInput { .. })));
        let neither = tool.prepare(json!({ "title": "x" }), &ctx()).await;
        assert!(matches!(neither, Err(ToolError::InvalidInput { .. })));
        let missing = call(&tool, json!({ "path": "nope.json" })).await.unwrap();
        assert!(missing.is_error);
    }
}
