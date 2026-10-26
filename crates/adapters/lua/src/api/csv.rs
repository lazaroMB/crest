//! The CSV reader behind `app.ui.csv`: RFC 4180 records (split by
//! [`application::csv`]) into a [`TableSpec`].
//!
//! Fields are split on the delimiter, may be quoted with `"` (a doubled
//! `""` is a quote inside), and quoted fields may span lines. Records end
//! at `\n` or `\r\n`; blank lines are skipped. Cells that read as a finite
//! number become numbers (so the table sorts them as such), empty cells
//! become nulls and the rest stay text. Short rows are padded with nulls
//! to the widest row.

use application::csv::{self, records};
use domain::block::{Cell, Column, TableSpec};

/// How to read the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CsvOptions {
    /// Field separator.
    pub(crate) delimiter: char,
    /// Whether the first record names the columns.
    pub(crate) header: bool,
}

impl Default for CsvOptions {
    fn default() -> Self {
        Self {
            delimiter: ',',
            header: true,
        }
    }
}

/// A cell from its text: a finite number, null when empty, else text.
fn cell(text: String) -> Cell {
    if text.trim().is_empty() {
        return Cell::Null;
    }
    match csv::number(&text) {
        Some(n) => Cell::Number(n),
        None => Cell::Text(text),
    }
}

/// Reads `text` as CSV.
///
/// # Errors
///
/// A quote that is never closed.
pub(crate) fn parse(text: &str, options: CsvOptions) -> Result<TableSpec, String> {
    let mut records = records(text, options.delimiter)?.into_iter();
    let header = if options.header {
        records.next().unwrap_or_default()
    } else {
        Vec::new()
    };
    let rows: Vec<Vec<String>> = records.collect();
    let width = rows
        .iter()
        .map(Vec::len)
        .chain(std::iter::once(header.len()))
        .max()
        .unwrap_or_default()
        .max(1);
    let columns = (0..width)
        .map(|ix| match header.get(ix).map(|name| name.trim()) {
            Some(name) if !name.is_empty() => Column::new(name),
            _ => Column::new(format!("column {}", ix + 1)),
        })
        .collect();
    let rows = rows
        .into_iter()
        .map(|row| {
            let mut cells: Vec<Cell> = row.into_iter().map(cell).collect();
            cells.resize(width, Cell::Null);
            cells
        })
        .collect();
    Ok(TableSpec { columns, rows })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(spec: &TableSpec) -> Vec<&str> {
        spec.columns.iter().map(|c| c.name.as_str()).collect()
    }

    #[test]
    fn reads_a_header_numbers_text_and_nulls() {
        let spec = parse(
            "file,lines,owner\nsrc/a.rs,120,ana\nsrc/b.rs,,\n",
            CsvOptions::default(),
        )
        .unwrap();
        assert_eq!(names(&spec), ["file", "lines", "owner"]);
        assert_eq!(
            spec.rows,
            vec![
                vec![
                    Cell::Text("src/a.rs".into()),
                    Cell::Number(120.0),
                    Cell::Text("ana".into())
                ],
                vec![Cell::Text("src/b.rs".into()), Cell::Null, Cell::Null],
            ]
        );
        assert_eq!(spec.validate(), Ok(()));
    }

    #[test]
    fn quotes_escapes_and_crlf() {
        let text = "a,b\r\n\"x, y\",\"say \"\"hi\"\"\"\r\n\"multi\nline\",2.5\r\n";
        let spec = parse(text, CsvOptions::default()).unwrap();
        assert_eq!(
            spec.rows,
            vec![
                vec![Cell::Text("x, y".into()), Cell::Text("say \"hi\"".into())],
                vec![Cell::Text("multi\nline".into()), Cell::Number(2.5)],
            ]
        );
    }

    #[test]
    fn without_header_columns_are_numbered_and_rows_padded() {
        let options = CsvOptions {
            delimiter: '\t',
            header: false,
        };
        let spec = parse("1\t2\t3\n\n4\n", options).unwrap();
        assert_eq!(names(&spec), ["column 1", "column 2", "column 3"]);
        assert_eq!(
            spec.rows[1],
            vec![Cell::Number(4.0), Cell::Null, Cell::Null]
        );
        assert_eq!(spec.validate(), Ok(()));
    }

    #[test]
    fn words_that_parse_as_floats_stay_text() {
        let spec = parse("x\ninf\nNaN\n1e3\n-2\n", CsvOptions::default()).unwrap();
        let cells: Vec<_> = spec.rows.into_iter().map(|mut r| r.remove(0)).collect();
        assert_eq!(
            cells,
            [
                Cell::Text("inf".into()),
                Cell::Text("NaN".into()),
                Cell::Number(1000.0),
                Cell::Number(-2.0)
            ]
        );
    }

    #[test]
    fn empty_text_is_one_empty_column_and_unclosed_quotes_fail() {
        let spec = parse("", CsvOptions::default()).unwrap();
        assert_eq!(names(&spec), ["column 1"]);
        assert!(spec.rows.is_empty());
        let err = parse("a\n\"open\n", CsvOptions::default()).unwrap_err();
        assert!(err.contains("line 2"), "{err}");
    }
}
