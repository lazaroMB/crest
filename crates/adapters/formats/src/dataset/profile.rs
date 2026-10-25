//! Per-column statistics shared by every dataset reader: null share,
//! distinct count (exact up to [`DISTINCT_CAP`]), min/max and, for
//! numeric columns, a histogram for the sparkline.

use std::collections::HashSet;

use domain::block::Cell;
use domain::computed::Histogram;
use domain::dataset::DatasetColumn;

/// Distinct values are counted exactly up to this many; past it the
/// column says "at least".
pub const DISTINCT_CAP: usize = 10_000;

/// Most bins of a column histogram.
pub const MAX_BINS: usize = 20;

/// Longest text kept in a preview cell or a min/max, in characters.
pub const MAX_TEXT_CHARS: usize = 120;

/// One cell, typed by the reader.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// Missing.
    Null,
    /// Boolean.
    Bool(bool),
    /// Whole number.
    Int(i64),
    /// Floating-point number (may be NaN or infinite in Parquet).
    Float(f64),
    /// Anything else, as text (dates in ISO form sort correctly as text).
    Text(String),
}

impl Value {
    /// The value as a preview cell; long text is cut.
    #[must_use]
    pub fn cell(&self) -> Cell {
        match self {
            Self::Null => Cell::Null,
            Self::Bool(b) => Cell::Bool(*b),
            // Integers beyond f64's exact range stay readable as text.
            #[allow(clippy::cast_precision_loss)]
            Self::Int(i) if i.unsigned_abs() <= (1 << 53) => Cell::Number(*i as f64),
            Self::Int(i) => Cell::Text(i.to_string()),
            Self::Float(f) if f.is_finite() => Cell::Number(*f),
            Self::Float(f) => Cell::Text(f.to_string()),
            Self::Text(text) => Cell::Text(shorten(text)),
        }
    }

    fn number(&self) -> Option<f64> {
        match self {
            #[allow(clippy::cast_precision_loss)]
            Self::Int(i) => Some(*i as f64),
            Self::Float(f) if f.is_finite() => Some(*f),
            _ => None,
        }
    }
}

/// `text` cut to [`MAX_TEXT_CHARS`] characters with an ellipsis.
#[must_use]
pub fn shorten(text: &str) -> String {
    if text.chars().count() > MAX_TEXT_CHARS {
        let cut: String = text.chars().take(MAX_TEXT_CHARS).collect();
        format!("{cut}…")
    } else {
        text.to_owned()
    }
}

/// Key for distinct counting: floats by bits, so `1.0` and `1` differ
/// only when their types do.
#[derive(Debug, PartialEq, Eq, Hash)]
enum Key<'a> {
    Bool(bool),
    Int(i64),
    Float(u64),
    Text(&'a str),
}

/// The statistics of one column from its profiled `values`.
#[must_use]
pub fn column(name: &str, dtype: &str, values: &[Value]) -> DatasetColumn {
    let nulls = values.iter().filter(|v| **v == Value::Null).count();
    #[allow(clippy::cast_precision_loss)]
    let null_percent = if values.is_empty() {
        0.0
    } else {
        (nulls as f64 / values.len() as f64 * 100.0).clamp(0.0, 100.0)
    };
    let mut distinct = HashSet::new();
    let mut capped = false;
    for value in values {
        let key = match value {
            Value::Null => continue,
            Value::Bool(b) => Key::Bool(*b),
            Value::Int(i) => Key::Int(*i),
            Value::Float(f) => Key::Float(f.to_bits()),
            Value::Text(t) => Key::Text(t),
        };
        if distinct.len() >= DISTINCT_CAP && !distinct.contains(&key) {
            capped = true;
            break;
        }
        distinct.insert(key);
    }
    let numbers: Vec<f64> = values.iter().filter_map(Value::number).collect();
    let numeric = values
        .iter()
        .all(|v| matches!(v, Value::Null | Value::Int(_) | Value::Float(_)));
    let (min, max, histogram) = if numeric && !numbers.is_empty() {
        let lo = numbers.iter().copied().fold(f64::INFINITY, f64::min);
        let hi = numbers.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        (
            Some(Cell::Number(lo)),
            Some(Cell::Number(hi)),
            Some(histogram(&numbers, lo, hi)),
        )
    } else {
        let (min, max) = extremes(values);
        (min, max, None)
    };
    DatasetColumn {
        name: name.to_owned(),
        dtype: dtype.to_owned(),
        null_percent,
        distinct: Some(distinct.len() as u64),
        distinct_capped: capped,
        min,
        max,
        histogram,
    }
}

/// Smallest and largest of non-numeric values: booleans false < true,
/// text by code point (ISO dates sort by time).
fn extremes(values: &[Value]) -> (Option<Cell>, Option<Cell>) {
    let texts: Vec<&str> = values
        .iter()
        .filter_map(|v| match v {
            Value::Text(t) => Some(t.as_str()),
            _ => None,
        })
        .collect();
    if let (Some(lo), Some(hi)) = (texts.iter().min(), texts.iter().max()) {
        return (Some(Cell::Text(shorten(lo))), Some(Cell::Text(shorten(hi))));
    }
    let bools: Vec<bool> = values
        .iter()
        .filter_map(|v| match v {
            Value::Bool(b) => Some(*b),
            _ => None,
        })
        .collect();
    match (bools.iter().min(), bools.iter().max()) {
        (Some(lo), Some(hi)) => (Some(Cell::Bool(*lo)), Some(Cell::Bool(*hi))),
        _ => (None, None),
    }
}

/// Equal-width bins over `lo..=hi`: Sturges' count, at most
/// [`MAX_BINS`]; a single bin when every value is the same.
#[must_use]
pub fn histogram(numbers: &[f64], lo: f64, hi: f64) -> Histogram {
    let n = numbers.len();
    if n == 0 || hi <= lo {
        return Histogram {
            edges: vec![lo, hi.max(lo)],
            counts: vec![n as u64],
        };
    }
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    let bins = (((n as f64).log2().ceil() as usize) + 1).clamp(1, MAX_BINS);
    #[allow(clippy::cast_precision_loss)]
    let width = (hi - lo) / bins as f64;
    #[allow(clippy::cast_precision_loss)]
    let mut edges: Vec<f64> = (0..bins).map(|i| lo + width * i as f64).collect();
    edges.push(hi);
    let mut counts = vec![0_u64; bins];
    for &x in numbers {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let ix = (((x - lo) / width) as usize).min(bins - 1);
        counts[ix] += 1;
    }
    Histogram { edges, counts }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numeric_columns_get_extremes_and_a_histogram() {
        let values = [Value::Int(1), Value::Null, Value::Float(3.5), Value::Int(1)];
        let col = column("x", "float", &values);
        assert!((col.null_percent - 25.0).abs() < 1e-9);
        assert_eq!(col.distinct, Some(2));
        assert_eq!(col.min, Some(Cell::Number(1.0)));
        assert_eq!(col.max, Some(Cell::Number(3.5)));
        let histogram = col.histogram.unwrap_or_else(|| panic!("histogram"));
        assert_eq!(histogram.counts.iter().sum::<u64>(), 3);
        assert_eq!(histogram.validate(), Ok(()));
        assert_eq!(histogram.edges.last(), Some(&3.5));
    }

    #[test]
    fn text_columns_sort_and_distinct_counts_stop_at_the_cap() {
        let col = column(
            "s",
            "text",
            &[Value::Text("pear".into()), Value::Text("apple".into())],
        );
        assert_eq!(col.min, Some(Cell::Text("apple".into())));
        assert_eq!(col.max, Some(Cell::Text("pear".into())));
        assert_eq!(col.histogram, None);
        let many: Vec<Value> = (0..=DISTINCT_CAP as i64).map(Value::Int).collect();
        let col = column("id", "integer", &many);
        assert_eq!(col.distinct, Some(DISTINCT_CAP as u64));
        assert!(col.distinct_capped);
    }

    #[test]
    fn constant_columns_have_one_bin() {
        let h = histogram(&[2.0, 2.0], 2.0, 2.0);
        assert_eq!((h.edges, h.counts), (vec![2.0, 2.0], vec![2]));
    }
}
