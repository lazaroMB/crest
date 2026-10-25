//! [`FakeFormats`]: a [`FormatsPort`] with scripted answers, for tests of
//! the report tools without the real parsers.
//!
//! By default every test report is one suite `fake` with a passed and a
//! failed case (at `src/lib.rs:3`, or the absolute path given with
//! [`FakeFormats::with_location`]), every SARIF log is one warning and
//! every diagram is a tiny SVG, every dataset has an integer `x` and a
//! text `label` column over three rows, and every molecule is ethanol's
//! numbers (`C2H6O`, 46.07 g/mol) unless the SMILES is `bad`. Every trace
//! is `GET /checkout` (api, 100 ms) calling a failed `charge` (payments,
//! 10–90 ms, "card declined") that calls `SELECT cards` (db, 20–40 ms);
//! every profile is `main;work 70` and `main;idle 30` samples; every log
//! is an info line and two `db down` errors of service `db`.
//! [`FakeFormats::failing`] makes every fallible call fail. Calls are
//! recorded as `(method, path, source or SMILES)`.

use std::sync::{Mutex, PoisonError};

use domain::block::{Cell, Column, SourceRef, TableSpec};
use domain::computed::Histogram;
use domain::dataset::{DatasetColumn, DatasetSpec};
use domain::devops::flame::{FlameSpec, FlameTree, ProfileFormat};
use domain::devops::logs::{LogFormat, LogSpec};
use domain::devops::trace::{SERVICE, STATUS_MESSAGE, TraceFormat, TraceSpec, critical_path};
use domain::report::{
    Finding, FindingSeverity, FindingsSpec, TestCase, TestOutcome, TestReportSpec, TestSuite,
};
use domain::widget::list::{ListItem, ListSpec};
use domain::widget::timeline::{Span, SpanStatus, TimeUnit, TimelineRow, TimelineSpec};
use serde_json::Map;

use domain::science::{MoleculeSpec, formulas_match};

use crate::ports::formats::{DatasetRequest, FormatError, FormatsPort, LogRequest, ProfileRequest};

/// The SVG [`FakeFormats::render_wavedrom`] returns.
pub const FAKE_SVG: &str =
    r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"></svg>"#;

/// A scripted formats port.
#[derive(Debug, Default)]
pub struct FakeFormats {
    calls: Mutex<Vec<(String, String)>>,
    error: Option<String>,
    location: Option<String>,
}

impl FakeFormats {
    /// A port answering the defaults.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A port whose every call fails with `message`.
    #[must_use]
    pub fn failing(message: &str) -> Self {
        Self {
            error: Some(message.to_owned()),
            ..Self::default()
        }
    }

    /// Locations point at `path` instead of `src/lib.rs`.
    #[must_use]
    pub fn with_location(mut self, path: &str) -> Self {
        self.location = Some(path.to_owned());
        self
    }

    /// The calls so far, as `(method, path, source or SMILES)`.
    pub fn calls(&self) -> Vec<(String, String)> {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn record(&self, method: &str, arg: &str) -> Result<(), FormatError> {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((method.to_owned(), arg.to_owned()));
        match &self.error {
            Some(message) => Err(FormatError::Invalid(message.clone())),
            None => Ok(()),
        }
    }

    fn location(&self) -> SourceRef {
        SourceRef::File {
            path: self.location.clone().unwrap_or_else(|| "src/lib.rs".into()),
            line_start: Some(3),
            line_end: None,
        }
    }
}

impl FormatsPort for FakeFormats {
    fn parse_test_report(&self, _bytes: &[u8], path: &str) -> Result<TestReportSpec, FormatError> {
        self.record("parse_test_report", path)?;
        let case = |name: &str, outcome, message: Option<&str>| TestCase {
            name: name.to_owned(),
            classname: Some("fake".into()),
            outcome,
            time_secs: Some(0.5),
            message: message.map(str::to_owned),
            details: None,
            location: Some(self.location()),
        };
        Ok(TestReportSpec {
            title: Some(path.to_owned()),
            suites: vec![TestSuite {
                name: "fake".into(),
                time_secs: Some(1.0),
                cases: vec![
                    case("passes", TestOutcome::Passed, None),
                    case("fails", TestOutcome::Failed, Some("assert 1 == 2")),
                ],
            }],
        })
    }

    fn parse_sarif(&self, _bytes: &[u8], path: &str) -> Result<FindingsSpec, FormatError> {
        self.record("parse_sarif", path)?;
        Ok(FindingsSpec {
            tool: Some("fake-lint".into()),
            findings: vec![Finding {
                rule: Some("fake::rule".into()),
                severity: FindingSeverity::Warning,
                message: "something looks off".into(),
                location: Some(self.location()),
                snippet: None,
                help_uri: None,
                fix: None,
                fixes: Vec::new(),
            }],
        })
    }

    fn render_wavedrom(&self, source: &str) -> Result<String, FormatError> {
        self.record("render_wavedrom", source)?;
        Ok(FAKE_SVG.to_owned())
    }

    fn preview_dataset(
        &self,
        _bytes: &[u8],
        request: &DatasetRequest,
    ) -> Result<DatasetSpec, FormatError> {
        self.record("preview_dataset", &request.path)?;
        let column = |name: &str, dtype: &str, null_percent, min, max, histogram| DatasetColumn {
            name: name.to_owned(),
            dtype: dtype.to_owned(),
            null_percent,
            distinct: Some(3),
            distinct_capped: false,
            min,
            max,
            histogram,
        };
        let rows: Vec<Vec<Cell>> = [(1.0, "a"), (2.0, "b"), (3.0, "c")]
            .iter()
            .take(request.preview_rows)
            .map(|(x, label)| vec![Cell::Number(*x), Cell::Text((*label).to_owned())])
            .collect();
        Ok(DatasetSpec {
            path: request.path.clone(),
            format: request.format,
            rows: (!request.truncated).then_some(3),
            sampled: request.truncated,
            columns: vec![
                column(
                    "x",
                    "integer",
                    0.0,
                    Some(Cell::Number(1.0)),
                    Some(Cell::Number(3.0)),
                    Some(Histogram {
                        edges: vec![1.0, 2.0, 3.0],
                        counts: vec![1, 2],
                    }),
                ),
                column("label", "text", 25.0, None, None, None),
            ],
            preview: TableSpec {
                columns: ["x", "label"]
                    .iter()
                    .map(|name| Column {
                        name: (*name).to_owned(),
                        align: None,
                    })
                    .collect(),
                rows,
            },
        })
    }

    fn depict_molecule(&self, smiles: &str, claimed_formula: Option<&str>) -> MoleculeSpec {
        // Recorded only: a molecule never fails as a call.
        let _ = self.record("depict_molecule", smiles);
        let mut spec = MoleculeSpec {
            smiles: smiles.to_owned(),
            name: None,
            svg: None,
            formula: None,
            mw: None,
            charge: None,
            canonical: None,
            error: None,
            claimed_formula: claimed_formula.map(str::to_owned),
            claimed_matches: None,
        };
        if smiles == "bad" {
            spec.error = Some("invalid SMILES: unexpected character at position 0".into());
            return spec;
        }
        spec.svg = Some(FAKE_SVG.to_owned());
        spec.formula = Some("C2H6O".into());
        spec.mw = Some(46.069);
        spec.charge = Some(0);
        spec.canonical = Some("CCO".into());
        spec.claimed_matches = claimed_formula.and_then(|c| formulas_match(c, "C2H6O"));
        spec
    }

    fn parse_trace(&self, _bytes: &[u8], path: &str) -> Result<TraceSpec, FormatError> {
        self.record("parse_trace", path)?;
        let row = |id: &str, depth, (start, end), service: &str, error: Option<&str>| {
            let mut attrs = Map::new();
            attrs.insert(SERVICE.into(), service.into());
            if let Some(message) = error {
                attrs.insert(STATUS_MESSAGE.into(), message.into());
            }
            TimelineRow {
                id: id.to_owned(),
                label: id.to_owned(),
                depth,
                group: None,
                spans: vec![Span {
                    start,
                    end,
                    label: None,
                    color_key: Some(service.to_owned()),
                    status: error.map(|_| SpanStatus::Error),
                    attrs,
                }],
                markers: Vec::new(),
            }
        };
        let timeline = TimelineSpec {
            title: None,
            unit: TimeUnit::Ms,
            rows: vec![
                row("GET /checkout", 0, (0.0, 100.0), "api", None),
                row("charge", 1, (10.0, 90.0), "payments", Some("card declined")),
                row("SELECT cards", 2, (20.0, 40.0), "db", None),
            ],
            range: None,
        };
        Ok(TraceSpec {
            title: None,
            path: Some(path.to_owned()),
            format: TraceFormat::Otlp,
            traces: 1,
            services: vec!["api".into(), "payments".into(), "db".into()],
            started_at: Some(1_792_847_107.0),
            total_spans: 3,
            critical_path: critical_path(&timeline),
            timeline,
        })
    }

    fn parse_profile(
        &self,
        _bytes: &[u8],
        request: &ProfileRequest,
    ) -> Result<FlameSpec, FormatError> {
        self.record("parse_profile", &request.path)?;
        let mut tree = FlameTree::new();
        tree.add(["main", "work"], 70);
        tree.add(["main", "idle"], 30);
        let mut spec = tree.finish(ProfileFormat::Folded, "samples");
        spec.path = Some(request.path.clone());
        Ok(spec)
    }

    fn parse_logs(&self, _bytes: &[u8], request: &LogRequest) -> Result<LogSpec, FormatError> {
        self.record("parse_logs", &request.path)?;
        let item = |level: &str, ts: &str, text: &str| {
            let mut fields = Map::new();
            fields.insert("service".into(), "db".into());
            ListItem {
                level: Some(level.to_owned()),
                ts: Some(ts.to_owned()),
                text: text.to_owned(),
                fields,
            }
        };
        Ok(LogSpec {
            path: Some(request.path.clone()),
            format: request.format.unwrap_or(LogFormat::Json),
            total_lines: 3,
            truncated: request.truncated,
            list: ListSpec {
                title: None,
                columns: vec!["service".into()],
                items: vec![
                    item("info", "2026-10-24T13:05:07Z", "started"),
                    item("error", "2026-10-24T13:05:08Z", "db down after 3 tries"),
                    item("error", "2026-10-24T13:05:09Z", "db down after 4 tries"),
                ],
                facets: vec!["service".into()],
                follow: false,
            },
            histogram: None,
        })
    }
}
