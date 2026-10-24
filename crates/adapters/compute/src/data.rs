//! Reading request columns: numeric values, group labels, missing rows.

use application::ports::compute::{ColumnValues, ComputeError, DataColumn, DataSet};

/// The rows of a column as numbers; `None` is missing (NaN counts as
/// missing too).
pub fn numeric(data: &DataSet, name: &str) -> Result<Vec<Option<f64>>, ComputeError> {
    let column = column(data, name)?;
    match &column.values {
        ColumnValues::Numeric(values) => {
            Ok(values.iter().map(|v| v.filter(|x| x.is_finite())).collect())
        }
        ColumnValues::Categorical(_) => Err(ComputeError::Invalid(format!(
            "column {name:?} is categorical; a numeric column is needed"
        ))),
    }
}

/// The column called `name`, after checking that every column has the
/// same length.
pub fn column<'a>(data: &'a DataSet, name: &str) -> Result<&'a DataColumn, ComputeError> {
    check_lengths(data)?;
    data.column(name).ok_or_else(|| {
        let known: Vec<&str> = data.columns.iter().map(|c| c.name.as_str()).collect();
        ComputeError::Invalid(format!(
            "unknown column {name:?}; the data has {}",
            if known.is_empty() {
                "no columns".to_owned()
            } else {
                known.join(", ")
            }
        ))
    })
}

fn check_lengths(data: &DataSet) -> Result<(), ComputeError> {
    let mut lengths = data.columns.iter().map(DataColumn::len);
    if let Some(first) = lengths.next()
        && lengths.any(|len| len != first)
    {
        return Err(ComputeError::Invalid(
            "all columns must have the same number of rows".into(),
        ));
    }
    Ok(())
}

/// Whether the column is numeric.
pub fn is_numeric(column: &DataColumn) -> bool {
    matches!(column.values, ColumnValues::Numeric(_))
}

/// A number as a level label: `1` for whole numbers, else the shortest
/// round-trip text.
pub fn level_label(x: f64) -> String {
    if x.fract() == 0.0 && x.abs() < 1e15 {
        format!("{x:.0}")
    } else {
        format!("{x}")
    }
}

/// The rows of a column as level labels (`None` = missing), and its
/// levels in order: numerically sorted for numeric columns, sorted text
/// otherwise.
pub fn labels(
    data: &DataSet,
    name: &str,
) -> Result<(Vec<Option<String>>, Vec<String>), ComputeError> {
    let column = column(data, name)?;
    match &column.values {
        ColumnValues::Numeric(values) => {
            let mut levels: Vec<f64> = values
                .iter()
                .flatten()
                .copied()
                .filter(|x| x.is_finite())
                .collect();
            levels.sort_by(f64::total_cmp);
            levels.dedup();
            let rows = values
                .iter()
                .map(|v| v.filter(|x| x.is_finite()).map(level_label))
                .collect();
            Ok((rows, levels.into_iter().map(level_label).collect()))
        }
        ColumnValues::Categorical(values) => {
            let mut levels: Vec<String> = values.iter().flatten().cloned().collect();
            levels.sort();
            levels.dedup();
            Ok((values.clone(), levels))
        }
    }
}

/// Non-missing values.
pub fn present(values: &[Option<f64>]) -> Vec<f64> {
    values.iter().flatten().copied().collect()
}

/// Number of missing values.
pub fn missing<T>(values: &[Option<T>]) -> u64 {
    values.iter().filter(|v| v.is_none()).count() as u64
}

/// The columns the computation read, for the input hash and the reproduce
/// lines; unknown names are skipped.
pub fn used_columns(data: &DataSet, names: &[&str]) -> Vec<DataColumn> {
    names
        .iter()
        .filter_map(|name| data.column(name).cloned())
        .collect()
}

/// A value key part made of the characters slots accept.
pub fn key_part(name: &str) -> String {
    let key: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if key.is_empty() { "_".into() } else { key }
}
