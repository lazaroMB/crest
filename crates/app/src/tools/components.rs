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
use domain::block::{
    BlockKind, Cell, ChartKind, ChartSpec, Column, MermaidSpec, Series, TableSpec,
};
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

/// Input of `show_table`.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShowTable {
    /// Column headers, in order; use `right` alignment for numbers.
    pub columns: Vec<Column>,
    /// Rows, each with exactly one cell (string, number, boolean or null)
    /// per column.
    pub rows: Vec<Vec<Cell>>,
}

/// `show_table`: shows records as a sortable table.
#[derive(Debug, Clone, Copy, Default)]
pub struct ShowTableTool;

const SHOW_TABLE: &str = "show_table";

#[async_trait]
impl TypedTool for ShowTableTool {
    type Input = ShowTable;

    fn name(&self) -> &str {
        SHOW_TABLE
    }

    fn description(&self) -> &str {
        "Shows rows of records as a sortable table in the user's timeline. \
         Use it for items with several attributes or exact values the user \
         will scan or compare, e.g. the largest files with their size and \
         last change. Every row needs one cell per column. Returns only a \
         short confirmation: do not repeat the rows in your reply."
    }

    fn effect(&self, _input: &ShowTable) -> ToolEffect {
        ToolEffect::Read
    }

    async fn prepare(
        &self,
        input: ShowTable,
        _ctx: &ToolContext,
    ) -> Result<PreparedCall, ToolError> {
        let spec = TableSpec {
            columns: input.columns,
            rows: input.rows,
        };
        spec.validate()
            .map_err(|err| invalid(SHOW_TABLE, err.to_string()))?;
        if spec.rows.is_empty() {
            return Err(invalid(SHOW_TABLE, "table has no rows"));
        }
        let confirmation = format!(
            "Table shown to the user ({} × {}).",
            count(spec.columns.len(), "column", "columns"),
            count(spec.rows.len(), "row", "rows"),
        );
        let subject = spec
            .columns
            .iter()
            .map(|column| column.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        Ok(show(subject, BlockKind::Table(spec), confirmation))
    }
}

/// Confirmation for a titled text component: `"<what> 'title' shown to the
/// user (N lines)."`.
fn source_confirmation(what: &str, title: Option<&str>, source: &str) -> String {
    let title = title.map(|t| format!(" '{t}'")).unwrap_or_default();
    let lines = count(source.lines().count(), "line", "lines");
    format!("{what}{title} shown to the user ({lines}).")
}

/// Subject of a text component: its title, or else its first line.
fn source_subject(title: Option<&str>, source: &str) -> String {
    title
        .or_else(|| source.lines().map(str::trim).find(|line| !line.is_empty()))
        .unwrap_or_default()
        .to_owned()
}

/// Input of `show_mermaid`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShowMermaid {
    /// Optional caption shown above the diagram.
    pub title: Option<String>,
    /// Mermaid source, starting with the diagram type (`flowchart TD`,
    /// `sequenceDiagram`, `classDiagram`, `stateDiagram-v2`, `erDiagram`…).
    pub source: String,
}

/// `show_mermaid`: renders a Mermaid diagram.
#[derive(Debug, Clone, Copy, Default)]
pub struct ShowMermaidTool;

const SHOW_MERMAID: &str = "show_mermaid";

#[async_trait]
impl TypedTool for ShowMermaidTool {
    type Input = ShowMermaid;

    fn name(&self) -> &str {
        SHOW_MERMAID
    }

    fn description(&self) -> &str {
        "Renders a Mermaid diagram in the user's timeline, with pan and zoom. \
         Use it to explain structure or flow: module dependencies, call \
         sequences, state machines, data models, architecture. If the source \
         does not parse, the user sees it with the error. Returns only a short \
         confirmation: do not repeat the source in your reply."
    }

    fn effect(&self, _input: &ShowMermaid) -> ToolEffect {
        ToolEffect::Read
    }

    async fn prepare(
        &self,
        input: ShowMermaid,
        _ctx: &ToolContext,
    ) -> Result<PreparedCall, ToolError> {
        if input.source.trim().is_empty() {
            return Err(invalid(SHOW_MERMAID, "diagram source is empty"));
        }
        let title = input.title.as_deref();
        let confirmation = source_confirmation("Mermaid diagram", title, &input.source);
        let subject = source_subject(title, &input.source);
        let spec = MermaidSpec {
            source: input.source,
            title: input.title,
        };
        Ok(show(subject, BlockKind::Mermaid(spec), confirmation))
    }
}

/// Input of `show_markdown`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShowMarkdown {
    /// Optional document title, shown in its header.
    pub title: Option<String>,
    /// Markdown source (CommonMark with tables and fenced code).
    pub source: String,
}

/// `show_markdown`: shows a markdown document with document typography.
#[derive(Debug, Clone, Copy, Default)]
pub struct ShowMarkdownTool;

const SHOW_MARKDOWN: &str = "show_markdown";

#[async_trait]
impl TypedTool for ShowMarkdownTool {
    type Input = ShowMarkdown;

    fn name(&self) -> &str {
        SHOW_MARKDOWN
    }

    fn description(&self) -> &str {
        "Shows a markdown document in the user's timeline as a separate, \
         well typeset page the user can toggle to source. Use it for long, \
         self-contained documents worth reading on their own: a report, a \
         plan, a design or review write-up. Short answers belong in your \
         normal reply. Returns only a short confirmation: do not repeat the \
         document in your reply."
    }

    fn effect(&self, _input: &ShowMarkdown) -> ToolEffect {
        ToolEffect::Read
    }

    async fn prepare(
        &self,
        input: ShowMarkdown,
        _ctx: &ToolContext,
    ) -> Result<PreparedCall, ToolError> {
        if input.source.trim().is_empty() {
            return Err(invalid(SHOW_MARKDOWN, "document source is empty"));
        }
        let title = input.title.as_deref();
        let confirmation = source_confirmation("Markdown document", title, &input.source);
        let subject = source_subject(title, &input.source);
        let block = BlockKind::Markdown {
            title: input.title,
            source: input.source,
        };
        Ok(show(subject, block, confirmation))
    }
}

#[cfg(test)]
mod tests {
    use domain::conversation::{SessionId, ToolUseId, TurnId};
    use serde_json::{Value, json};

    use super::*;
    use crate::tools::registry::{Tool, ToolRegistry, Typed};
    use domain::block::Align;

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

    #[test]
    fn show_table_definition() {
        insta::assert_json_snapshot!("show_table_definition", definition(ShowTableTool));
    }

    #[tokio::test]
    async fn show_table_adds_a_table_and_confirms_briefly() {
        let out = call(
            ShowTableTool,
            json!({
                "columns": [{ "name": "file" }, { "name": "bytes", "align": "right" }],
                "rows": [["a.rs", 120], ["b.rs", null]]
            }),
        )
        .await
        .unwrap();
        assert!(!out.is_error);
        assert_eq!(
            out.text_content(),
            "Table shown to the user (2 columns × 2 rows)."
        );
        let Some(BlockKind::Table(spec)) = out.block_update else {
            panic!("expected a table, got {:?}", out.block_update);
        };
        assert_eq!(spec.columns[1].align, Some(Align::Right));
        assert_eq!(
            spec.rows[0],
            [Cell::Text("a.rs".into()), Cell::Number(120.0)]
        );
        assert_eq!(spec.rows[1][1], Cell::Null);
    }

    #[tokio::test]
    async fn show_table_rejects_empty_tables() {
        let no_columns = json!({ "columns": [], "rows": [] });
        assert_eq!(
            invalid_message(call(ShowTableTool, no_columns).await),
            "table has no columns"
        );
        let no_rows = json!({ "columns": [{ "name": "a" }], "rows": [] });
        assert_eq!(
            invalid_message(call(ShowTableTool, no_rows).await),
            "table has no rows"
        );
    }

    #[tokio::test]
    async fn show_table_rejects_ragged_rows() {
        let input = json!({ "columns": [{ "name": "a" }, { "name": "b" }], "rows": [[1, 2], [3]] });
        assert_eq!(
            invalid_message(call(ShowTableTool, input).await),
            "table row 1 has 1 cells, expected 2"
        );
    }

    #[test]
    fn show_mermaid_definition() {
        insta::assert_json_snapshot!("show_mermaid_definition", definition(ShowMermaidTool));
    }

    #[tokio::test]
    async fn show_mermaid_adds_a_diagram_and_confirms_briefly() {
        let source = "flowchart TD\n  ui --> app\n  app --> domain";
        let out = call(
            ShowMermaidTool,
            json!({ "title": "Crates", "source": source }),
        )
        .await
        .unwrap();
        assert_eq!(
            out.text_content(),
            "Mermaid diagram 'Crates' shown to the user (3 lines)."
        );
        assert_eq!(
            out.block_update,
            Some(BlockKind::Mermaid(MermaidSpec {
                source: source.into(),
                title: Some("Crates".into()),
            }))
        );

        let untitled = call(ShowMermaidTool, json!({ "source": "graph LR; a-->b" }))
            .await
            .unwrap();
        assert_eq!(
            untitled.text_content(),
            "Mermaid diagram shown to the user (1 line)."
        );
    }

    #[tokio::test]
    async fn show_mermaid_rejects_an_empty_source() {
        assert_eq!(
            invalid_message(call(ShowMermaidTool, json!({ "source": " \n" })).await),
            "diagram source is empty"
        );
    }

    #[tokio::test]
    async fn untitled_components_are_summarized_by_their_first_line() {
        let prepared = Typed(ShowMermaidTool)
            .prepare(
                json!({ "source": "\n  sequenceDiagram\n  a->>b: hi" }),
                &ctx(),
            )
            .await
            .unwrap();
        assert_eq!(prepared.subject, "sequenceDiagram");
    }

    #[test]
    fn show_markdown_definition() {
        insta::assert_json_snapshot!("show_markdown_definition", definition(ShowMarkdownTool));
    }

    #[tokio::test]
    async fn show_markdown_adds_a_document_and_confirms_briefly() {
        let source = "# Plan\n\n- one\n- two\n";
        let out = call(
            ShowMarkdownTool,
            json!({ "title": "plan.md", "source": source }),
        )
        .await
        .unwrap();
        assert_eq!(
            out.text_content(),
            "Markdown document 'plan.md' shown to the user (4 lines)."
        );
        assert_eq!(
            out.block_update,
            Some(BlockKind::Markdown {
                title: Some("plan.md".into()),
                source: source.into(),
            })
        );
    }

    #[tokio::test]
    async fn show_markdown_rejects_an_empty_source() {
        assert_eq!(
            invalid_message(call(ShowMarkdownTool, json!({ "source": "" })).await),
            "document source is empty"
        );
    }
}
