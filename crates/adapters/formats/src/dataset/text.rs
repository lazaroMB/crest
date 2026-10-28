//! Text datasets: CSV and TSV (through `application::csv`) and JSON lines.
//!
//! CSV cells are typed per column by inference over the profiled rows: a
//! column is `integer` when every present cell is a whole number, `float`
//! when every one is a number, `bool` for `true`/`false`, `date` or
//! `datetime` for ISO dates (`2024-03-01`, `2024-03-01T12:00`), `text`
//! otherwise. Missing cells (empty, `NA`, `null`, …) are nulls. JSON lines
//! keep their JSON types; their strings are tested for dates the same way.

use application::csv;
use serde_json::Value as Json;

use super::profile::Value;
use super::{MAX_PROFILE_ROWS, Table};

/// The part of `bytes` that holds whole lines: everything when the file
/// is complete, up to the last newline when it was cut.
fn whole_lines(bytes: &[u8], truncated: bool) -> String {
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    let end = if truncated {
        bytes.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1)
    } else {
        bytes.len()
    };
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

/// Header names made usable: blanks become `column_<n>`, repeats get a
/// `_<k>` suffix.
fn names(header: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(header.len());
    for (ix, raw) in header.iter().enumerate() {
        let base = match raw.trim() {
            "" => format!("column_{}", ix + 1),
            name => name.to_owned(),
        };
        let mut name = base.clone();
        let mut k = 2;
        while out.contains(&name) {
            name = format!("{base}_{k}");
            k += 1;
        }
        out.push(name);
    }
    out
}

/// What a CSV column holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Integer,
    Float,
    Bool,
    Date,
    DateTime,
    Text,
    Empty,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Self::Integer => "integer",
            Self::Float => "float",
            Self::Bool => "bool",
            Self::Date => "date",
            Self::DateTime => "datetime",
            Self::Text => "text",
            Self::Empty => "empty",
        }
    }

    /// `cell` (present, trimmed) as a value of this kind.
    fn value(self, cell: &str) -> Value {
        match self {
            Self::Integer => cell.parse().map_or_else(|_| text(cell), Value::Int),
            Self::Float => csv::number(cell).map_or_else(|| text(cell), Value::Float),
            Self::Bool => boolean(cell).map_or_else(|| text(cell), Value::Bool),
            Self::Date | Self::DateTime | Self::Text | Self::Empty => text(cell),
        }
    }
}

fn text(cell: &str) -> Value {
    Value::Text(cell.to_owned())
}

fn boolean(cell: &str) -> Option<bool> {
    if cell.eq_ignore_ascii_case("true") {
        Some(true)
    } else if cell.eq_ignore_ascii_case("false") {
        Some(false)
    } else {
        None
    }
}

/// Whether `text` is an ISO date (`YYYY-MM-DD` or `YYYY/MM/DD`) and, if
/// so, whether a time follows it (`T` or a space, then `HH:MM`).
#[must_use]
pub fn date_kind(text: &str) -> Option<bool> {
    let b = text.as_bytes();
    let digits =
        |r: std::ops::Range<usize>| b.get(r).is_some_and(|s| s.iter().all(u8::is_ascii_digit));
    let sep = b.get(4).copied();
    if !(digits(0..4) && digits(5..7) && digits(8..10))
        || !matches!(sep, Some(b'-' | b'/'))
        || b.get(7).copied() != sep
    {
        return None;
    }
    let month = (b[5] - b'0') * 10 + (b[6] - b'0');
    let day = (b[8] - b'0') * 10 + (b[9] - b'0');
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    match b.get(10) {
        None => Some(false),
        Some(b'T' | b' ') if digits(11..13) && b.get(13) == Some(&b':') && digits(14..16) => {
            Some(true)
        }
        Some(_) => None,
    }
}

/// The kind every present cell of a column fits.
fn infer<'a>(cells: impl Iterator<Item = &'a str> + Clone) -> Kind {
    let mut present = cells
        .filter(|c| !csv::is_missing(c))
        .map(str::trim)
        .peekable();
    if present.peek().is_none() {
        return Kind::Empty;
    }
    let all = |f: &dyn Fn(&str) -> bool| present.clone().all(f);
    if all(&|c| c.parse::<i64>().is_ok()) {
        Kind::Integer
    } else if all(&|c| csv::number(c).is_some()) {
        Kind::Float
    } else if all(&|c| boolean(c).is_some()) {
        Kind::Bool
    } else if all(&|c| date_kind(c).is_some()) {
        if present.clone().any(|c| date_kind(c) == Some(true)) {
            Kind::DateTime
        } else {
            Kind::Date
        }
    } else {
        Kind::Text
    }
}

/// Reads CSV or TSV text: header, typed columns of the first
/// [`MAX_PROFILE_ROWS`] records and the record count.
///
/// # Errors
///
/// An unclosed quote, or no header row.
pub fn delimited(bytes: &[u8], delimiter: char, truncated: bool) -> Result<Table, String> {
    let text = whole_lines(bytes, truncated);
    let mut records = csv::records(&text, delimiter)?.into_iter();
    let header = records
        .next()
        .ok_or_else(|| "the file is empty; it needs a header row".to_owned())?;
    let names = names(&header);
    let rows: Vec<Vec<String>> = records.collect();
    let total = rows.len();
    let profiled = &rows[..total.min(MAX_PROFILE_ROWS)];
    let mut dtypes = Vec::with_capacity(names.len());
    let mut columns = Vec::with_capacity(names.len());
    for ix in 0..names.len() {
        let cells = profiled
            .iter()
            .map(move |row| row.get(ix).map_or("", String::as_str));
        let kind = infer(cells.clone());
        dtypes.push(kind.name().to_owned());
        columns.push(
            cells
                .map(|c| {
                    if csv::is_missing(c) {
                        Value::Null
                    } else {
                        kind.value(c.trim())
                    }
                })
                .collect(),
        );
    }
    Ok(Table {
        names,
        dtypes,
        columns,
        total_rows: (!truncated).then_some(total as u64),
        sampled: truncated || total > profiled.len(),
    })
}

/// The type name of a JSON lines column from the kinds of its values.
fn json_dtype(values: &[&Json]) -> &'static str {
    let present: Vec<&&Json> = values.iter().filter(|v| !v.is_null()).collect();
    if present.is_empty() {
        return "empty";
    }
    let all = |f: fn(&Json) -> bool| present.iter().all(|v| f(v));
    if all(|v| v.as_i64().is_some()) {
        "integer"
    } else if all(Json::is_number) {
        "float"
    } else if all(Json::is_boolean) {
        "bool"
    } else if all(Json::is_string) {
        let dates: Vec<Option<bool>> = present
            .iter()
            .map(|v| date_kind(v.as_str().unwrap_or_default()))
            .collect();
        if dates.iter().all(Option::is_some) {
            if dates.contains(&Some(true)) {
                "datetime"
            } else {
                "date"
            }
        } else {
            "text"
        }
    } else if all(|v| v.is_object() || v.is_array()) {
        "json"
    } else {
        "mixed"
    }
}

fn json_value(value: &Json) -> Value {
    match value {
        Json::Null => Value::Null,
        Json::Bool(b) => Value::Bool(*b),
        Json::Number(n) => n
            .as_i64()
            .map_or_else(|| Value::Float(n.as_f64().unwrap_or(f64::NAN)), Value::Int),
        Json::String(s) => Value::Text(s.clone()),
        other => Value::Text(other.to_string()),
    }
}

/// Reads JSON lines: one object per line; columns are the keys in the
/// order first seen.
///
/// # Errors
///
/// A line that is not a JSON object, with its number.
pub fn json_lines(bytes: &[u8], truncated: bool) -> Result<Table, String> {
    let text = whole_lines(bytes, truncated);
    let mut objects = Vec::new();
    let mut total = 0_usize;
    for (ix, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        total += 1;
        if objects.len() >= MAX_PROFILE_ROWS {
            continue;
        }
        match serde_json::from_str::<Json>(line) {
            Ok(Json::Object(map)) => objects.push(map),
            Ok(_) => return Err(format!("line {} is not a JSON object", ix + 1)),
            Err(e) => return Err(format!("line {} is not JSON: {e}", ix + 1)),
        }
    }
    let mut names: Vec<String> = Vec::new();
    for object in &objects {
        for key in object.keys() {
            if !names.contains(key) {
                names.push(key.clone());
            }
        }
    }
    if names.is_empty() {
        return Err("the file has no JSON objects with fields".into());
    }
    let mut dtypes = Vec::with_capacity(names.len());
    let mut columns = Vec::with_capacity(names.len());
    for name in &names {
        let values: Vec<&Json> = objects
            .iter()
            .map(|o| o.get(name).unwrap_or(&Json::Null))
            .collect();
        dtypes.push(json_dtype(&values).to_owned());
        columns.push(values.into_iter().map(json_value).collect());
    }
    Ok(Table {
        names,
        dtypes,
        columns,
        total_rows: (!truncated).then_some(total as u64),
        sampled: truncated || total > objects.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_are_recognised() {
        assert_eq!(date_kind("2024-03-01"), Some(false));
        assert_eq!(date_kind("2024/03/01"), Some(false));
        assert_eq!(date_kind("2024-03-01T12:30:00Z"), Some(true));
        assert_eq!(date_kind("2024-13-01"), None);
        assert_eq!(date_kind("2024-03/01"), None);
        assert_eq!(date_kind("20240301"), None);
    }

    #[test]
    fn header_names_are_made_unique() {
        let header = ["a".to_owned(), String::new(), "a".to_owned()];
        assert_eq!(names(&header), ["a", "column_2", "a_2"]);
    }

    #[test]
    fn a_cut_file_keeps_whole_lines() {
        let table = delimited(b"a,b\n1,2\n3,", ',', true).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(table.columns[0], [Value::Int(1)]);
        assert_eq!(table.total_rows, None);
        assert!(table.sampled);
    }
}
