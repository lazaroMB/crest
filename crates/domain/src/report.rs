//! Reports from developer tools: test runs (JUnit XML, `show_test_report`)
//! and static-analysis findings (SARIF, `show_findings`).
//!
//! Adapters parse the files; these types are what the blocks draw.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::block::{BlockError, SourceRef};

/// A test run: suites of cases.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TestReportSpec {
    /// Title, e.g. the report file name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Suites, in report order.
    pub suites: Vec<TestSuite>,
}

/// A test suite.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TestSuite {
    /// Suite name.
    pub name: String,
    /// Wall time of the suite, in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_secs: Option<f64>,
    /// Cases, in report order.
    pub cases: Vec<TestCase>,
}

/// One test case.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TestCase {
    /// Case name.
    pub name: String,
    /// Class or module, when the report has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub classname: Option<String>,
    /// Outcome.
    pub outcome: TestOutcome,
    /// Wall time, in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_secs: Option<f64>,
    /// Failure or skip message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Stack trace or captured output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
    /// Where the test or the failure is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<SourceRef>,
}

/// Outcome of a [`TestCase`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TestOutcome {
    /// Passed.
    Passed,
    /// An assertion failed.
    Failed,
    /// The test could not run to completion (panic, setup error).
    Errored,
    /// Not run.
    Skipped,
}

/// Counts of a report by outcome.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TestTotals {
    /// Passed cases.
    pub passed: usize,
    /// Failed cases.
    pub failed: usize,
    /// Errored cases.
    pub errored: usize,
    /// Skipped cases.
    pub skipped: usize,
}

impl TestTotals {
    /// All cases.
    #[must_use]
    pub fn total(self) -> usize {
        self.passed + self.failed + self.errored + self.skipped
    }
}

impl TestReportSpec {
    /// Counts the cases by outcome.
    #[must_use]
    pub fn totals(&self) -> TestTotals {
        let mut totals = TestTotals::default();
        for case in self.suites.iter().flat_map(|s| &s.cases) {
            match case.outcome {
                TestOutcome::Passed => totals.passed += 1,
                TestOutcome::Failed => totals.failed += 1,
                TestOutcome::Errored => totals.errored += 1,
                TestOutcome::Skipped => totals.skipped += 1,
            }
        }
        totals
    }

    /// Checks the case locations.
    ///
    /// # Errors
    ///
    /// The first [`BlockError`] of a location.
    pub fn validate(&self) -> Result<(), BlockError> {
        self.suites
            .iter()
            .flat_map(|s| &s.cases)
            .filter_map(|c| c.location.as_ref())
            .try_for_each(SourceRef::validate)
    }
}

/// Static-analysis findings, SARIF-like.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct FindingsSpec {
    /// Tool that produced them, e.g. `clippy 0.1.95`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// The findings, in report order.
    pub findings: Vec<Finding>,
}

/// One finding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Finding {
    /// Rule id, e.g. `clippy::unwrap_used`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule: Option<String>,
    /// Severity (SARIF `level`).
    pub severity: FindingSeverity,
    /// What is wrong.
    pub message: String,
    /// Where; usually a [`SourceRef::File`] with lines.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<SourceRef>,
    /// Source excerpt of the location.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
    /// Link to the rule's documentation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub help_uri: Option<String>,
    /// Suggested fix as a unified diff, made from [`Finding::fixes`]
    /// against the files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
    /// Suggested fixes as the log describes them (SARIF `fixes`): edits
    /// of byte or text ranges. Alternatives; the first is the preferred.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fixes: Vec<FindingFix>,
}

/// A suggested fix: edits of one or more files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FindingFix {
    /// What it does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The edits, in log order.
    pub edits: Vec<FixEdit>,
}

/// Replaces a range of a file with `inserted` (SARIF `replacement`). The
/// range is given by lines and columns (1-based, the end column
/// exclusive; columns count characters), by character offset and length,
/// or by byte offset and length.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FixEdit {
    /// The file, as the log names it (made workspace-relative by the tool).
    pub path: String,
    /// First line of the range.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_line: Option<u32>,
    /// First column; 1 when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_column: Option<u32>,
    /// Last line; `start_line` when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_line: Option<u32>,
    /// Column after the range; the end of `end_line` when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_column: Option<u32>,
    /// Characters before the range, when given by offset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub char_offset: Option<u64>,
    /// Characters in the range (0 when absent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub char_length: Option<u64>,
    /// Bytes before the range, when given by byte offset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub byte_offset: Option<u64>,
    /// Bytes in the range (0 when absent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub byte_length: Option<u64>,
    /// The new text.
    #[serde(default)]
    pub inserted: String,
}

impl FixEdit {
    /// The byte range of `text` this edit replaces.
    ///
    /// # Errors
    ///
    /// A message when the range is missing, outside the text or not on
    /// character boundaries.
    pub fn byte_range(&self, text: &str) -> Result<std::ops::Range<usize>, String> {
        let to_usize = |v: u64| usize::try_from(v).map_err(|_| "offset too large".to_owned());
        if let Some(start) = self.byte_offset {
            let start = to_usize(start)?;
            let end = start + to_usize(self.byte_length.unwrap_or(0))?;
            if end > text.len() || !text.is_char_boundary(start) || !text.is_char_boundary(end) {
                return Err(format!("bytes {start}..{end} are outside the file"));
            }
            return Ok(start..end);
        }
        if let Some(start) = self.char_offset {
            let start = to_usize(start)?;
            let end = start + to_usize(self.char_length.unwrap_or(0))?;
            let at = |n: usize| {
                text.char_indices()
                    .map(|(i, _)| i)
                    .chain(std::iter::once(text.len()))
                    .nth(n)
                    .ok_or_else(|| format!("characters {start}..{end} are outside the file"))
            };
            return Ok(at(start)?..at(end)?);
        }
        let start_line = self
            .start_line
            .filter(|&l| l > 0)
            .ok_or_else(|| "the edit has no range".to_owned())?;
        let end_line = self.end_line.unwrap_or(start_line).max(start_line);
        // Byte offset of each line start, without its newline at the end.
        let lines: Vec<(usize, &str)> = text
            .split_inclusive('\n')
            .scan(0, |offset, line| {
                let start = *offset;
                *offset += line.len();
                Some((start, line.trim_end_matches(['\n', '\r'])))
            })
            .collect();
        let line = |n: u32| -> Result<(usize, &str), String> {
            let ix = usize::try_from(n - 1).map_err(|e| e.to_string())?;
            match lines.get(ix) {
                Some(&line) => Ok(line),
                // Just past the end: an insertion at the end of the file.
                None if ix == lines.len() => Ok((text.len(), "")),
                None => Err(format!("line {n} is outside the file")),
            }
        };
        let column = |(start, content): (usize, &str), column: u32| -> Result<usize, String> {
            let n = usize::try_from(column.max(1) - 1).map_err(|e| e.to_string())?;
            content
                .char_indices()
                .map(|(i, _)| i)
                .chain(std::iter::once(content.len()))
                .nth(n)
                .map(|i| start + i)
                .ok_or_else(|| format!("column {column} is outside its line"))
        };
        let first = line(start_line)?;
        let last = line(end_line)?;
        let start = column(first, self.start_column.unwrap_or(1))?;
        let end = match self.end_column {
            Some(c) => column(last, c)?,
            None => last.0 + last.1.len(),
        };
        if end < start {
            return Err("the edit ends before it starts".into());
        }
        Ok(start..end)
    }

    /// The edit in words, for when the file cannot be read:
    /// `src/a.rs:3:5-3:9: replace with "x"`.
    #[must_use]
    pub fn describe(&self) -> String {
        let at = match (self.start_line, self.byte_offset, self.char_offset) {
            (Some(line), _, _) => {
                let mut at = format!("{}:{line}", self.path);
                if let Some(c) = self.start_column {
                    at.push_str(&format!(":{c}"));
                }
                match (self.end_line, self.end_column) {
                    (Some(l), Some(c)) => at.push_str(&format!("-{l}:{c}")),
                    (None, Some(c)) => at.push_str(&format!("-{c}")),
                    (Some(l), None) if l != line => at.push_str(&format!("-{l}")),
                    _ => {}
                }
                at
            }
            (None, Some(offset), _) => format!(
                "{} bytes {offset}..{}",
                self.path,
                offset + self.byte_length.unwrap_or(0)
            ),
            (None, None, Some(offset)) => format!(
                "{} characters {offset}..{}",
                self.path,
                offset + self.char_length.unwrap_or(0)
            ),
            (None, None, None) => self.path.clone(),
        };
        let empty = self.byte_length.or(self.char_length) == Some(0)
            || (self.start_line.is_some()
                && self.end_line.is_none_or(|l| Some(l) == self.start_line)
                && self.start_column.is_some()
                && self.start_column == self.end_column);
        match (empty, self.inserted.is_empty()) {
            (true, _) => format!("{at}: insert {:?}", self.inserted),
            (false, true) => format!("{at}: delete"),
            (false, false) => format!("{at}: replace with {:?}", self.inserted),
        }
    }
}

/// `text` with `edits` applied (all of one file).
///
/// # Errors
///
/// A message when an edit's range is bad or two edits overlap.
pub fn apply_edits<'a>(
    text: &str,
    edits: impl IntoIterator<Item = &'a FixEdit>,
) -> Result<String, String> {
    let mut ranges = edits
        .into_iter()
        .map(|e| e.byte_range(text).map(|r| (r, e.inserted.as_str())))
        .collect::<Result<Vec<_>, _>>()?;
    ranges.sort_by_key(|(r, _)| (r.start, r.end));
    if ranges.windows(2).any(|w| w[0].0.end > w[1].0.start) {
        return Err("the edits overlap".into());
    }
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for (range, inserted) in ranges {
        out.push_str(&text[at..range.start]);
        out.push_str(inserted);
        at = range.end;
    }
    out.push_str(&text[at..]);
    Ok(out)
}

/// Severity of a [`Finding`], the SARIF levels.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum FindingSeverity {
    /// A problem.
    Error,
    /// A possible problem.
    Warning,
    /// Information.
    Note,
    /// No severity given.
    None,
}

impl FindingsSpec {
    /// Checks the finding locations.
    ///
    /// # Errors
    ///
    /// The first [`BlockError`] of a location.
    pub fn validate(&self) -> Result<(), BlockError> {
        self.findings
            .iter()
            .filter_map(|f| f.location.as_ref())
            .try_for_each(SourceRef::validate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_report_totals_and_roundtrip() {
        let spec: TestReportSpec = serde_json::from_value(json!({
            "suites": [{"name": "unit", "time_secs": 0.5, "cases": [
                {"name": "a", "outcome": "passed"},
                {"name": "b", "outcome": "failed", "message": "assert", "time_secs": 0.1,
                 "location": {"type": "file", "path": "src/a.rs", "line_start": 3}},
                {"name": "c", "outcome": "skipped"},
                {"name": "d", "outcome": "errored"}
            ]}]
        }))
        .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(spec.validate(), Ok(()));
        let totals = spec.totals();
        assert_eq!((totals.passed, totals.failed, totals.total()), (1, 1, 4));
        let back: TestReportSpec =
            serde_json::from_value(serde_json::to_value(&spec).unwrap()).unwrap();
        assert_eq!(back, spec);
    }

    fn edit(json: serde_json::Value) -> FixEdit {
        let mut value = json;
        value["path"] = json!("a.rs");
        serde_json::from_value(value).unwrap_or_else(|e| panic!("{e}"))
    }

    #[test]
    fn fix_edits_apply_by_lines_columns_and_offsets() {
        let text = "fn main() {\n    let x = f().unwrap();\n}\n";
        // Replace `.unwrap()` (columns 16..25 of line 2) with `?`.
        let by_column =
            edit(json!({"start_line": 2, "start_column": 16, "end_column": 25, "inserted": "?"}));
        assert_eq!(
            apply_edits(text, [&by_column]).unwrap(),
            "fn main() {\n    let x = f()?;\n}\n"
        );
        // A whole line's content, then an insertion at a column.
        let line = edit(json!({"start_line": 3, "inserted": "} // end"}));
        let insert =
            edit(json!({"start_line": 1, "start_column": 3, "end_column": 3, "inserted": " my"}));
        assert_eq!(
            apply_edits(text, [&line, &insert]).unwrap(),
            "fn my main() {\n    let x = f().unwrap();\n} // end\n"
        );
        let by_char = edit(json!({"char_offset": 3, "char_length": 4, "inserted": "start"}));
        assert!(
            apply_edits(text, [&by_char])
                .unwrap()
                .starts_with("fn start() {")
        );
        let by_byte = edit(json!({"byte_offset": 0, "byte_length": 2, "inserted": "pub fn"}));
        assert!(
            apply_edits(text, [&by_byte])
                .unwrap()
                .starts_with("pub fn main")
        );
        // Out of range, overlapping or without a range.
        assert!(apply_edits(text, [&edit(json!({"start_line": 9}))]).is_err());
        assert!(apply_edits(text, [&by_char, &by_byte]).is_ok());
        let overlap = edit(json!({"char_offset": 4, "char_length": 2}));
        assert!(apply_edits(text, [&by_char, &overlap]).is_err());
        assert!(apply_edits(text, [&edit(json!({}))]).is_err());
        assert_eq!(by_column.describe(), r#"a.rs:2:16-25: replace with "?""#);
        assert_eq!(insert.describe(), r#"a.rs:1:3-3: insert " my""#);
        assert_eq!(
            edit(json!({"byte_offset": 4, "byte_length": 2})).describe(),
            "a.rs bytes 4..6: delete"
        );
    }

    #[test]
    fn findings_validate_locations() {
        let mut spec: FindingsSpec = serde_json::from_value(json!({
            "tool": "clippy",
            "findings": [{"rule": "clippy::unwrap_used", "severity": "warning",
                          "message": "used unwrap",
                          "location": {"type": "file", "path": "src/a.rs", "line_start": 4, "line_end": 4}}]
        }))
        .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(spec.validate(), Ok(()));
        assert!(FindingSeverity::Error < FindingSeverity::Note);
        spec.findings[0].location = Some(SourceRef::File {
            path: "a".into(),
            line_start: Some(0),
            line_end: None,
        });
        assert!(spec.validate().is_err());
    }
}
