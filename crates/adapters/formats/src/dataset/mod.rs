//! Dataset previews: the schema, per-column statistics and first rows of
//! a CSV, TSV, JSON lines or Parquet file.
//!
//! Each reader ([`text`], [`parquet`]) turns the bytes into a [`Table`] of
//! typed columns holding at most [`MAX_PROFILE_ROWS`] rows; [`preview`]
//! then profiles every column ([`profile`]) and keeps the first rows for
//! the preview table.

pub mod parquet;
pub mod profile;
pub mod text;

use application::ports::formats::DatasetRequest;
use domain::block::{Column, TableSpec};
use domain::dataset::{DatasetFormat, DatasetSpec};

use self::profile::Value;

/// Most rows the statistics cover; past it the block says "sampled".
pub const MAX_PROFILE_ROWS: usize = 100_000;

/// Most columns kept; wider files are cut.
pub const MAX_COLUMNS: usize = 500;

/// What a reader produces: typed columns of the profiled rows.
#[derive(Debug, Clone, PartialEq)]
pub struct Table {
    /// Column names, in file order.
    pub names: Vec<String>,
    /// Type name per column.
    pub dtypes: Vec<String>,
    /// Values per column, all of the same length.
    pub columns: Vec<Vec<Value>>,
    /// Rows in the whole file, when known.
    pub total_rows: Option<u64>,
    /// Whether `columns` holds fewer rows than the file.
    pub sampled: bool,
}

/// Reads `bytes` as `request.format` into a dataset block.
///
/// # Errors
///
/// The bytes do not parse, or the format is not supported (Arrow IPC).
pub fn preview(bytes: &[u8], request: &DatasetRequest) -> Result<DatasetSpec, String> {
    let truncated = request.truncated;
    let mut table = match request.format {
        DatasetFormat::Csv => text::delimited(bytes, ',', truncated)?,
        DatasetFormat::Tsv => text::delimited(bytes, '\t', truncated)?,
        DatasetFormat::Jsonl => text::json_lines(bytes, truncated)?,
        DatasetFormat::Parquet => parquet::read(bytes)?,
        DatasetFormat::Arrow => {
            return Err("Arrow IPC files are not supported; convert to Parquet or CSV".into());
        }
    };
    table.names.truncate(MAX_COLUMNS);
    table.dtypes.truncate(MAX_COLUMNS);
    table.columns.truncate(MAX_COLUMNS);
    let columns = table
        .names
        .iter()
        .zip(&table.dtypes)
        .zip(&table.columns)
        .map(|((name, dtype), values)| profile::column(name, dtype, values))
        .collect();
    let rows = table.columns.first().map_or(0, Vec::len);
    let preview = TableSpec {
        columns: table
            .names
            .iter()
            .map(|name| Column {
                name: name.clone(),
                align: None,
            })
            .collect(),
        rows: (0..rows.min(request.preview_rows))
            .map(|r| table.columns.iter().map(|c| c[r].cell()).collect())
            .collect(),
    };
    Ok(DatasetSpec {
        path: request.path.clone(),
        format: request.format,
        rows: table.total_rows,
        sampled: table.sampled,
        columns,
        preview,
    })
}
