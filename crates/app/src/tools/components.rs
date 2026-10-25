//! Component tools: the model calls them to show rich blocks in the
//! timeline (`show_chart`, `show_plot`, `show_table`, `show_mermaid`,
//! `show_markdown`).
//! [`register_component_tools`] adds them all and [`COMPONENT_TOOLS_PROMPT`]
//! tells the model when to use each.
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
use domain::plot::{PlotFunction, PlotParam, PlotRange, PlotSpec, XySeries};
use futures::FutureExt;
use schemars::JsonSchema;
use serde::Deserialize;

use super::registry::{
    PreparedCall, RegisterError, ToolContext, ToolError, ToolOutput, ToolRegistry, TypedTool,
};

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

/// Input of `show_plot`.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShowPlot {
    /// Plot title, shown above it.
    #[serde(default)]
    pub title: Option<String>,
    /// Visible x range; give it for functions. Without one the plot fits
    /// the series, or shows `-10..10`.
    #[serde(default)]
    pub x_range: Option<PlotRange>,
    /// Visible y range; fitted to the curves (ignoring spikes near
    /// asymptotes) when absent.
    #[serde(default)]
    pub y_range: Option<PlotRange>,
    /// Label of the x axis.
    #[serde(default)]
    pub x_label: Option<String>,
    /// Label of the y axis.
    #[serde(default)]
    pub y_label: Option<String>,
    /// Functions in calculator notation: `explicit` is `f(x)` (variable
    /// `x`), `parametric` is `expr` = x(t) and `y_expr` = y(t) over
    /// `t_range`, `implicit` draws the curve where `f(x, y) = 0`. Operators
    /// `+ - * / ^`, functions sin cos tan asin acos atan atan2 sinh cosh tanh
    /// exp ln log (natural) log10 sqrt abs floor ceil min max pow, constants
    /// pi and e, and the parameter names.
    #[serde(default)]
    pub functions: Vec<PlotFunction>,
    /// Parameters the expressions use, each shown as a slider the user can
    /// drag to re-plot.
    #[serde(default)]
    pub params: Vec<PlotParam>,
    /// Numeric (x, y) points drawn as `scatter` or a `line`, e.g. measured
    /// data next to a model curve.
    #[serde(default)]
    pub series: Vec<XySeries>,
}

/// `show_plot`: draws functions (with parameter sliders) and xy data on
/// numeric axes.
#[derive(Debug, Clone, Copy, Default)]
pub struct ShowPlotTool;

const SHOW_PLOT: &str = "show_plot";

#[async_trait]
impl TypedTool for ShowPlotTool {
    type Input = ShowPlot;

    fn name(&self) -> &str {
        SHOW_PLOT
    }

    fn description(&self) -> &str {
        "Plots mathematical functions and numeric (x, y) data on numeric axes \
         in the user's timeline. Functions can be explicit y = f(x), \
         parametric (x(t), y(t)) or implicit f(x, y) = 0; parameters become \
         sliders the user drags to see how the curves change, without \
         another call. Use it to explain a formula, compare models or show a \
         fit next to its data. Returns only a short confirmation: do not \
         repeat the data in your reply."
    }

    fn effect(&self, _input: &ShowPlot) -> ToolEffect {
        ToolEffect::Read
    }

    async fn prepare(
        &self,
        input: ShowPlot,
        _ctx: &ToolContext,
    ) -> Result<PreparedCall, ToolError> {
        let spec = PlotSpec {
            title: input.title.filter(|t| !t.trim().is_empty()),
            x_range: input.x_range,
            y_range: input.y_range,
            x_label: input.x_label,
            y_label: input.y_label,
            functions: input.functions,
            params: input.params,
            series: input.series,
        };
        spec.validate()
            .map_err(|err| invalid(SHOW_PLOT, err.to_string()))?;
        let mut parts = Vec::new();
        if !spec.functions.is_empty() {
            parts.push(count(spec.functions.len(), "function", "functions"));
        }
        if !spec.params.is_empty() {
            parts.push(count(spec.params.len(), "slider", "sliders"));
        }
        if !spec.series.is_empty() {
            parts.push(count(spec.series.len(), "series", "series"));
        }
        let title = spec
            .title
            .as_deref()
            .map(|t| format!(" '{t}'"))
            .unwrap_or_default();
        let confirmation = format!("Plot{title} shown to the user ({}).", parts.join(", "));
        let subject = spec
            .title
            .clone()
            .or_else(|| spec.functions.first().map(|f| f.expr.clone()))
            .or_else(|| spec.series.first().map(|s| s.name.clone()))
            .unwrap_or_default();
        Ok(show(subject, BlockKind::Plot(spec), confirmation))
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

/// System prompt section explaining when to use each component tool.
pub const COMPONENT_TOOLS_PROMPT: &str = "\
# Showing results

The timeline can show rich components. Use them when they make a result \
easier to read than prose:

- `show_chart`: quantities across categories or over time (sizes by file \
type, durations per run, commits per week). `bar` compares categories, \
`line`/`area` show trends, `pie` shows parts of a whole with one series.
- `show_plot`: mathematical functions and curves on numeric axes \
(explicit `f(x)`, parametric, implicit), with a slider per parameter so the \
user can explore how a formula behaves; also (x, y) data next to a model \
curve. Prefer it over `show_chart` whenever x is a number rather than a \
category. Plotting a formula is not computing a result: numbers you report \
from data (a fit, a mean) still come from the statistics tools.
- `show_table`: records with several attributes or exact values the user \
will scan, sort or compare (largest files, failing tests with their errors).
- `show_mermaid`: structure and flow (module dependencies, call sequences, \
state machines, data models).
- `show_markdown`: a long, self-contained document worth reading on its own \
(a report, a plan, a review). Short answers go in your normal reply.

Gather the data first with the other tools; never invent numbers. Each \
component tool returns only a short confirmation because the user already \
sees the component: do not repeat its data, add a sentence or two with the \
insight instead. If a call fails with an input error, fix the input and \
call it again.";

/// Registers `show_chart`, `show_plot`, `show_table`, `show_mermaid` and
/// `show_markdown`.
///
/// # Errors
///
/// [`RegisterError::Duplicate`] when one of them is already registered.
pub fn register_component_tools(registry: &mut ToolRegistry) -> Result<(), RegisterError> {
    registry.register_typed(ShowChartTool)?;
    registry.register_typed(ShowPlotTool)?;
    registry.register_typed(ShowTableTool)?;
    registry.register_typed(ShowMermaidTool)?;
    registry.register_typed(ShowMarkdownTool)
}

#[cfg(test)]
mod tests {
    use domain::conversation::{SessionId, ToolUseId, TurnId};
    use serde_json::{Value, json};

    use super::*;
    use crate::tools::registry::{Tool, Typed};
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
    fn show_plot_definition() {
        insta::assert_json_snapshot!("show_plot_definition", definition(ShowPlotTool));
    }

    #[tokio::test]
    async fn show_plot_adds_a_plot_and_confirms_briefly() {
        let out = call(
            ShowPlotTool,
            json!({
                "title": "Damped wave",
                "x_range": { "min": 0, "max": 10 },
                "functions": [
                    { "kind": "explicit", "expr": "a * exp(-k x) sin(x)" },
                    { "kind": "implicit", "expr": "x^2 + y^2 - 1" }
                ],
                "params": [
                    { "name": "a", "min": 0, "max": 2, "value": 1 },
                    { "name": "k", "min": 0, "max": 1, "step": 0.01, "value": 0.2 }
                ],
                "series": [{ "name": "measured", "x": [1, 2], "y": [0.7, 0.6] }]
            }),
        )
        .await
        .unwrap();
        assert!(!out.is_error);
        assert_eq!(
            out.text_content(),
            "Plot 'Damped wave' shown to the user (2 functions, 2 sliders, 1 series)."
        );
        let Some(BlockKind::Plot(spec)) = out.block_update else {
            panic!("expected a plot, got {:?}", out.block_update);
        };
        assert_eq!(spec.params[1].name, "k");
        assert_eq!(spec.series[0].x, [1.0, 2.0]);
    }

    #[tokio::test]
    async fn show_plot_reports_a_bad_expression_to_the_model() {
        let result = call(
            ShowPlotTool,
            json!({
                "functions": [{ "kind": "explicit", "expr": "a * sinx" }],
                "params": [{ "name": "a", "min": 0, "max": 2, "value": 1 }]
            }),
        )
        .await;
        let out = ToolOutput::from(result.unwrap_err());
        assert!(out.is_error);
        assert!(
            out.text_content()
                .contains(r#"unknown name "sinx" (variables here: x, a) (column 5)"#),
            "{}",
            out.text_content()
        );
        assert_eq!(out.block_update, None);

        let unclosed = json!({ "functions": [{ "kind": "explicit", "expr": "sin(x" }] });
        assert!(invalid_message(call(ShowPlotTool, unclosed).await).contains("expected ')'"));
        let nothing = json!({ "title": "empty" });
        assert_eq!(
            invalid_message(call(ShowPlotTool, nothing).await),
            "plot: a plot needs a function or a series"
        );
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

    #[test]
    fn registers_the_component_tools() {
        let mut registry = ToolRegistry::new();
        register_component_tools(&mut registry).unwrap();
        assert_eq!(
            registry.names(),
            [
                "show_chart",
                "show_markdown",
                "show_mermaid",
                "show_plot",
                "show_table"
            ]
        );
        for name in registry.names() {
            assert!(
                COMPONENT_TOOLS_PROMPT.contains(&format!("`{name}`")),
                "{name}"
            );
        }
        assert_eq!(
            register_component_tools(&mut registry),
            Err(RegisterError::Duplicate("show_chart".into()))
        );
    }

    #[tokio::test]
    async fn run_turn_shows_the_chart_the_model_asks_for() {
        use std::sync::{Arc, Mutex};

        use domain::block::ToolStatus;
        use domain::conversation::{ContentPart, Session};
        use domain::model::ModelPolicy;
        use domain::permission::{PermissionMode, PermissionPolicy};

        use crate::ports::events::DomainEvent;
        use crate::testing::llm::{ScriptedLlm, ScriptedResponse};
        use crate::testing::system::{RecordingSink, RecordingTelemetry, ScriptedApproval};
        use crate::usecases::run_turn::{AgentDeps, AgentService, TurnOptions};

        let chart = json!({
            "title": "Bytes by extension",
            "kind": "pie",
            "labels": ["rs", "md"],
            "series": [{ "name": "bytes", "values": [900, 100] }]
        });
        let llm = Arc::new(ScriptedLlm::new([
            ScriptedResponse::tool_use("toolu_1", "show_chart", chart),
            ScriptedResponse::text("Mostly Rust."),
        ]));
        let sink = Arc::new(RecordingSink::new());
        let deps = AgentDeps {
            llm: llm.clone(),
            events: sink.clone(),
            approval: Arc::new(ScriptedApproval::new()),
            telemetry: Arc::new(RecordingTelemetry::new()),
        };
        let mut tools = ToolRegistry::new();
        register_component_tools(&mut tools).unwrap();
        let permissions = Arc::new(Mutex::new(PermissionPolicy::new(PermissionMode::Ask)));
        let service = AgentService::new(deps, tools, ModelPolicy::default(), permissions)
            .with_system_prompt(COMPONENT_TOOLS_PROMPT);
        let mut session = Session::new("/ws", 0);
        service
            .run_turn(&mut session, "chart it", TurnOptions::default())
            .await
            .unwrap();

        let events = sink.events();
        let charts: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                DomainEvent::BlockAdded { block } => match &block.kind {
                    BlockKind::Chart(spec) => Some(spec),
                    _ => None,
                },
                _ => None,
            })
            .collect();
        assert_eq!(charts.len(), 1, "{events:#?}");
        assert_eq!(charts[0].title, "Bytes by extension");
        assert_eq!(charts[0].kind, ChartKind::Pie);
        assert!(events.iter().any(|event| matches!(
            event,
            DomainEvent::ToolStatusChanged {
                status: ToolStatus::Succeeded,
                ..
            }
        )));
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, DomainEvent::ApprovalRequested { .. })),
            "component tools must not ask for approval"
        );

        let results: Vec<_> = session
            .turns
            .iter()
            .flat_map(|turn| &turn.messages)
            .flat_map(|message| &message.content)
            .filter_map(|part| match part {
                ContentPart::ToolResult {
                    content, is_error, ..
                } => Some((content.clone(), *is_error)),
                _ => None,
            })
            .collect();
        let expected = ToolOutput::text(
            "Chart 'Bytes by extension' shown to the user (pie, 1 series × 2 points).",
        );
        assert_eq!(results, [(expected.content, false)]);
    }
}
