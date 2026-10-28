//! JUnit XML test reports, read with quick-xml.
//!
//! Tolerant of the dialects in the wild: a `<testsuites>` root or a single
//! `<testsuite>` (Maven Surefire), nested suites (flattened, names joined
//! with ` / `), `<testcase>` directly under the root, missing counts and
//! times with thousands separators. A case is failed with `<failure>`,
//! errored with `<error>`, skipped with `<skipped>`, passed otherwise.
//! Failure bodies, `<system-out>` and `<system-err>` become the case's
//! details (kept only for cases that did not pass, and cut to
//! [`MAX_DETAILS`]). `file` and `line` attributes (pytest, gtest) become
//! the location.

use domain::block::SourceRef;
use domain::report::{TestCase, TestOutcome, TestReportSpec, TestSuite};
use quick_xml::events::{BytesRef, BytesStart, Event};
use quick_xml::{Reader, XmlVersion};

/// Longest details text kept per case, in bytes.
pub const MAX_DETAILS: usize = 16 * 1024;

/// What the text being read belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Capture {
    /// Body of `<failure>`, `<error>` or `<skipped>`.
    Result,
    /// `<system-out>` of a case.
    Stdout,
    /// `<system-err>` of a case.
    Stderr,
}

/// The parse in progress.
#[derive(Default)]
struct Parser {
    title: Option<String>,
    /// Every suite, in order of appearance.
    suites: Vec<TestSuite>,
    /// Indexes of the suites being read, outermost first.
    open: Vec<usize>,
    /// Index of the suite made for cases outside any suite.
    loose: Option<usize>,
    case: Option<TestCase>,
    capture: Option<Capture>,
    body: String,
    stdout: String,
    stderr: String,
}

/// Parses `bytes`; `path` names the file in errors and the title.
///
/// # Errors
///
/// Malformed XML or no suite or case at all.
pub fn parse(bytes: &[u8], path: &str) -> Result<TestReportSpec, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| format!("`{path}` is not UTF-8 text"))?;
    let mut reader = Reader::from_str(text);
    let mut parser = Parser::default();
    let mut seen = false;
    loop {
        let event = reader.read_event().map_err(|e| {
            format!(
                "`{path}` is not valid XML (at byte {}): {e}",
                reader.error_position()
            )
        })?;
        match event {
            Event::Start(e) => {
                seen |= parser.start(&e);
            }
            Event::Empty(e) => {
                seen |= parser.start(&e);
                parser.end(e.name().as_ref());
            }
            Event::End(e) => parser.end(e.name().as_ref()),
            Event::Text(t) => {
                if let Ok(text) = t.xml10_content() {
                    parser.text(&text);
                }
            }
            Event::CData(t) => {
                if let Ok(text) = t.decode() {
                    parser.text(&text);
                }
            }
            Event::GeneralRef(r) => {
                if let Some(text) = entity(&r) {
                    parser.text(&text);
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    // A file that ended inside a case.
    parser.finish_case();
    if !seen {
        return Err(format!(
            "`{path}` is not a JUnit report: no <testsuites>, <testsuite> or <testcase>"
        ));
    }
    let mut suites = parser.suites;
    // Suites that only group others carry no cases of their own.
    suites.retain(|suite| !suite.cases.is_empty());
    Ok(TestReportSpec {
        title: parser.title.or_else(|| file_name(path)),
        suites,
    })
}

impl Parser {
    /// An element starts; whether it is one of the report's own.
    fn start(&mut self, e: &BytesStart<'_>) -> bool {
        match e.name().as_ref() {
            b"testsuites" => {
                if self.title.is_none() {
                    self.title = attr(e, b"name").filter(|n| !n.trim().is_empty());
                }
                true
            }
            b"testsuite" => {
                let name = attr(e, b"name").unwrap_or_default();
                let name = match self.open.last() {
                    Some(&parent) => format!("{} / {name}", self.suites[parent].name),
                    None => name,
                };
                self.open.push(self.suites.len());
                self.suites.push(TestSuite {
                    name,
                    time_secs: attr(e, b"time").and_then(|t| seconds(&t)),
                    cases: Vec::new(),
                });
                true
            }
            b"testcase" => {
                self.finish_case();
                let location = attr(e, b"file").map(|path| SourceRef::File {
                    path,
                    line_start: attr(e, b"line")
                        .and_then(|l| l.trim().parse::<u32>().ok())
                        .filter(|&l| l > 0),
                    line_end: None,
                });
                self.case = Some(TestCase {
                    name: attr(e, b"name").unwrap_or_default(),
                    classname: attr(e, b"classname").filter(|c| !c.is_empty()),
                    outcome: TestOutcome::Passed,
                    time_secs: attr(e, b"time").and_then(|t| seconds(&t)),
                    message: None,
                    details: None,
                    location,
                });
                true
            }
            _ => {
                self.capture = self.case_child(e);
                false
            }
        }
    }

    /// Handles `<failure>`, `<error>`, `<skipped>`, `<system-out>` and
    /// `<system-err>` inside a case; returns where its text goes.
    fn case_child(&mut self, e: &BytesStart<'_>) -> Option<Capture> {
        let case = self.case.as_mut()?;
        let outcome = match e.name().as_ref() {
            b"failure" => TestOutcome::Failed,
            b"error" => TestOutcome::Errored,
            b"skipped" => TestOutcome::Skipped,
            b"system-out" => return Some(Capture::Stdout),
            b"system-err" => return Some(Capture::Stderr),
            _ => return None,
        };
        // The first non-passing result wins (later ones are reruns).
        if case.outcome != TestOutcome::Passed {
            return None;
        }
        case.outcome = outcome;
        case.message = attr(e, b"message")
            .filter(|m| !m.trim().is_empty())
            .or_else(|| attr(e, b"type").filter(|t| !t.trim().is_empty()));
        Some(Capture::Result)
    }

    fn end(&mut self, name: &[u8]) {
        match name {
            b"testcase" => self.finish_case(),
            b"testsuite" => {
                self.finish_case();
                self.open.pop();
            }
            _ => self.capture = None,
        }
    }

    fn text(&mut self, text: &str) {
        let buffer = match self.capture {
            Some(Capture::Result) => &mut self.body,
            Some(Capture::Stdout) => &mut self.stdout,
            Some(Capture::Stderr) => &mut self.stderr,
            None => return,
        };
        if buffer.len() < MAX_DETAILS * 2 {
            buffer.push_str(text);
        }
    }

    /// The suite a case goes to: the innermost open one, or one made for
    /// cases outside any suite.
    fn current_suite(&mut self) -> usize {
        if let Some(&ix) = self.open.last() {
            return ix;
        }
        *self.loose.get_or_insert_with(|| {
            self.suites.push(TestSuite {
                name: "tests".to_owned(),
                time_secs: None,
                cases: Vec::new(),
            });
            self.suites.len() - 1
        })
    }

    fn finish_case(&mut self) {
        let Some(mut case) = self.case.take() else {
            return;
        };
        self.capture = None;
        let body = std::mem::take(&mut self.body);
        let stdout = std::mem::take(&mut self.stdout);
        let stderr = std::mem::take(&mut self.stderr);
        if case.outcome != TestOutcome::Passed {
            let body = body.trim();
            if case.message.is_none() {
                case.message = body.lines().next().map(str::to_owned);
            }
            let mut details = String::new();
            for (label, text) in [
                (None, body),
                (Some("--- stdout ---"), &stdout),
                (Some("--- stderr ---"), &stderr),
            ] {
                let text = text.trim_end();
                if text.trim().is_empty() {
                    continue;
                }
                if !details.is_empty() {
                    details.push_str("\n\n");
                }
                if let Some(label) = label {
                    details.push_str(label);
                    details.push('\n');
                }
                details.push_str(text);
            }
            if !details.is_empty() {
                case.details = Some(cut(details));
            }
        }
        let suite = self.current_suite();
        self.suites[suite].cases.push(case);
    }
}

/// `text` cut to [`MAX_DETAILS`] bytes on a character boundary.
fn cut(mut text: String) -> String {
    if text.len() > MAX_DETAILS {
        let mut end = MAX_DETAILS;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push_str("\n… (cut)");
    }
    text
}

/// The unescaped value of attribute `name`.
fn attr(e: &BytesStart<'_>, name: &[u8]) -> Option<String> {
    let attribute = e.try_get_attribute(name).ok().flatten()?;
    attribute
        .normalized_value(XmlVersion::Implicit1_0)
        .ok()
        .map(std::borrow::Cow::into_owned)
}

/// The text of an entity or character reference (`&lt;`, `&#10;`).
fn entity(r: &BytesRef<'_>) -> Option<String> {
    if let Ok(Some(c)) = r.resolve_char_ref() {
        return Some(c.to_string());
    }
    let name = r.decode().ok()?;
    quick_xml::escape::resolve_predefined_entity(&name).map(str::to_owned)
}

/// Seconds from a JUnit `time` (`1.5`, `1,234.5`).
fn seconds(text: &str) -> Option<f64> {
    let value: f64 = text.trim().replace(',', "").parse().ok()?;
    (value.is_finite() && value >= 0.0).then_some(value)
}

fn file_name(path: &str) -> Option<String> {
    path.rsplit(['/', '\\'])
        .next()
        .filter(|n| !n.is_empty())
        .map(str::to_owned)
}
