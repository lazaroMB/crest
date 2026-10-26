//! Timeline blocks and their payloads.
//!
//! A [`Block`] is a stable [`BlockId`] plus a [`BlockKind`]. The kind is
//! flattened into the block and tagged by `type`, so a block serializes as
//! `{"id": "...", "type": "text", "markdown": "..."}`. Payloads are plain
//! data: the UI decides how to draw each kind, and adapters (diff engines,
//! component tools, Lua plugins) only fill them in.

use std::borrow::Cow;
use std::fmt;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::computed::{Computed, ComputedBody, ComputedError};
use crate::dataset::DatasetSpec;
use crate::devops::bench::BenchSpec;
use crate::devops::diagram::DiagramSpec;
use crate::devops::flame::FlameSpec;
use crate::devops::har::HarSpec;
use crate::devops::logs::LogSpec;
use crate::devops::query_plan::QueryPlanSpec;
use crate::devops::terraform::TerraformPlanSpec;
use crate::devops::trace::TraceSpec;
use crate::engineering::cad::CadModelSpec;
use crate::engineering::drawing::DrawingSpec;
use crate::engineering::ifc::IfcScheduleSpec;
use crate::engineering::material::MaterialSpec;
use crate::engineering::netlist::NetlistSpec;
use crate::engineering::waveform::WaveformSpec;
use crate::form::FormSpec;
use crate::model3d::{MeshError, ModelSpec};
use crate::plot::PlotSpec;
use crate::report::{FindingsSpec, TestReportSpec};
use crate::science::{MoleculeSpec, PlateSpec};

/// Stable identifier of a block in the timeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BlockId(pub Uuid);

impl BlockId {
    /// Creates a fresh random id.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for BlockId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for BlockId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl JsonSchema for BlockId {
    fn schema_name() -> Cow<'static, str> {
        "BlockId".into()
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "description": "Stable identifier of a block in the timeline.",
            "type": "string",
            "format": "uuid"
        })
    }
}

/// A block of the timeline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Block {
    /// Stable id, used to update the block in place (streaming, status).
    pub id: BlockId,
    /// What the block shows.
    #[serde(flatten)]
    pub kind: BlockKind,
}

impl Block {
    /// Creates a block with a fresh id.
    #[must_use]
    pub fn new(kind: BlockKind) -> Self {
        Self {
            id: BlockId::new(),
            kind,
        }
    }
}

impl Block {
    /// Validates the payload (tables, charts, custom trees, sources, stage
    /// progress, computed envelopes and the spec of every other rich kind);
    /// text-like kinds are always valid.
    ///
    /// # Errors
    ///
    /// Returns the first [`BlockError`] found.
    pub fn validate(&self) -> Result<(), BlockError> {
        match &self.kind {
            BlockKind::Table(spec) => spec.validate(),
            BlockKind::Chart(spec) => spec.validate(),
            BlockKind::Custom { tree, .. } => tree.validate(),
            BlockKind::Sources { sources } => sources.iter().try_for_each(SourceRef::validate),
            BlockKind::Image { source, .. } => source.as_ref().map_or(Ok(()), SourceRef::validate),
            BlockKind::StageHeader { index, total, .. } => {
                if (1..=*total).contains(index) {
                    Ok(())
                } else {
                    Err(BlockError::StageOutOfRange {
                        index: *index,
                        total: *total,
                    })
                }
            }
            BlockKind::Computed { envelope, body } => {
                envelope.validate()?;
                Ok(body.validate()?)
            }
            BlockKind::Form(spec) => spec.validate(),
            BlockKind::Plot(spec) => spec.validate(),
            BlockKind::TestReport(spec) => spec.validate(),
            BlockKind::Findings(spec) => spec.validate(),
            BlockKind::Dataset(spec) => spec.validate(),
            BlockKind::Molecule(spec) => spec.validate(),
            BlockKind::Plate(spec) => spec.validate(),
            BlockKind::Trace(spec) => spec.validate(),
            BlockKind::FlameGraph(spec) => spec.validate(),
            BlockKind::Logs(spec) => spec.validate(),
            BlockKind::QueryPlan(spec) => spec.validate(),
            BlockKind::Har(spec) => spec.validate(),
            BlockKind::TerraformPlan(spec) => spec.validate(),
            BlockKind::Diagram(spec) => spec.validate(),
            BlockKind::Benchmarks(spec) => spec.validate(),
            BlockKind::Waveform(spec) => spec.validate(),
            BlockKind::Netlist(spec) => spec.validate(),
            BlockKind::Material(spec) => spec.validate(),
            BlockKind::CadModel(spec) => spec.validate(),
            BlockKind::Drawing(spec) => spec.validate(),
            BlockKind::IfcSchedule(spec) => spec.validate(),
            BlockKind::Text { .. }
            | BlockKind::Thinking { .. }
            | BlockKind::ToolCall { .. }
            | BlockKind::Diff(_)
            | BlockKind::Mermaid(_)
            | BlockKind::Markdown { .. }
            | BlockKind::Shell { .. }
            | BlockKind::Error { .. } => Ok(()),
        }
    }
}

/// The payload of a [`Block`], tagged by `type`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BlockKind {
    /// Model answer, markdown that may still be streaming.
    Text {
        /// Markdown source.
        markdown: String,
    },
    /// Extended thinking of the model.
    Thinking {
        /// Raw reasoning text.
        text: String,
        /// Whether the UI shows it collapsed (the default).
        collapsed: bool,
    },
    /// A tool invocation: one line with name, key arguments and status.
    ToolCall {
        /// Id of the `tool_use` content block this call belongs to.
        tool_use_id: String,
        /// Tool name.
        name: String,
        /// Short human summary of the key arguments.
        summary: String,
        /// Raw tool input as sent by the model.
        input: Value,
        /// Lifecycle state.
        status: ToolStatus,
        /// Tool output shown when expanded, once available.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<String>,
    },
    /// Tabular data (tool `show_table`).
    Table(TableSpec),
    /// A chart (tool `show_chart`); the UI can toggle it to a table.
    Chart(ChartSpec),
    /// A proposed file change (`edit_file`, `write_file`).
    Diff(DiffSpec),
    /// A Mermaid diagram (tool `show_mermaid`).
    Mermaid(MermaidSpec),
    /// A markdown document (tool `show_markdown` or a written `.md`), shown
    /// with document typography and toggleable to source.
    Markdown {
        /// Document title, e.g. the file name.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        /// Markdown source.
        source: String,
    },
    /// A block drawn by a Lua plugin from a declarative tree.
    Custom {
        /// Plugin that produced it.
        plugin: String,
        /// Block name within the plugin.
        name: String,
        /// Content to draw.
        tree: CustomNode,
    },
    /// Sources cited by a workflow node; citations in the text link here.
    Sources {
        /// Cited sources, in citation order.
        sources: Vec<SourceRef>,
    },
    /// An image (screenshot, exported chart, web image) with its source.
    Image {
        /// MIME type, e.g. `image/png`.
        media_type: String,
        /// Inline bytes or a path to read them from.
        data_or_path: ImageData,
        /// Where the image came from.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source: Option<SourceRef>,
    },
    /// Marks that the workflow entered a new stage, with progress
    /// (`index` of `total`, 1-based, e.g. 5/14).
    StageHeader {
        /// Stage name.
        stage: String,
        /// 1-based position of the stage.
        index: u32,
        /// Number of stages.
        total: u32,
    },
    /// A command the user ran with `/!` in the workspace, and what it
    /// printed.
    Shell {
        /// The command line as typed.
        command: String,
        /// Exit code; `None` when it was killed (signal or timeout) or could
        /// not start.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exit_code: Option<i32>,
        /// Standard output.
        #[serde(default)]
        stdout: String,
        /// Standard error (or why it could not start).
        #[serde(default)]
        stderr: String,
        /// Whether it was killed for taking too long.
        #[serde(default)]
        timed_out: bool,
        /// Whether the output was cut to the size limit.
        #[serde(default)]
        truncated: bool,
    },
    /// Failure of the API or a tool.
    Error {
        /// What went wrong.
        message: String,
        /// Suggested action for the user.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hint: Option<String>,
    },
    /// An engine-computed result (statistics, calc sheet, unit
    /// conversion) with its provenance.
    Computed {
        /// The numbers and how they were produced.
        envelope: Box<Computed>,
        /// What the card draws.
        body: Box<ComputedBody>,
    },
    /// A form the model asks the user to fill (tool `ask_user`).
    Form(FormSpec),
    /// Function plots with parameter sliders and numeric series (tool
    /// `show_plot`).
    Plot(PlotSpec),
    /// A test run report, e.g. from JUnit XML (tool `show_test_report`).
    TestReport(TestReportSpec),
    /// Static-analysis findings, e.g. from SARIF (tool `show_findings`).
    Findings(FindingsSpec),
    /// Schema, column statistics and first rows of a data file (tool
    /// `show_dataset`).
    Dataset(DatasetSpec),
    /// A 2D molecule with computed formula and mass (tool `show_molecule`).
    Molecule(MoleculeSpec),
    /// A 96- or 384-well plate map (tool `show_plate`).
    Plate(PlateSpec),
    /// A distributed trace as a waterfall (tool `show_trace`).
    Trace(Box<TraceSpec>),
    /// A profile as a flame graph (tool `show_flamegraph`).
    FlameGraph(Box<FlameSpec>),
    /// A log file on a filterable list (tool `show_logs`).
    Logs(Box<LogSpec>),
    /// A SQL query plan on a tree table (tool `show_query_plan`).
    QueryPlan(Box<QueryPlanSpec>),
    /// An HTTP archive as a waterfall, request list and inspector (tool
    /// `show_har`).
    Har(Box<HarSpec>),
    /// A Terraform plan's resource changes on a tree table (tool
    /// `show_terraform_plan`).
    TerraformPlan(Box<TerraformPlanSpec>),
    /// A DOT graph or an ER diagram from SQL DDL on the graph widget
    /// (tools `show_dot`, `show_er`).
    Diagram(Box<DiagramSpec>),
    /// Criterion benchmarks, baseline against new (tool
    /// `show_benchmarks`).
    Benchmarks(Box<BenchSpec>),
    /// Digital waveforms from a VCD, FST or GHW dump (tool
    /// `show_waveform`).
    Waveform(Box<WaveformSpec>),
    /// A SPICE or KiCad netlist as a component–net graph and a component
    /// table (tool `show_netlist`).
    Netlist(Box<NetlistSpec>),
    /// A material's properties, each with its source or marked
    /// unverified (tool `show_material`).
    Material(Box<MaterialSpec>),
    /// A STEP part tessellated into the 3D view, with its solids (tool
    /// `show_step`).
    CadModel(Box<CadModelSpec>),
    /// A 2D DXF drawing with its layers (tool `show_dxf`).
    Drawing(Box<DrawingSpec>),
    /// An IFC model's room schedule by storey (tool `show_ifc_schedule`).
    IfcSchedule(Box<IfcScheduleSpec>),
}

/// Lifecycle of a tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    /// Requested by the model, not started yet.
    Pending,
    /// Waiting for the user to approve it.
    AwaitingApproval,
    /// Executing.
    Running,
    /// Finished successfully.
    Succeeded,
    /// Finished with an error.
    Failed,
    /// Rejected by the user or a permission rule.
    Denied,
    /// Interrupted before finishing.
    Cancelled,
}

impl ToolStatus {
    /// Whether the call has reached a final state.
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::Denied | Self::Cancelled
        )
    }
}

/// Validation failure of a block payload.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum BlockError {
    /// A chart without series.
    #[error("chart has no series")]
    ChartNoSeries,
    /// A pie chart with other than one series.
    #[error("pie chart needs exactly one series, got {count}")]
    PieSeriesCount {
        /// Number of series given.
        count: usize,
    },
    /// A series whose length differs from the number of labels.
    #[error("series {series:?} has {actual} values, expected {expected}")]
    SeriesLength {
        /// Series name.
        series: String,
        /// Number of labels.
        expected: usize,
        /// Number of values.
        actual: usize,
    },
    /// A NaN or infinite value in a series.
    #[error("series {series:?} has a non-finite value at {index}")]
    NonFiniteValue {
        /// Series name.
        series: String,
        /// Zero-based index of the value.
        index: usize,
    },
    /// A file source with a line range that is not 1-based or is reversed.
    #[error("invalid line range {start:?}..={end:?}")]
    InvalidLineRange {
        /// First line.
        start: Option<u32>,
        /// Last line.
        end: Option<u32>,
    },
    /// A stage header whose index is not within `1..=total`.
    #[error("stage {index} out of range 1..={total}")]
    StageOutOfRange {
        /// Given index.
        index: u32,
        /// Number of stages.
        total: u32,
    },
    /// A table without columns.
    #[error("table has no columns")]
    TableNoColumns,
    /// An inline image without its MIME type.
    #[error("an inline image needs a media_type (e.g. image/png)")]
    ImageMediaType,
    /// An image file with an empty path.
    #[error("image path is empty")]
    ImageEmptyPath,
    /// A link whose URL is not `http`, `https` or `mailto`.
    #[error("link {url:?} must be an http, https or mailto URL")]
    LinkScheme {
        /// The rejected URL.
        url: String,
    },
    /// A 3D model that cannot be drawn.
    #[error("3D model: {0}")]
    Model(#[from] MeshError),
    /// An invalid computed envelope or card body.
    #[error("computed: {0}")]
    Computed(#[from] ComputedError),
    /// An invalid spec of a rich block (form, plot, dataset, plate...).
    #[error("{kind}: {reason}")]
    Invalid {
        /// Block kind, e.g. `form`.
        kind: &'static str,
        /// What is wrong.
        reason: String,
    },
    /// A table row whose width differs from the number of columns.
    #[error("table row {row} has {actual} cells, expected {expected}")]
    TableRowWidth {
        /// Zero-based row index.
        row: usize,
        /// Number of columns.
        expected: usize,
        /// Number of cells in the row.
        actual: usize,
    },
}

/// A table: named columns and rows of cells.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TableSpec {
    /// Column headers, in order.
    pub columns: Vec<Column>,
    /// Rows; each must have one cell per column.
    pub rows: Vec<Vec<Cell>>,
}

impl TableSpec {
    /// Checks that there is at least one column and every row has one cell
    /// per column.
    ///
    /// # Errors
    ///
    /// Returns the first [`BlockError`] found.
    pub fn validate(&self) -> Result<(), BlockError> {
        if self.columns.is_empty() {
            return Err(BlockError::TableNoColumns);
        }
        let expected = self.columns.len();
        for (row, cells) in self.rows.iter().enumerate() {
            if cells.len() != expected {
                return Err(BlockError::TableRowWidth {
                    row,
                    expected,
                    actual: cells.len(),
                });
            }
        }
        Ok(())
    }
}

/// A table column header.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Column {
    /// Header text.
    pub name: String,
    /// Horizontal alignment; the UI picks one when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub align: Option<Align>,
}

impl Column {
    /// A column with default alignment.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            align: None,
        }
    }
}

/// Horizontal alignment of a column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Align {
    /// Left aligned.
    Left,
    /// Centered.
    Center,
    /// Right aligned.
    Right,
}

/// A table cell: a plain JSON scalar.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Cell {
    /// Empty cell (`null`).
    Null,
    /// Boolean.
    Bool(bool),
    /// Number.
    Number(f64),
    /// Text.
    Text(String),
}

/// A chart: labels on one axis and one or more numeric series.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ChartSpec {
    /// Chart title.
    pub title: String,
    /// How the series are drawn.
    pub kind: ChartKind,
    /// Category labels, one per value of each series.
    pub labels: Vec<String>,
    /// Data series.
    pub series: Vec<Series>,
}

impl ChartSpec {
    /// Checks that there is at least one series, each with exactly one finite
    /// value per label, and that a pie chart has a single series.
    ///
    /// # Errors
    ///
    /// Returns the first [`BlockError`] found.
    pub fn validate(&self) -> Result<(), BlockError> {
        if self.series.is_empty() {
            return Err(BlockError::ChartNoSeries);
        }
        if self.kind == ChartKind::Pie && self.series.len() != 1 {
            return Err(BlockError::PieSeriesCount {
                count: self.series.len(),
            });
        }
        for series in &self.series {
            if series.values.len() != self.labels.len() {
                return Err(BlockError::SeriesLength {
                    series: series.name.clone(),
                    expected: self.labels.len(),
                    actual: series.values.len(),
                });
            }
            if let Some(index) = series.values.iter().position(|v| !v.is_finite()) {
                return Err(BlockError::NonFiniteValue {
                    series: series.name.clone(),
                    index,
                });
            }
        }
        Ok(())
    }

    /// The same data as a table: a label column plus one column per series.
    ///
    /// Missing values (a series shorter than `labels`) become [`Cell::Null`].
    #[must_use]
    pub fn to_table(&self) -> TableSpec {
        let columns = std::iter::once(Column::new(""))
            .chain(self.series.iter().map(|s| Column {
                name: s.name.clone(),
                align: Some(Align::Right),
            }))
            .collect();
        let rows = self
            .labels
            .iter()
            .enumerate()
            .map(|(i, label)| {
                std::iter::once(Cell::Text(label.clone()))
                    .chain(
                        self.series
                            .iter()
                            .map(|s| s.values.get(i).map_or(Cell::Null, |v| Cell::Number(*v))),
                    )
                    .collect()
            })
            .collect();
        TableSpec { columns, rows }
    }
}

/// How a chart is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChartKind {
    /// Vertical bars.
    Bar,
    /// Lines.
    Line,
    /// Filled areas.
    Area,
    /// Pie; takes exactly one series.
    Pie,
}

/// A named series of values, one per chart label.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Series {
    /// Legend name.
    pub name: String,
    /// Values, aligned with the chart labels.
    pub values: Vec<f64>,
}

/// A file change. The adapter computes the diff; this is only data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DiffSpec {
    /// Path of the file, relative to the workspace.
    pub path: String,
    /// Previous content; absent when the file is new.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_text: Option<String>,
    /// New content.
    pub new_text: String,
    /// Unified diff between both.
    pub unified: String,
    /// Approval state when the change needs the user's decision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<ApprovalState>,
}

/// Decision of the user on a proposed file change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalState {
    /// Waiting for the user.
    Pending,
    /// Accepted.
    Approved,
    /// Rejected.
    Rejected,
}

/// A Mermaid diagram; the UI shows the source and the error if it does not
/// parse.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MermaidSpec {
    /// Mermaid source.
    pub source: String,
    /// Optional caption.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// Declarative UI primitive of a `Custom` block, tagged by `type`.
///
/// Lua never touches GPUI: plugins return this tree and the UI turns it
/// into components.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CustomNode {
    /// Plain text.
    Text {
        /// The text.
        text: String,
    },
    /// Children laid out horizontally.
    Row {
        /// Child nodes.
        children: Vec<CustomNode>,
    },
    /// Children laid out vertically.
    Col {
        /// Child nodes.
        children: Vec<CustomNode>,
    },
    /// A bulleted list.
    List {
        /// One node per item.
        items: Vec<CustomNode>,
    },
    /// A table.
    Table(TableSpec),
    /// A chart.
    Chart(ChartSpec),
    /// A code snippet.
    Code {
        /// Language for highlighting.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lang: Option<String>,
        /// Source code.
        source: String,
    },
    /// Rendered markdown.
    Markdown {
        /// Markdown source.
        source: String,
    },
    /// A picture, from a file or inline bytes, with zoom and pan.
    Image {
        /// Inline bytes or a file. A file must be inside the workspace and
        /// not sensitive; the app replaces a rejected one with a text node.
        source: ImageData,
        /// MIME type, e.g. `image/png`; required for inline bytes.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        media_type: Option<String>,
        /// Text shown when the image cannot be drawn.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        alt: Option<String>,
        /// How the image is sized at first.
        #[serde(default)]
        fit: ImageFit,
    },
    /// A basic HTML subset (headings, paragraphs, lists, tables, links,
    /// emphasis, code), drawn natively; no CSS layout and no scripts.
    Html {
        /// HTML source.
        source: String,
    },
    /// A JSON value as a collapsible tree.
    Json {
        /// The value.
        value: Value,
        /// Levels expanded at first (0 shows only the root collapsed).
        #[serde(default = "default_expand_depth")]
        expand_depth: u32,
    },
    /// A Mermaid diagram, with pan and zoom.
    Mermaid {
        /// Mermaid source.
        source: String,
    },
    /// A link that opens in the user's browser.
    Link {
        /// Text shown.
        text: String,
        /// Address; `http`, `https` or `mailto` only.
        url: String,
    },
    /// A colored unified diff.
    Diff {
        /// Unified diff text.
        unified: String,
    },
    /// A 3D model (`/open` of an OBJ, STL or PLY file), with an orbit
    /// camera.
    Model3d(ModelSpec),
    /// A function plot with parameter sliders, as the `plot` block draws
    /// it.
    Plot(Box<crate::plot::PlotSpec>),
    /// Rows of spans on a time axis, with zoom, pan and a tooltip
    /// (traces, Gantt charts, waveforms).
    Timeline(Box<crate::widget::timeline::TimelineSpec>),
    /// Records (log lines, requests) with a text or regex filter, level
    /// and field chips, follow mode and rows that open to their fields.
    FilterList(Box<crate::widget::list::ListSpec>),
    /// Nodes joined by edges, laid out in layers, by forces or on a grid,
    /// with pan, zoom and a click to focus a node's neighbourhood (DOT,
    /// ER diagrams, netlists, networks).
    Graph(Box<crate::widget::graph::GraphSpec>),
    /// Nested rows with typed columns (text, numbers, durations, inline
    /// bars) and badges, collapsible (query plans, Terraform plans).
    Tree(Box<crate::widget::tree::TreeSpec>),
}

fn default_expand_depth() -> u32 {
    1
}

/// How an [`CustomNode::Image`] is sized before the user zooms.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ImageFit {
    /// Natural size, scaled down to the width available.
    #[default]
    Contain,
    /// Natural size (1:1); wider images scroll.
    Actual,
}

/// URL schemes a [`CustomNode::Link`] may open.
pub const LINK_SCHEMES: [&str; 3] = ["http", "https", "mailto"];

/// Whether `url` starts with one of [`LINK_SCHEMES`] (case-insensitive).
#[must_use]
pub fn is_openable_url(url: &str) -> bool {
    url.split_once(':').is_some_and(|(scheme, rest)| {
        !rest.is_empty()
            && LINK_SCHEMES
                .iter()
                .any(|allowed| scheme.eq_ignore_ascii_case(allowed))
    })
}

impl CustomNode {
    /// Validates every table, chart, image and link in the tree.
    ///
    /// # Errors
    ///
    /// Returns the first [`BlockError`] found, depth first.
    pub fn validate(&self) -> Result<(), BlockError> {
        match self {
            Self::Row { children } | Self::Col { children } | Self::List { items: children } => {
                children.iter().try_for_each(Self::validate)
            }
            Self::Table(spec) => spec.validate(),
            Self::Chart(spec) => spec.validate(),
            Self::Image {
                source, media_type, ..
            } => match source {
                ImageData::Base64 { .. }
                    if media_type.as_deref().is_none_or(|t| t.trim().is_empty()) =>
                {
                    Err(BlockError::ImageMediaType)
                }
                ImageData::Path { path } if path.trim().is_empty() => {
                    Err(BlockError::ImageEmptyPath)
                }
                _ => Ok(()),
            },
            Self::Link { url, .. } if !is_openable_url(url) => {
                Err(BlockError::LinkScheme { url: url.clone() })
            }
            Self::Model3d(spec) => Ok(spec.mesh.validate()?),
            Self::Plot(spec) => spec.validate(),
            Self::Timeline(spec) => spec.validate(),
            Self::FilterList(spec) => spec.validate(),
            Self::Graph(spec) => spec.validate(),
            Self::Tree(spec) => spec.validate(),
            Self::Text { .. }
            | Self::Code { .. }
            | Self::Markdown { .. }
            | Self::Html { .. }
            | Self::Json { .. }
            | Self::Mermaid { .. }
            | Self::Link { .. }
            | Self::Diff { .. } => Ok(()),
        }
    }

    /// Calls `check` with the path of every image read from a file, depth
    /// first. `Ok(path)` replaces the path (e.g. with the resolved one);
    /// `Err(reason)` replaces the whole image with a text node that says
    /// it is unavailable and why.
    pub fn check_image_paths(&mut self, check: &mut dyn FnMut(&str) -> Result<String, String>) {
        match self {
            Self::Row { children } | Self::Col { children } | Self::List { items: children } => {
                for child in children {
                    child.check_image_paths(check);
                }
            }
            Self::Image {
                source: ImageData::Path { path },
                alt,
                ..
            } => match check(path) {
                Ok(resolved) => *path = resolved,
                Err(reason) => {
                    let name = alt.as_deref().unwrap_or(path.as_str());
                    *self = Self::Text {
                        text: format!("[image unavailable: {name}: {reason}]"),
                    };
                }
            },
            _ => {}
        }
    }

    /// Whether the tree has an image read from a file.
    #[must_use]
    pub fn has_image_paths(&self) -> bool {
        match self {
            Self::Row { children } | Self::Col { children } | Self::List { items: children } => {
                children.iter().any(Self::has_image_paths)
            }
            Self::Image {
                source: ImageData::Path { .. },
                ..
            } => true,
            _ => false,
        }
    }
}

/// Where a piece of information came from, tagged by `type`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SourceRef {
    /// A file of the workspace, optionally a 1-based inclusive line range.
    File {
        /// Path relative to the workspace.
        path: String,
        /// First line.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        line_start: Option<u32>,
        /// Last line.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        line_end: Option<u32>,
    },
    /// A git commit.
    Commit {
        /// Commit hash.
        sha: String,
        /// First line of the message.
        summary: String,
    },
    /// A web page.
    Url {
        /// Address.
        url: String,
        /// Page title.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },
    /// Output of a shell command.
    Command {
        /// Command line.
        command: String,
        /// Relevant excerpt of its output.
        output_excerpt: String,
    },
}

impl SourceRef {
    /// Checks that a file line range is 1-based and not reversed.
    ///
    /// # Errors
    ///
    /// Returns [`BlockError::InvalidLineRange`] for a bad range.
    pub fn validate(&self) -> Result<(), BlockError> {
        if let Self::File {
            line_start,
            line_end,
            ..
        } = self
        {
            let bad_start = *line_start == Some(0);
            let reversed = matches!((line_start, line_end), (Some(s), Some(e)) if e < s);
            let end_only = line_start.is_none() && line_end.is_some();
            if bad_start || reversed || end_only {
                return Err(BlockError::InvalidLineRange {
                    start: *line_start,
                    end: *line_end,
                });
            }
        }
        Ok(())
    }
}

/// Bytes of an image block, tagged by `type`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ImageData {
    /// Inline, base64 encoded.
    Base64 {
        /// Base64 payload.
        data: String,
    },
    /// A file on disk.
    Path {
        /// File path.
        path: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn roundtrip(block: &Block) -> serde_json::Value {
        let value = serde_json::to_value(block).unwrap();
        let back: Block = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(&back, block);
        value
    }

    #[test]
    fn block_ids_are_unique() {
        assert_ne!(BlockId::new(), BlockId::new());
    }

    #[test]
    fn block_id_serializes_as_plain_uuid_string() {
        let id = BlockId(Uuid::nil());
        assert_eq!(
            serde_json::to_value(id).unwrap(),
            json!("00000000-0000-0000-0000-000000000000")
        );
        assert_eq!(id.to_string(), "00000000-0000-0000-0000-000000000000");
    }

    #[test]
    fn text_block_flattens_kind_with_type_tag() {
        let block = Block {
            id: BlockId(Uuid::nil()),
            kind: BlockKind::Text {
                markdown: "# hi".into(),
            },
        };
        assert_eq!(
            roundtrip(&block),
            json!({"id": "00000000-0000-0000-0000-000000000000", "type": "text", "markdown": "# hi"})
        );
    }

    #[test]
    fn thinking_and_error_roundtrip() {
        roundtrip(&Block::new(BlockKind::Thinking {
            text: "hmm".into(),
            collapsed: true,
        }));
        let error = Block::new(BlockKind::Error {
            message: "rate limited".into(),
            hint: None,
        });
        let value = roundtrip(&error);
        assert!(value.get("hint").is_none());
        roundtrip(&Block::new(BlockKind::Error {
            message: "rate limited".into(),
            hint: Some("retry later".into()),
        }));
    }

    #[test]
    fn tool_call_roundtrip_with_snake_case_status() {
        let block = Block::new(BlockKind::ToolCall {
            tool_use_id: "toolu_01".into(),
            name: "read_file".into(),
            summary: "src/main.rs".into(),
            input: json!({"path": "src/main.rs"}),
            status: ToolStatus::AwaitingApproval,
            output: None,
        });
        let value = roundtrip(&block);
        assert_eq!(value["type"], "tool_call");
        assert_eq!(value["status"], "awaiting_approval");
        assert!(value.get("output").is_none());
    }

    #[test]
    fn tool_status_terminal_states() {
        assert!(!ToolStatus::Pending.is_terminal());
        assert!(!ToolStatus::AwaitingApproval.is_terminal());
        assert!(!ToolStatus::Running.is_terminal());
        assert!(ToolStatus::Succeeded.is_terminal());
        assert!(ToolStatus::Failed.is_terminal());
        assert!(ToolStatus::Denied.is_terminal());
        assert!(ToolStatus::Cancelled.is_terminal());
    }

    fn table(columns: &[&str], rows: Vec<Vec<Cell>>) -> TableSpec {
        TableSpec {
            columns: columns.iter().map(|c| Column::new(*c)).collect(),
            rows,
        }
    }

    #[test]
    fn cells_are_plain_json_scalars() {
        let row = vec![
            Cell::Text("a".into()),
            Cell::Number(1.5),
            Cell::Bool(true),
            Cell::Null,
        ];
        let value = serde_json::to_value(&row).unwrap();
        assert_eq!(value, json!(["a", 1.5, true, null]));
        let back: Vec<Cell> = serde_json::from_value(value).unwrap();
        assert_eq!(back, row);
    }

    #[test]
    fn table_block_roundtrip() {
        let mut spec = table(
            &["name", "n"],
            vec![vec![Cell::Text("x".into()), Cell::Number(2.0)]],
        );
        spec.columns[1].align = Some(Align::Right);
        let value = roundtrip(&Block::new(BlockKind::Table(spec)));
        assert_eq!(value["type"], "table");
        assert_eq!(value["columns"][1]["align"], "right");
        assert!(value["columns"][0].get("align").is_none());
    }

    #[test]
    fn table_validate_accepts_matching_rows() {
        let spec = table(&["a", "b"], vec![vec![Cell::Null, Cell::Bool(false)]]);
        assert_eq!(spec.validate(), Ok(()));
        assert_eq!(table(&["a"], vec![]).validate(), Ok(()));
    }

    #[test]
    fn table_validate_rejects_bad_shapes() {
        assert_eq!(
            table(&[], vec![]).validate(),
            Err(BlockError::TableNoColumns)
        );
        let spec = table(
            &["a", "b"],
            vec![vec![Cell::Null, Cell::Null], vec![Cell::Null]],
        );
        assert_eq!(
            spec.validate(),
            Err(BlockError::TableRowWidth {
                row: 1,
                expected: 2,
                actual: 1
            })
        );
    }

    fn chart(kind: ChartKind, labels: &[&str], series: &[(&str, &[f64])]) -> ChartSpec {
        ChartSpec {
            title: "t".into(),
            kind,
            labels: labels.iter().map(|l| (*l).to_owned()).collect(),
            series: series
                .iter()
                .map(|(name, values)| Series {
                    name: (*name).to_owned(),
                    values: values.to_vec(),
                })
                .collect(),
        }
    }

    #[test]
    fn chart_block_roundtrip() {
        let spec = chart(ChartKind::Bar, &["a", "b"], &[("x", &[1.0, 2.0])]);
        let value = roundtrip(&Block::new(BlockKind::Chart(spec)));
        assert_eq!(value["type"], "chart");
        assert_eq!(value["kind"], "bar");
    }

    #[test]
    fn chart_accepts_integer_json_values() {
        let spec: ChartSpec = serde_json::from_value(json!({
            "title": "t", "kind": "line", "labels": ["a"], "series": [{"name": "x", "values": [3]}]
        }))
        .unwrap();
        assert_eq!(spec.series[0].values, vec![3.0]);
    }

    #[test]
    fn chart_validate_accepts_aligned_series() {
        let spec = chart(
            ChartKind::Line,
            &["a", "b"],
            &[("x", &[1.0, 2.0]), ("y", &[3.0, 4.0])],
        );
        assert_eq!(spec.validate(), Ok(()));
        assert_eq!(
            chart(ChartKind::Pie, &["a"], &[("x", &[1.0])]).validate(),
            Ok(())
        );
    }

    #[test]
    fn chart_validate_rejects_bad_shapes() {
        assert_eq!(
            chart(ChartKind::Bar, &["a"], &[]).validate(),
            Err(BlockError::ChartNoSeries)
        );
        assert_eq!(
            chart(ChartKind::Pie, &["a"], &[("x", &[1.0]), ("y", &[2.0])]).validate(),
            Err(BlockError::PieSeriesCount { count: 2 })
        );
        assert_eq!(
            chart(ChartKind::Area, &["a", "b"], &[("x", &[1.0])]).validate(),
            Err(BlockError::SeriesLength {
                series: "x".into(),
                expected: 2,
                actual: 1
            })
        );
        assert_eq!(
            chart(ChartKind::Bar, &["a", "b"], &[("x", &[1.0, f64::NAN])]).validate(),
            Err(BlockError::NonFiniteValue {
                series: "x".into(),
                index: 1
            })
        );
    }

    #[test]
    fn chart_to_table_has_label_column_and_one_column_per_series() {
        let spec = chart(
            ChartKind::Bar,
            &["a", "b"],
            &[("x", &[1.0, 2.0]), ("y", &[3.0])],
        );
        let table = spec.to_table();
        let names: Vec<_> = table.columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["", "x", "y"]);
        assert_eq!(
            table.rows,
            vec![
                vec![Cell::Text("a".into()), Cell::Number(1.0), Cell::Number(3.0)],
                vec![Cell::Text("b".into()), Cell::Number(2.0), Cell::Null],
            ]
        );
        assert_eq!(table.validate(), Ok(()));
    }

    #[test]
    fn diff_block_roundtrip() {
        let new_file = DiffSpec {
            path: "src/lib.rs".into(),
            old_text: None,
            new_text: "fn a() {}\n".into(),
            unified: "+fn a() {}\n".into(),
            approval: Some(ApprovalState::Pending),
        };
        let value = roundtrip(&Block::new(BlockKind::Diff(new_file.clone())));
        assert_eq!(value["type"], "diff");
        assert_eq!(value["approval"], "pending");
        assert!(value.get("old_text").is_none());
        roundtrip(&Block::new(BlockKind::Diff(DiffSpec {
            old_text: Some(String::new()),
            approval: None,
            ..new_file
        })));
    }

    #[test]
    fn mermaid_block_roundtrip() {
        let value = roundtrip(&Block::new(BlockKind::Mermaid(MermaidSpec {
            source: "graph TD; a-->b".into(),
            title: None,
        })));
        assert_eq!(value["type"], "mermaid");
        assert!(value.get("title").is_none());
    }

    #[test]
    fn markdown_block_roundtrip() {
        let value = roundtrip(&Block::new(BlockKind::Markdown {
            title: Some("spec.md".into()),
            source: "# Spec".into(),
        }));
        assert_eq!(value["type"], "markdown");
        assert_eq!(value["title"], "spec.md");
    }

    #[test]
    fn custom_block_roundtrip_with_nested_tree() {
        let tree = CustomNode::Col {
            children: vec![
                CustomNode::Text {
                    text: "todo".into(),
                },
                CustomNode::List {
                    items: vec![CustomNode::Markdown {
                        source: "*a*".into(),
                    }],
                },
                CustomNode::Row {
                    children: vec![CustomNode::Code {
                        lang: Some("rust".into()),
                        source: "fn a() {}".into(),
                    }],
                },
                CustomNode::Table(table(&["a"], vec![vec![Cell::Number(1.0)]])),
            ],
        };
        let value = roundtrip(&Block::new(BlockKind::Custom {
            plugin: "todo".into(),
            name: "todo".into(),
            tree,
        }));
        assert_eq!(value["type"], "custom");
        assert_eq!(value["tree"]["type"], "col");
        assert_eq!(value["tree"]["children"][1]["type"], "list");
        assert_eq!(value["tree"]["children"][3]["type"], "table");
        assert_eq!(value["tree"]["children"][3]["columns"][0]["name"], "a");
    }

    #[test]
    fn custom_validate_finds_nested_errors() {
        let ok = CustomNode::Row {
            children: vec![CustomNode::Chart(chart(
                ChartKind::Bar,
                &["a"],
                &[("x", &[1.0])],
            ))],
        };
        assert_eq!(ok.validate(), Ok(()));
        let bad = CustomNode::Col {
            children: vec![CustomNode::List {
                items: vec![CustomNode::Table(table(&[], vec![]))],
            }],
        };
        assert_eq!(bad.validate(), Err(BlockError::TableNoColumns));
    }

    #[test]
    fn rich_nodes_roundtrip_and_validate() {
        let tree = CustomNode::Col {
            children: vec![
                CustomNode::Image {
                    source: ImageData::Path {
                        path: "out/a.png".into(),
                    },
                    media_type: None,
                    alt: Some("plot".into()),
                    fit: ImageFit::Actual,
                },
                CustomNode::Html {
                    source: "<b>hi</b>".into(),
                },
                CustomNode::Json {
                    value: json!({"a": [1, 2]}),
                    expand_depth: 2,
                },
                CustomNode::Mermaid {
                    source: "graph TD; a-->b".into(),
                },
                CustomNode::Link {
                    text: "docs".into(),
                    url: "https://example.com".into(),
                },
                CustomNode::Diff {
                    unified: "@@ -1 +1 @@\n-a\n+b\n".into(),
                },
            ],
        };
        assert_eq!(tree.validate(), Ok(()));
        let value = serde_json::to_value(&tree).unwrap();
        assert_eq!(value["children"][0]["type"], "image");
        assert_eq!(value["children"][0]["source"]["type"], "path");
        assert_eq!(value["children"][0]["fit"], "actual");
        assert_eq!(value["children"][2]["expand_depth"], 2);
        let back: CustomNode = serde_json::from_value(value).unwrap();
        assert_eq!(back, tree);
        let defaults: CustomNode = serde_json::from_value(json!({
            "type": "json", "value": null
        }))
        .unwrap();
        assert_eq!(
            defaults,
            CustomNode::Json {
                value: Value::Null,
                expand_depth: 1
            }
        );
        let image: CustomNode = serde_json::from_value(json!({
            "type": "image", "source": {"type": "path", "path": "a.png"}
        }))
        .unwrap();
        assert!(matches!(
            image,
            CustomNode::Image {
                fit: ImageFit::Contain,
                ..
            }
        ));
    }

    #[test]
    fn bad_images_and_links_are_rejected() {
        let inline = CustomNode::Image {
            source: ImageData::Base64 {
                data: "AA==".into(),
            },
            media_type: None,
            alt: None,
            fit: ImageFit::Contain,
        };
        assert_eq!(inline.validate(), Err(BlockError::ImageMediaType));
        let empty = CustomNode::Image {
            source: ImageData::Path { path: " ".into() },
            media_type: None,
            alt: None,
            fit: ImageFit::Contain,
        };
        assert_eq!(empty.validate(), Err(BlockError::ImageEmptyPath));
        for url in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "example.com",
            "https:",
        ] {
            let link = CustomNode::Row {
                children: vec![CustomNode::Link {
                    text: "x".into(),
                    url: url.into(),
                }],
            };
            assert_eq!(
                link.validate(),
                Err(BlockError::LinkScheme { url: url.into() }),
                "{url}"
            );
        }
        for url in ["https://a.b/c", "HTTP://a", "mailto:me@example.com"] {
            assert!(is_openable_url(url), "{url}");
        }
    }

    #[test]
    fn image_paths_are_checked_and_replaced() {
        let image = |path: &str| CustomNode::Image {
            source: ImageData::Path { path: path.into() },
            media_type: None,
            alt: None,
            fit: ImageFit::Contain,
        };
        let mut tree = CustomNode::Col {
            children: vec![
                image("ok.png"),
                CustomNode::List {
                    items: vec![image(".env")],
                },
                CustomNode::Image {
                    source: ImageData::Base64 {
                        data: "AA==".into(),
                    },
                    media_type: Some("image/png".into()),
                    alt: None,
                    fit: ImageFit::Contain,
                },
            ],
        };
        assert!(tree.has_image_paths());
        let mut seen = Vec::new();
        tree.check_image_paths(&mut |path| {
            seen.push(path.to_owned());
            if path == "ok.png" {
                Ok("/ws/ok.png".into())
            } else {
                Err("sensitive".into())
            }
        });
        assert_eq!(seen, ["ok.png", ".env"]);
        let CustomNode::Col { children } = &tree else {
            panic!("not a col")
        };
        assert_eq!(children[0], image("/ws/ok.png"));
        assert_eq!(
            children[1],
            CustomNode::List {
                items: vec![CustomNode::Text {
                    text: "[image unavailable: .env: sensitive]".into()
                }]
            }
        );
        assert!(!children[2].has_image_paths());
    }

    fn file(start: Option<u32>, end: Option<u32>) -> SourceRef {
        SourceRef::File {
            path: "src/main.rs".into(),
            line_start: start,
            line_end: end,
        }
    }

    #[test]
    fn sources_block_roundtrip() {
        let value = roundtrip(&Block::new(BlockKind::Sources {
            sources: vec![
                file(Some(3), Some(9)),
                SourceRef::Commit {
                    sha: "abc123".into(),
                    summary: "fix".into(),
                },
                SourceRef::Url {
                    url: "https://example.com".into(),
                    title: None,
                },
                SourceRef::Command {
                    command: "git log -1".into(),
                    output_excerpt: "commit abc123".into(),
                },
            ],
        }));
        assert_eq!(value["type"], "sources");
        assert_eq!(value["sources"][0]["type"], "file");
        assert_eq!(value["sources"][3]["type"], "command");
    }

    #[test]
    fn source_ref_validates_line_ranges() {
        assert_eq!(file(None, None).validate(), Ok(()));
        assert_eq!(file(Some(1), None).validate(), Ok(()));
        assert_eq!(file(Some(4), Some(4)).validate(), Ok(()));
        for (start, end) in [(Some(0), None), (Some(5), Some(4)), (None, Some(2))] {
            assert_eq!(
                file(start, end).validate(),
                Err(BlockError::InvalidLineRange { start, end })
            );
        }
    }

    #[test]
    fn image_block_roundtrip() {
        let value = roundtrip(&Block::new(BlockKind::Image {
            media_type: "image/png".into(),
            data_or_path: ImageData::Base64 {
                data: "iVBORw0KGgo=".into(),
            },
            source: Some(SourceRef::Url {
                url: "https://example.com/a.png".into(),
                title: Some("a".into()),
            }),
        }));
        assert_eq!(value["type"], "image");
        assert_eq!(value["data_or_path"]["type"], "base64");
        assert_eq!(value["source"]["type"], "url");
        roundtrip(&Block::new(BlockKind::Image {
            media_type: "image/svg+xml".into(),
            data_or_path: ImageData::Path {
                path: "out/chart.svg".into(),
            },
            source: None,
        }));
    }

    #[test]
    fn stage_header_roundtrip() {
        let value = roundtrip(&Block::new(BlockKind::StageHeader {
            stage: "design".into(),
            index: 5,
            total: 14,
        }));
        assert_eq!(value["type"], "stage_header");
        assert_eq!(value["index"], 5);
    }

    #[test]
    fn block_validate_dispatches_to_payloads() {
        let ok = [
            BlockKind::Text {
                markdown: String::new(),
            },
            BlockKind::Chart(chart(ChartKind::Pie, &["a"], &[("x", &[1.0])])),
            BlockKind::StageHeader {
                stage: "s".into(),
                index: 1,
                total: 1,
            },
        ];
        for kind in ok {
            assert_eq!(Block::new(kind).validate(), Ok(()));
        }
        let bad = [
            (
                BlockKind::Table(table(&[], vec![])),
                BlockError::TableNoColumns,
            ),
            (
                BlockKind::Chart(chart(ChartKind::Bar, &["a"], &[])),
                BlockError::ChartNoSeries,
            ),
            (
                BlockKind::Custom {
                    plugin: "p".into(),
                    name: "n".into(),
                    tree: CustomNode::Table(table(&[], vec![])),
                },
                BlockError::TableNoColumns,
            ),
            (
                BlockKind::Sources {
                    sources: vec![file(Some(0), None)],
                },
                BlockError::InvalidLineRange {
                    start: Some(0),
                    end: None,
                },
            ),
            (
                BlockKind::Image {
                    media_type: "image/png".into(),
                    data_or_path: ImageData::Path {
                        path: "a.png".into(),
                    },
                    source: Some(file(Some(3), Some(2))),
                },
                BlockError::InvalidLineRange {
                    start: Some(3),
                    end: Some(2),
                },
            ),
            (
                BlockKind::StageHeader {
                    stage: "s".into(),
                    index: 0,
                    total: 3,
                },
                BlockError::StageOutOfRange { index: 0, total: 3 },
            ),
            (
                BlockKind::StageHeader {
                    stage: "s".into(),
                    index: 4,
                    total: 3,
                },
                BlockError::StageOutOfRange { index: 4, total: 3 },
            ),
        ];
        for (kind, err) in bad {
            assert_eq!(Block::new(kind).validate(), Err(err));
        }
    }

    #[test]
    fn computed_block_roundtrips_and_validates() {
        use crate::computed::{
            CalcLine, CalcSheetBody, ComputedValue, DescriptiveBody, Engine, Histogram,
            VariableKind, VariableSummary,
        };
        let mut envelope = Computed::new(
            "c1",
            "calc.sheet",
            "numbat",
            Engine {
                name: "crest-calc".into(),
                version: "0.15.0".into(),
                backend: None,
                platform: None,
            },
        );
        envelope.values.insert("g".into(), ComputedValue::new(9.81));
        let block = Block::new(BlockKind::Computed {
            envelope: Box::new(envelope.clone()),
            body: Box::new(ComputedBody::CalcSheet(CalcSheetBody {
                lines: vec![CalcLine {
                    name: Some("g".into()),
                    expr: "9.81 m/s^2".into(),
                    value: Some(9.81),
                    display: None,
                    unit: Some("m/s²".into()),
                    latex: None,
                    dimension: Some("Acceleration".into()),
                    error: None,
                    notes: Vec::new(),
                }],
            })),
        });
        let value = roundtrip(&block);
        assert_eq!(value["type"], "computed");
        assert_eq!(value["envelope"]["crest_computed"], "1.0");
        assert_eq!(value["body"]["type"], "calc_sheet");
        assert_eq!(block.validate(), Ok(()));

        envelope.id = "bad id".into();
        let bad = Block::new(BlockKind::Computed {
            envelope: Box::new(envelope.clone()),
            body: Box::new(ComputedBody::CalcSheet(CalcSheetBody { lines: vec![] })),
        });
        assert!(matches!(bad.validate(), Err(BlockError::Computed(_))));
        envelope.id = "c1".into();
        let bad_body = Block::new(BlockKind::Computed {
            envelope: Box::new(envelope),
            body: Box::new(ComputedBody::Descriptive(DescriptiveBody {
                by: None,
                variables: vec![VariableSummary {
                    name: "x".into(),
                    group: None,
                    kind: VariableKind::Numeric,
                    unit: None,
                    n: 1,
                    missing: 0,
                    mean: None,
                    sd: None,
                    median: None,
                    q1: None,
                    q3: None,
                    min: None,
                    max: None,
                    skew: None,
                    kurtosis: None,
                    histogram: Some(Histogram {
                        edges: vec![0.0],
                        counts: vec![1],
                    }),
                    levels: vec![],
                }],
            })),
        });
        assert!(matches!(bad_body.validate(), Err(BlockError::Computed(_))));
    }

    #[test]
    fn new_rich_kinds_are_tagged_and_validated() {
        let plate: Block = serde_json::from_value(json!({
            "id": "00000000-0000-0000-0000-000000000000",
            "type": "plate", "format": 96, "wells": [{"well": "Z9"}]
        }))
        .unwrap();
        assert!(matches!(
            plate.validate(),
            Err(BlockError::Invalid { kind: "plate", .. })
        ));
        for (tag, body) in [
            ("form", json!({"title": "t", "fields": []})),
            (
                "plot",
                json!({"series": [{"name": "s", "x": [1], "y": [2]}]}),
            ),
            ("test_report", json!({"suites": []})),
            ("findings", json!({"findings": []})),
            ("molecule", json!({"smiles": "CCO"})),
            (
                "dataset",
                json!({"path": "a.csv", "format": "csv", "columns": [],
                       "preview": {"columns": [{"name": "a"}], "rows": []}}),
            ),
            (
                "trace",
                json!({"format": "otlp", "traces": 1, "total_spans": 1,
                       "timeline": {"rows": [{"id": "s1", "label": "GET /",
                                              "spans": [{"start": 0, "end": 5}]}]},
                       "critical_path": ["s1"]}),
            ),
            (
                "flame_graph",
                json!({"format": "folded", "unit": "samples", "nodes": [
                    {"name": "all", "depth": 0, "start": 0, "total": 3, "self_value": 0},
                    {"name": "main", "depth": 1, "start": 0, "total": 3, "self_value": 3}
                ]}),
            ),
            (
                "logs",
                json!({"format": "logfmt", "total_lines": 1,
                       "list": {"items": [{"level": "info", "text": "hi"}]}}),
            ),
        ] {
            let mut value = body;
            value["id"] = json!("00000000-0000-0000-0000-000000000000");
            value["type"] = json!(tag);
            let block: Block = serde_json::from_value(value).unwrap();
            assert_eq!(block.validate(), Ok(()), "{tag}");
            assert_eq!(roundtrip(&block)["type"], tag);
        }
    }

    #[test]
    fn json_schemas() {
        insta::assert_json_snapshot!("block_schema", schemars::schema_for!(Block));
        insta::assert_json_snapshot!("chart_spec_schema", schemars::schema_for!(ChartSpec));
        insta::assert_json_snapshot!("table_spec_schema", schemars::schema_for!(TableSpec));
        insta::assert_json_snapshot!("mermaid_spec_schema", schemars::schema_for!(MermaidSpec));
        insta::assert_json_snapshot!("diff_spec_schema", schemars::schema_for!(DiffSpec));
        insta::assert_json_snapshot!("custom_node_schema", schemars::schema_for!(CustomNode));
        insta::assert_json_snapshot!("source_ref_schema", schemars::schema_for!(SourceRef));
    }
}
