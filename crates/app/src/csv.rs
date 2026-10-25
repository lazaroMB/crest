//! A small RFC 4180 reader: CSV or TSV text into records, and into the
//! named columns the compute engine reads ([`data_set`]).
//!
//! Fields are split on the delimiter, may be quoted with `"` (a doubled
//! `""` is a quote inside), and quoted fields may span lines. Records end
//! at `\n` or `\r\n`; blank lines are skipped. It is pure string work, so
//! the statistics tools and the Lua `app.ui.csv` share it.

use domain::computed::InputRef;

use crate::ports::compute::{ColumnValues, DataColumn, DataSet};

/// Splits `text` into records of fields.
///
/// # Errors
///
/// A quote that is never closed, with the line it opened on.
pub fn records(text: &str, delimiter: char) -> Result<Vec<Vec<String>>, String> {
    let mut records = Vec::new();
    let mut record = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    let mut line = 1;
    let mut quote_line = 0;
    while let Some(c) = chars.next() {
        if quoted {
            match c {
                '"' if chars.peek() == Some(&'"') => {
                    chars.next();
                    field.push('"');
                }
                '"' => quoted = false,
                '\n' => {
                    line += 1;
                    field.push(c);
                }
                _ => field.push(c),
            }
            continue;
        }
        match c {
            '"' if field.is_empty() => {
                quoted = true;
                quote_line = line;
            }
            '\r' if chars.peek() == Some(&'\n') => {}
            '\n' => {
                line += 1;
                record.push(std::mem::take(&mut field));
                push_record(&mut records, std::mem::take(&mut record));
            }
            c if c == delimiter => record.push(std::mem::take(&mut field)),
            _ => field.push(c),
        }
    }
    if quoted {
        return Err(format!("unclosed quote opened on line {quote_line}"));
    }
    if !field.is_empty() || !record.is_empty() {
        record.push(field);
        push_record(&mut records, record);
    }
    Ok(records)
}

/// Keeps `record` unless it is a blank line.
fn push_record(records: &mut Vec<Vec<String>>, record: Vec<String>) {
    let blank = record.len() == 1 && record[0].trim().is_empty();
    if !blank {
        records.push(record);
    }
}

/// `text` as a finite number, when it is written as one (`inf`, `NaN` and
/// words that Rust happens to parse stay text).
#[must_use]
pub fn number(text: &str) -> Option<f64> {
    let trimmed = text.trim();
    let numeric = !trimmed.is_empty()
        && trimmed
            .chars()
            .all(|c| c.is_ascii_digit() || matches!(c, '+' | '-' | '.' | 'e' | 'E'));
    trimmed
        .parse::<f64>()
        .ok()
        .filter(|n| numeric && n.is_finite())
}

/// Cells that mean "missing": empty, `NA` (R), `NaN`, `null`, `None`.
#[must_use]
pub fn is_missing(text: &str) -> bool {
    matches!(
        text.trim(),
        "" | "NA" | "N/A" | "NaN" | "nan" | "null" | "NULL" | "None"
    )
}

/// The delimiter a file name implies: tab for `.tsv` and `.tab`, else a
/// comma.
#[must_use]
pub fn delimiter_for(path: &str) -> char {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".tsv") || lower.ends_with(".tab") {
        '\t'
    } else {
        ','
    }
}

/// Reads `text` (first record = column names) as named columns: a column
/// whose present cells are all numbers is numeric, any other is
/// categorical; missing cells are `None`. Short rows are padded with
/// missing cells. `source` says where the text came from.
///
/// # Errors
///
/// An unclosed quote, no header, an empty or repeated column name.
pub fn data_set(text: &str, delimiter: char, source: InputRef) -> Result<DataSet, String> {
    let mut records = records(text, delimiter)?.into_iter();
    let header = records
        .next()
        .ok_or_else(|| "the file is empty; it needs a header row".to_owned())?;
    let names: Vec<String> = header.iter().map(|n| n.trim().to_owned()).collect();
    for (ix, name) in names.iter().enumerate() {
        if name.is_empty() {
            return Err(format!("column {} has no name in the header row", ix + 1));
        }
        if names[..ix].contains(name) {
            return Err(format!("column `{name}` appears twice in the header row"));
        }
    }
    let rows: Vec<Vec<String>> = records.collect();
    let columns = names
        .into_iter()
        .enumerate()
        .map(|(ix, name)| {
            let cells: Vec<Option<&str>> = rows
                .iter()
                .map(|row| row.get(ix).map(String::as_str).filter(|c| !is_missing(c)))
                .collect();
            let numbers: Option<Vec<Option<f64>>> = cells
                .iter()
                .map(|c| match c {
                    None => Some(None),
                    Some(text) => number(text).map(Some),
                })
                .collect();
            let values = match numbers {
                Some(numbers) => ColumnValues::Numeric(numbers),
                None => ColumnValues::Categorical(
                    cells
                        .into_iter()
                        .map(|c| c.map(|t| t.trim().to_owned()))
                        .collect(),
                ),
            };
            DataColumn {
                name,
                values,
                unit: None,
            }
        })
        .collect();
    Ok(DataSet { source, columns })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::compute::inline_source;

    #[test]
    fn quotes_escapes_and_crlf() {
        let text = "a,b\r\n\"x, y\",\"say \"\"hi\"\"\"\r\n\"multi\nline\",2.5\r\n";
        assert_eq!(
            records(text, ',').unwrap(),
            vec![
                vec!["a".to_owned(), "b".into()],
                vec!["x, y".into(), "say \"hi\"".into()],
                vec!["multi\nline".into(), "2.5".into()],
            ]
        );
        let err = records("a\n\"open\n", ',').unwrap_err();
        assert!(err.contains("line 2"), "{err}");
    }

    #[test]
    fn columns_are_numeric_or_categorical_with_missing_cells() {
        let text = "mpg\tam\tnote\n21\tmanual\tok\nNA\tauto\n22.8\t\t1\n";
        let data = data_set(text, '\t', inline_source()).unwrap();
        assert_eq!(
            data.column("mpg").map(|c| &c.values),
            Some(&ColumnValues::Numeric(vec![Some(21.0), None, Some(22.8)]))
        );
        assert_eq!(
            data.column("am").map(|c| &c.values),
            Some(&ColumnValues::Categorical(vec![
                Some("manual".into()),
                Some("auto".into()),
                None
            ]))
        );
        assert_eq!(
            data.column("note").map(|c| &c.values),
            Some(&ColumnValues::Categorical(vec![
                Some("ok".into()),
                None,
                Some("1".into())
            ]))
        );
    }

    #[test]
    fn bad_headers_are_errors() {
        assert!(data_set("", ',', inline_source()).is_err());
        assert!(
            data_set("a,a\n1,2\n", ',', inline_source())
                .unwrap_err()
                .contains("twice")
        );
        assert!(
            data_set("a,\n1,2\n", ',', inline_source())
                .unwrap_err()
                .contains("column 2")
        );
        assert_eq!(delimiter_for("data/x.TSV"), '\t');
        assert_eq!(delimiter_for("x.csv"), ',');
    }
}
