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
    /// Failure of the API or a tool.
    Error {
        /// What went wrong.
        message: String,
        /// Suggested action for the user.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hint: Option<String>,
    },
}

/// Lifecycle of a [`BlockKind::ToolCall`].
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
    /// A table without columns.
    #[error("table has no columns")]
    TableNoColumns,
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

/// How a [`ChartSpec`] is drawn.
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

/// Decision of the user on a [`DiffSpec`].
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
}
