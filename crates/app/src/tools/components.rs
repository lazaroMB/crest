//! Component tools: the model calls them to show rich blocks in the
//! timeline (`show_chart`, …).
//!
//! Each input is plain JSON validated against a schema derived with
//! `schemars` and then by the block's own `validate()`; a bad input comes
//! back to the model as an `is_error` result with the reason, so it can fix
//! the call. A good one is shown as a new block (through
//! [`ToolOutput::block_update`]) and the model only gets a one-line
//! confirmation, never the data, to save context. They only read: no call
//! needs approval.

use async_trait::async_trait;
use domain::block::{BlockKind, ChartKind, ChartSpec, Series};
use domain::permission::ToolEffect;
use futures::FutureExt;
use schemars::JsonSchema;
use serde::Deserialize;

use super::registry::{PreparedCall, ToolContext, ToolError, ToolOutput, TypedTool};

/// An input error for the model to fix.
fn invalid(tool: &str, message: impl Into<String>) -> ToolError {
    ToolError::InvalidInput {
        tool: tool.to_owned(),
        message: message.into(),
    }
}

/// A call on `subject` that shows `block` and answers `confirmation`.
fn show(subject: String, block: BlockKind, confirmation: String) -> PreparedCall {
    let output = ToolOutput::text(confirmation).with_block_update(block);
    PreparedCall::new(subject, futures::future::ready(Ok(output)).boxed())
}

/// `n` followed by the singular or plural noun.
fn count(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Input of `show_chart`.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShowChart {
    /// Chart title, shown above it.
    pub title: String,
    /// `bar` compares categories, `line` and `area` show a trend along an
    /// ordered axis (time, versions), `pie` shows parts of a whole and takes
    /// exactly one series.
    pub kind: ChartKind,
    /// Category labels (x axis or pie slices), in display order.
    pub labels: Vec<String>,
    /// One or more named series, each with exactly one value per label.
    pub series: Vec<Series>,
}

/// `show_chart`: draws numeric series as a chart the user can toggle to a
/// table.
#[derive(Debug, Clone, Copy, Default)]
pub struct ShowChartTool;

const SHOW_CHART: &str = "show_chart";

#[async_trait]
impl TypedTool for ShowChartTool {
    type Input = ShowChart;

    fn name(&self) -> &str {
        SHOW_CHART
    }

    fn description(&self) -> &str {
        "Shows numeric data as a chart in the user's timeline (the user can \
         toggle it to a table). Use it for quantities across categories or \
         over time, e.g. file sizes by extension or test durations per run. \
         Every series needs one finite value per label; a pie chart takes \
         exactly one series. Returns only a short confirmation: do not repeat \
         the data in your reply."
    }

    fn effect(&self, _input: &ShowChart) -> ToolEffect {
        ToolEffect::Read
    }

    async fn prepare(
        &self,
        input: ShowChart,
        _ctx: &ToolContext,
    ) -> Result<PreparedCall, ToolError> {
        if input.labels.is_empty() {
            return Err(invalid(SHOW_CHART, "chart needs at least one label"));
        }
        let spec = ChartSpec {
            title: input.title,
            kind: input.kind,
            labels: input.labels,
            series: input.series,
        };
        spec.validate()
            .map_err(|err| invalid(SHOW_CHART, err.to_string()))?;
        let confirmation = format!(
            "Chart '{}' shown to the user ({}, {} × {}).",
            spec.title,
            chart_kind_name(spec.kind),
            count(spec.series.len(), "series", "series"),
            count(spec.labels.len(), "point", "points"),
        );
        Ok(show(
            spec.title.clone(),
            BlockKind::Chart(spec),
            confirmation,
        ))
    }
}

/// Name of `kind` as the model writes it.
fn chart_kind_name(kind: ChartKind) -> &'static str {
    match kind {
        ChartKind::Bar => "bar",
        ChartKind::Line => "line",
        ChartKind::Area => "area",
        ChartKind::Pie => "pie",
    }
}

#[cfg(test)]
mod tests {
    use domain::conversation::{SessionId, ToolUseId, TurnId};
    use serde_json::{Value, json};

    use super::*;
    use crate::tools::registry::{Tool, ToolRegistry, Typed};

    fn ctx() -> ToolContext {
        ToolContext::new(SessionId::new(), TurnId::new(), ToolUseId::new("toolu_1"))
    }

    /// Prepares and runs `tool` on `input` as the agent loop would.
    async fn call<T: TypedTool + 'static>(tool: T, input: Value) -> Result<ToolOutput, ToolError> {
        let mut registry = ToolRegistry::new();
        let name = tool.name().to_owned();
        registry.register_typed(tool).unwrap();
        registry.dispatch(&name, input, &ctx()).await
    }

    fn definition<T: TypedTool + 'static>(tool: T) -> Value {
        let tool = Typed(tool);
        json!({
            "name": tool.name(),
            "description": tool.description(),
            "input_schema": tool.input_schema(),
        })
    }

    fn invalid_message(result: Result<ToolOutput, ToolError>) -> String {
        match result {
            Err(ToolError::InvalidInput { message, .. }) => message,
            other => panic!("expected an input error, got {other:?}"),
        }
    }

    #[test]
    fn show_chart_definition() {
        insta::assert_json_snapshot!("show_chart_definition", definition(ShowChartTool));
    }

    #[test]
    fn show_chart_only_reads() {
        let input = json!({ "title": "t", "kind": "bar", "labels": [], "series": [] });
        assert_eq!(Typed(ShowChartTool).effect(&input), ToolEffect::Read);
    }

    #[tokio::test]
    async fn show_chart_adds_a_chart_and_confirms_briefly() {
        let out = call(
            ShowChartTool,
            json!({
                "title": "Size by extension",
                "kind": "bar",
                "labels": ["rs", "md", "toml"],
                "series": [
                    { "name": "bytes", "values": [1200, 300, 40] },
                    { "name": "files", "values": [12, 3, 2] }
                ]
            }),
        )
        .await
        .unwrap();
        assert!(!out.is_error);
        assert_eq!(
            out.text_content(),
            "Chart 'Size by extension' shown to the user (bar, 2 series × 3 points)."
        );
        let Some(BlockKind::Chart(spec)) = out.block_update else {
            panic!("expected a chart, got {:?}", out.block_update);
        };
        assert_eq!(spec.kind, ChartKind::Bar);
        assert_eq!(spec.labels, ["rs", "md", "toml"]);
        assert_eq!(spec.series[1].values, [12.0, 3.0, 2.0]);
    }

    #[tokio::test]
    async fn show_chart_rejects_misaligned_series() {
        let message = invalid_message(
            call(
                ShowChartTool,
                json!({
                    "title": "t", "kind": "line", "labels": ["a", "b"],
                    "series": [{ "name": "x", "values": [1, 2] }, { "name": "y", "values": [1] }]
                }),
            )
            .await,
        );
        assert_eq!(message, r#"series "y" has 1 values, expected 2"#);
    }

    #[tokio::test]
    async fn show_chart_rejects_a_pie_with_two_series() {
        let message = invalid_message(
            call(
                ShowChartTool,
                json!({
                    "title": "t", "kind": "pie", "labels": ["a"],
                    "series": [{ "name": "x", "values": [1] }, { "name": "y", "values": [2] }]
                }),
            )
            .await,
        );
        assert_eq!(message, "pie chart needs exactly one series, got 2");
    }

    #[tokio::test]
    async fn show_chart_rejects_empty_charts() {
        let no_labels = json!({ "title": "t", "kind": "bar", "labels": [], "series": [] });
        assert_eq!(
            invalid_message(call(ShowChartTool, no_labels).await),
            "chart needs at least one label"
        );
        let no_series = json!({ "title": "t", "kind": "bar", "labels": ["a"], "series": [] });
        assert_eq!(
            invalid_message(call(ShowChartTool, no_series).await),
            "chart has no series"
        );
    }

    #[tokio::test]
    async fn invalid_input_becomes_an_error_result_for_the_model() {
        let err = call(ShowChartTool, json!({ "title": "t", "kind": "radar" }))
            .await
            .unwrap_err();
        let out = ToolOutput::from(err);
        assert!(out.is_error);
        assert!(
            out.text_content().contains("show_chart"),
            "{}",
            out.text_content()
        );
        assert_eq!(out.block_update, None);
    }
}
