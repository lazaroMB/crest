//! A dataset preview (`show_dataset`): the schema of a CSV, Parquet or
//! Arrow file with per-column statistics, plus its first rows.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::block::{BlockError, Cell, TableSpec};
use crate::computed::Histogram;

/// A dataset preview.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DatasetSpec {
    /// Workspace-relative path of the file.
    pub path: String,
    /// File format.
    pub format: DatasetFormat,
    /// Total number of rows, when known without reading everything.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rows: Option<u64>,
    /// Whether the statistics come from a sample of the rows.
    #[serde(default)]
    pub sampled: bool,
    /// One entry per column, in file order.
    pub columns: Vec<DatasetColumn>,
    /// The first rows.
    pub preview: TableSpec,
}

/// Format of a dataset file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DatasetFormat {
    /// Comma-separated values.
    Csv,
    /// Tab-separated values.
    Tsv,
    /// Apache Parquet.
    Parquet,
    /// Arrow IPC.
    Arrow,
    /// JSON lines.
    Jsonl,
}

/// Schema and statistics of one column.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DatasetColumn {
    /// Column name.
    pub name: String,
    /// Data type as the file or the inference names it, e.g. `int64`,
    /// `utf8`, `timestamp[ms]`.
    pub dtype: String,
    /// Share of null values, 0–100.
    pub null_percent: f64,
    /// Number of distinct values, when computed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distinct: Option<u64>,
    /// Smallest value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<Cell>,
    /// Largest value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<Cell>,
    /// Distribution for the sparkline (numeric columns).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub histogram: Option<Histogram>,
}

fn invalid(reason: String) -> BlockError {
    BlockError::Invalid {
        kind: "dataset",
        reason,
    }
}

impl DatasetSpec {
    /// Checks the preview table, null shares within 0–100 and histogram
    /// shapes.
    ///
    /// # Errors
    ///
    /// The first [`BlockError`] found.
    pub fn validate(&self) -> Result<(), BlockError> {
        self.preview.validate()?;
        for column in &self.columns {
            if !(0.0..=100.0).contains(&column.null_percent) {
                return Err(invalid(format!(
                    "column {:?} has null_percent outside 0–100",
                    column.name
                )));
            }
            if let Some(histogram) = &column.histogram {
                histogram.validate()?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn dataset_roundtrips_and_validates() {
        let spec: DatasetSpec = serde_json::from_value(json!({
            "path": "data/cars.csv", "format": "csv", "rows": 32,
            "columns": [
                {"name": "mpg", "dtype": "float64", "null_percent": 0, "distinct": 25,
                 "min": 10.4, "max": 33.9, "histogram": {"edges": [10, 20, 34], "counts": [14, 18]}},
                {"name": "model", "dtype": "utf8", "null_percent": 3.1, "min": "AMC", "max": "Volvo"}
            ],
            "preview": {"columns": [{"name": "mpg"}, {"name": "model"}],
                        "rows": [[21.0, "Mazda RX4"]]}
        }))
        .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(spec.validate(), Ok(()));
        assert_eq!(spec.columns[1].min, Some(Cell::Text("AMC".into())));
        let back: DatasetSpec =
            serde_json::from_value(serde_json::to_value(&spec).unwrap()).unwrap();
        assert_eq!(back, spec);

        let mut bad = spec;
        bad.columns[0].null_percent = 120.0;
        assert!(bad.validate().is_err());
    }
}
