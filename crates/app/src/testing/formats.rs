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
//! is an info line and two `db down` errors of service `db`; every query
//! plan is a `Hash Join` (100 ms, 5000 rows for 50 planned) over a hot
//! `Seq Scan on orders` (70 ms) and a `Hash`; every HAR is a `GET /` of
//! `app.test` (200, 120 ms, 2 kB, with an `Authorization` header) and a
//! failed `POST /api/pay?token=...` (503, 300 ms); every Terraform plan
//! replaces `aws_instance.web` (forced by `ami`), updates a sensitive
//! `password` of `aws_db_instance.main`, creates `aws_s3_bucket.logs`
//! and leaves 4 resources unchanged; every DOT graph is `build -> test ->
//! deploy` plus `lint -> test` with `build` and `test` in cluster `ci`;
//! every DDL is `users` and `orders` (`orders.user_id` → `users.id`);
//! benchmarks are `fib/20` regressed (+14%) and `sort/1k` unchanged, plus
//! one per directory of the request beyond those two, without baseline;
//! every waveform is `tb.clk` (period 10 ns) and an 8-bit `tb.count`
//! counting 00, 01, 02 on its rising edges, over 0–30 ns; every netlist
//! is a divider `V1` (in, 0), `R1` (in, out), `R2` (out, 0) plus `R3`
//! (out, nc) left open; every STEP part is a 10 mm tetrahedron `Body`
//! (4 faces, one not meshed) in product `Block`; every DXF drawing is a
//! 100 × 50 mm rectangle on layer `Outline` with a hole on `Holes` (off)
//! and a `PLATE` note; every IFC schedule is project `House` with a
//! `Ground floor` holding `101` Kitchen (12.5 m² from
//! Qto_SpaceBaseQuantities) and `102` Store (no area), 4 walls, 2 doors
//! and 3 windows; every sequence is the circular 30 bp `pFake` with a
//! CDS `orf` (1..9) and one EcoRI site (cut after 10); every reaction is
//! `H2 + O2 → H2O` (O unbalanced, 2 H2 + O2 → 2 H2O suggested) unless its
//! SMILES is `bad`; every molecule grid gives each entry ethanol's
//! numbers (MW 46.07, logP -0.0014, TPSA 20.23) unless its SMILES is
//! `bad`; every spectrum is an IR transmittance trace 4000–400 cm⁻¹ with
//! one dip picked at 1715 cm⁻¹; every 3D molecule is water (O and two H
//! 0.9572 Å from it) unless its SMILES is `bad`.
//! [`FakeFormats::failing`] makes every fallible call fail. Calls are
//! recorded as `(method, path, source or SMILES)`.

use std::sync::{Mutex, PoisonError};

use domain::block::{Cell, Column, SourceRef, TableSpec};
use domain::computed::Histogram;
use domain::dataset::{DatasetColumn, DatasetSpec};
use domain::devops::bench::{Bench, BenchSpec, Estimate, NOISE_THRESHOLD, RunEstimates, Verdict};
use domain::devops::diagram::{DiagramSource, DiagramSpec, ErColumn, ErTable};
use domain::devops::flame::{FlameSpec, FlameTree, ProfileFormat};
use domain::devops::har::{HarEntry, HarHeader, HarSpec, HarTimings};
use domain::devops::logs::{LogFormat, LogSpec};
use domain::devops::query_plan::{PlanEngine, PlanNode, QueryPlanSpec};
use domain::devops::terraform::{
    AttrChange, AttrValue, PlanAction, PlanSource, ResourceChange, TerraformPlanSpec,
};
use domain::devops::trace::{SERVICE, STATUS_MESSAGE, TraceFormat, TraceSpec, critical_path};
use domain::engineering::cad::{CadModelSpec, CadSolid};
use domain::engineering::drawing::{DrawEntity, DrawingLayer, DrawingSpec, PolyVertex, Shape};
use domain::engineering::ifc::{ElementCounts, IfcScheduleSpec, IfcSpace, IfcStorey};
use domain::engineering::netlist::{Component, NetlistFormat, NetlistSpec, PinNet};
use domain::engineering::waveform::{
    SignalKind, Timescale, WaveChange, WaveFormat, WaveSignal, WaveformSpec,
};
use domain::model3d::{ModelMesh, UpAxis};
use domain::report::{
    Finding, FindingSeverity, FindingsSpec, TestCase, TestOutcome, TestReportSpec, TestSuite,
};
use domain::widget::graph::GraphSpec;
use domain::widget::list::{ListItem, ListSpec};
use domain::widget::timeline::{Span, SpanStatus, TimeUnit, TimelineRow, TimelineSpec};
use serde_json::Map;

use domain::computed::{Computed, ComputedStatus, ComputedValue, Engine};
use domain::science::molecule3d::{
    Atom3d, Bond3d, CoordinateSource, Molecule3dSpec, StructureFormat,
};
use domain::science::molecule_grid::{Descriptors, GridMolecule, MoleculeGridSpec};
use domain::science::spectrum::{Peak, PeakSense, SpectrumFormat, SpectrumKind, SpectrumSpec};
use domain::science::reaction::{
    ElementBalance, ReactionComponent, ReactionRole, ReactionSpec, Stoichiometry,
};
use domain::science::sequence::{
    Alphabet, CutSite, SeqFeature, SeqSpan, SequenceFormat, SequenceSpec, Strand, Topology,
};
use domain::science::{MoleculeSpec, formulas_match};

use crate::ports::formats::{
    BenchRequest, DatasetRequest, FormatError, FormatsPort, LogRequest, ProfileRequest,
    Molecule3dSource, MoleculeGridRequest, SequenceRequest, SpectrumRequest, StepRequest, WaveRequest,
};

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

    fn parse_query_plan(
        &self,
        _bytes: &[u8],
        path: Option<&str>,
    ) -> Result<QueryPlanSpec, FormatError> {
        self.record("parse_query_plan", path.unwrap_or("-"))?;
        let node = |node_type: &str, depth, time, rows, planned| PlanNode {
            node_type: node_type.to_owned(),
            depth,
            actual_time: Some(time),
            actual_rows: Some(rows),
            plan_rows: Some(planned),
            loops: Some(1.0),
            ..PlanNode::default()
        };
        let mut scan = node("Seq Scan", 1, 70.0, 10_000.0, 9_000.0);
        scan.relation = Some("orders".into());
        let mut spec = QueryPlanSpec {
            title: None,
            path: path.map(str::to_owned),
            engine: PlanEngine::Postgres,
            analyzed: true,
            buffers: false,
            planning_time: Some(0.5),
            execution_time: Some(100.0),
            trigger_time: None,
            jit_time: None,
            nodes: vec![
                node("Hash Join", 0, 100.0, 5_000.0, 50.0),
                scan,
                node("Hash", 1, 20.0, 10.0, 10.0),
            ],
        };
        spec.analyse();
        Ok(spec)
    }

    fn parse_har(&self, _bytes: &[u8], path: &str) -> Result<HarSpec, FormatError> {
        self.record("parse_har", path)?;
        let entry = |method: &str, url: &str, status, start, wait: f64| HarEntry {
            method: method.to_owned(),
            url: url.to_owned(),
            status,
            resource_type: "fetch".into(),
            start,
            time: 20.0 + wait,
            timings: HarTimings {
                connect: Some(10.0),
                send: Some(10.0),
                wait: Some(wait),
                ..HarTimings::default()
            },
            transfer_size: Some(2_000),
            ..HarEntry::default()
        };
        let mut home = entry("GET", "https://app.test/", 200, 0.0, 100.0);
        home.resource_type = "document".into();
        home.request_headers = vec![HarHeader {
            name: "Authorization".into(),
            value: "Bearer s3cret".into(),
        }];
        Ok(HarSpec {
            title: None,
            path: Some(path.to_owned()),
            creator: Some("WebInspector 537.36".into()),
            started_at: Some(1_792_847_107.0),
            pages: Vec::new(),
            entries: vec![
                home,
                entry(
                    "POST",
                    "https://app.test/api/pay?token=abc",
                    503,
                    150.0,
                    280.0,
                ),
            ],
            total_entries: 2,
        })
    }

    fn parse_terraform_plan(
        &self,
        _bytes: &[u8],
        path: &str,
    ) -> Result<TerraformPlanSpec, FormatError> {
        self.record("parse_terraform_plan", path)?;
        let known = |v: &str| AttrValue::Known(v.into());
        let change = |path: &str, before, after, forces| AttrChange {
            path: path.to_owned(),
            before,
            after,
            forces_replacement: forces,
        };
        let resource = |address: &str, action, attributes: Vec<AttrChange>| ResourceChange {
            address: address.to_owned(),
            kind: address.split('.').next().map(str::to_owned),
            data: false,
            provider: None,
            action,
            reason: None,
            previous_address: None,
            attributes_total: attributes.len(),
            attributes,
        };
        Ok(TerraformPlanSpec {
            title: None,
            path: Some(path.to_owned()),
            source: PlanSource::Show,
            terraform_version: Some("1.9.0".into()),
            resources: vec![
                resource(
                    "aws_instance.web",
                    PlanAction::Replace,
                    vec![change("ami", known("ami-1"), known("ami-2"), true)],
                ),
                resource(
                    "aws_db_instance.main",
                    PlanAction::Update,
                    vec![change(
                        "password",
                        AttrValue::Sensitive,
                        AttrValue::Sensitive,
                        false,
                    )],
                ),
                resource(
                    "aws_s3_bucket.logs",
                    PlanAction::Create,
                    vec![change("bucket", AttrValue::Absent, known("logs"), false)],
                ),
            ],
            unchanged: 4,
            drift: Vec::new(),
            outputs: Vec::new(),
            errored: false,
            applyable: Some(true),
        })
    }

    fn parse_dot(&self, _text: &str, path: Option<&str>) -> Result<DiagramSpec, FormatError> {
        self.record("parse_dot", path.unwrap_or("-"))?;
        let graph: GraphSpec = serde_json::from_value(serde_json::json!({
            "title": "pipeline",
            "nodes": [
                {"id": "build", "group": "ci"}, {"id": "test", "group": "ci"},
                {"id": "deploy"}, {"id": "lint"}
            ],
            "edges": [
                {"from": "build", "to": "test"}, {"from": "test", "to": "deploy"},
                {"from": "lint", "to": "test"}
            ],
            "groups": [{"id": "ci", "label": "CI"}]
        }))
        .map_err(|e| FormatError::Invalid(e.to_string()))?;
        Ok(DiagramSpec {
            title: None,
            path: path.map(str::to_owned),
            source: DiagramSource::Dot,
            dialect: None,
            graph,
            tables: Vec::new(),
            warnings: Vec::new(),
        })
    }

    fn parse_ddl(
        &self,
        _sql: &str,
        dialect: Option<&str>,
        path: Option<&str>,
    ) -> Result<DiagramSpec, FormatError> {
        self.record("parse_ddl", path.unwrap_or("-"))?;
        let col = |name: &str, pk, references: Option<&str>| ErColumn {
            name: name.to_owned(),
            data_type: "bigint".into(),
            primary_key: pk,
            nullable: !pk,
            unique: false,
            references: references.map(str::to_owned),
        };
        let graph: GraphSpec = serde_json::from_value(serde_json::json!({
            "nodes": [{"id": "users", "kind": "table"}, {"id": "orders", "kind": "table"}],
            "edges": [{"from": "orders", "to": "users", "label": "user_id → id"}]
        }))
        .map_err(|e| FormatError::Invalid(e.to_string()))?;
        Ok(DiagramSpec {
            title: None,
            path: path.map(str::to_owned),
            source: DiagramSource::SqlDdl,
            dialect: Some(dialect.unwrap_or("generic").to_owned()),
            graph,
            tables: vec![
                ErTable {
                    name: "users".into(),
                    columns: vec![col("id", true, None)],
                },
                ErTable {
                    name: "orders".into(),
                    columns: vec![
                        col("id", true, None),
                        col("user_id", false, Some("users.id")),
                    ],
                },
            ],
            warnings: Vec::new(),
        })
    }

    fn parse_benchmarks(&self, request: &BenchRequest) -> Result<BenchSpec, FormatError> {
        self.record("parse_benchmarks", &request.path)?;
        let est = |point: f64, lo: f64, hi: f64| Estimate {
            point,
            lo,
            hi,
            confidence: 0.95,
            standard_error: None,
        };
        let run = |mean: Estimate| RunEstimates {
            mean,
            median: None,
            std_dev: None,
            slope: None,
        };
        let bench = |id: &str, new, base: Option<Estimate>, change: Option<Estimate>| Bench {
            id: id.to_owned(),
            dir: id.to_owned(),
            new: run(new),
            base: base.map(run),
            verdict: change.map_or(Verdict::Unknown, |c| Verdict::of(&c, NOISE_THRESHOLD)),
            change,
            median_change: None,
            ratio: None,
        };
        let mut benches = vec![
            bench(
                "fib/20",
                est(25_100.0, 24_900.0, 25_300.0),
                Some(est(22_000.0, 21_800.0, 22_200.0)),
                Some(est(0.142, 0.129, 0.156)),
            ),
            bench(
                "sort/1k",
                est(1_000_000.0, 990_000.0, 1_010_000.0),
                Some(est(1_000_000.0, 990_000.0, 1_010_000.0)),
                Some(est(0.002, -0.01, 0.014)),
            ),
        ];
        for files in request.benches.iter().skip(2) {
            benches.push(bench(&files.dir, est(10.0, 9.0, 11.0), None, None));
        }
        Ok(BenchSpec {
            title: None,
            path: request.path.clone(),
            baseline: request.baseline.clone(),
            noise_threshold: NOISE_THRESHOLD,
            benches,
            skipped: Vec::new(),
        })
    }

    fn parse_waveform(
        &self,
        _bytes: &[u8],
        request: &WaveRequest,
    ) -> Result<WaveformSpec, FormatError> {
        self.record("parse_waveform", &request.path)?;
        let changes = |list: &[(u64, &str)]| {
            list.iter()
                .map(|(t, v)| WaveChange {
                    t: *t,
                    v: (*v).to_owned(),
                })
                .collect()
        };
        let signal = |name: &str, kind, width, list: &[(u64, &str)]| WaveSignal {
            name: name.to_owned(),
            scope: "tb".to_owned(),
            kind,
            width,
            changes: changes(list),
            truncated: false,
        };
        Ok(WaveformSpec {
            title: None,
            path: request.path.clone(),
            format: WaveFormat::Vcd,
            timescale: Timescale::default(),
            start: 0,
            end: 30,
            signals: vec![
                signal(
                    "clk",
                    SignalKind::Bit,
                    1,
                    &[
                        (0, "0"),
                        (5, "1"),
                        (10, "0"),
                        (15, "1"),
                        (20, "0"),
                        (25, "1"),
                    ],
                ),
                signal(
                    "count[7:0]",
                    SignalKind::Vector,
                    8,
                    &[(0, "00000000"), (15, "00000001"), (25, "00000010")],
                ),
            ],
            total_signals: 2,
            radix: request.radix,
            generator: None,
        })
    }

    fn parse_netlist(
        &self,
        _text: &str,
        format: Option<NetlistFormat>,
        path: Option<&str>,
    ) -> Result<NetlistSpec, FormatError> {
        self.record("parse_netlist", path.unwrap_or(""))?;
        let part = |reference: &str, kind: &str, value: &str, nets: [&str; 2]| Component {
            reference: reference.to_owned(),
            kind: kind.to_owned(),
            value: Some(value.to_owned()),
            model: None,
            pins: nets
                .iter()
                .zip(["1", "2"])
                .map(|(net, pin)| PinNet {
                    pin: pin.to_owned(),
                    net: (*net).to_owned(),
                    function: None,
                })
                .collect(),
        };
        Ok(NetlistSpec {
            title: None,
            path: path.map(str::to_owned),
            format: format.unwrap_or_default(),
            design: Some("divider".to_owned()),
            components: vec![
                part("V1", "voltage source", "DC 5", ["in", "0"]),
                part("R1", "resistor", "10k", ["in", "out"]),
                part("R2", "resistor", "10k", ["out", "0"]),
                part("R3", "resistor", "1k", ["out", "nc"]),
            ],
            subcircuits: Vec::new(),
            warnings: Vec::new(),
        })
    }

    fn parse_step(
        &self,
        _bytes: &[u8],
        request: &StepRequest,
    ) -> Result<CadModelSpec, FormatError> {
        self.record("parse_step", &request.path)?;
        Ok(CadModelSpec {
            title: None,
            path: request.path.clone(),
            schema: Some("AUTOMOTIVE_DESIGN".to_owned()),
            application: Some("fake CAD".to_owned()),
            unit: Some("mm".to_owned()),
            products: vec!["Block".to_owned()],
            solids: vec![CadSolid {
                name: "Body".to_owned(),
                product: Some("Block".to_owned()),
                faces: 4,
                failed_faces: 1,
                first_triangle: 0,
                triangles: 3,
                min: [0.0; 3],
                max: [10.0; 3],
            }],
            mesh: ModelMesh {
                positions: vec![
                    [0.0, 0.0, 0.0],
                    [10.0, 0.0, 0.0],
                    [0.0, 10.0, 0.0],
                    [0.0, 0.0, 10.0],
                ],
                triangles: vec![[0, 2, 1], [0, 1, 3], [0, 3, 2]],
                up: UpAxis::Z,
            },
            tolerance: request.tolerance.unwrap_or(0.01),
            warnings: Vec::new(),
        })
    }

    fn parse_dxf(&self, _bytes: &[u8], path: &str) -> Result<DrawingSpec, FormatError> {
        self.record("parse_dxf", path)?;
        let layer = |name: &str, color: u8, off: bool, entities: usize| DrawingLayer {
            name: name.to_owned(),
            color,
            off,
            entities,
        };
        let corner = |x: f64, y: f64| PolyVertex { x, y, bulge: 0.0 };
        Ok(DrawingSpec {
            title: None,
            path: path.to_owned(),
            version: Some("R2000".to_owned()),
            units: Some("mm".to_owned()),
            header_extents: None,
            layers: vec![layer("Outline", 5, false, 2), layer("Holes", 1, true, 1)],
            entities: vec![
                DrawEntity {
                    layer: 0,
                    color: None,
                    shape: Shape::Polyline {
                        vertices: vec![
                            corner(0.0, 0.0),
                            corner(100.0, 0.0),
                            corner(100.0, 50.0),
                            corner(0.0, 50.0),
                        ],
                        closed: true,
                    },
                },
                DrawEntity {
                    layer: 0,
                    color: Some(3),
                    shape: Shape::Text {
                        at: [5.0, 5.0],
                        height: 2.5,
                        rotation: 0.0,
                        text: "PLATE".to_owned(),
                    },
                },
                DrawEntity {
                    layer: 1,
                    color: None,
                    shape: Shape::Circle {
                        center: [50.0, 25.0],
                        radius: 3.0,
                    },
                },
            ],
            inserts: 0,
            skipped: Vec::new(),
            truncated: false,
            warnings: Vec::new(),
        })
    }

    fn parse_ifc_schedule(
        &self,
        _bytes: &[u8],
        path: &str,
    ) -> Result<IfcScheduleSpec, FormatError> {
        self.record("parse_ifc_schedule", path)?;
        let space = |name: &str, long: &str, area: Option<f64>| IfcSpace {
            name: Some(name.to_owned()),
            long_name: Some(long.to_owned()),
            area,
            area_source: area.map(|_| "Qto_SpaceBaseQuantities.NetFloorArea".to_owned()),
            global_id: None,
        };
        Ok(IfcScheduleSpec {
            title: None,
            path: path.to_owned(),
            schema: "IFC4".to_owned(),
            project: Some("House".to_owned()),
            site: None,
            buildings: Vec::new(),
            length_unit: Some("m".to_owned()),
            area_unit: Some("m²".to_owned()),
            storeys: vec![IfcStorey {
                name: "Ground floor".to_owned(),
                elevation: Some(0.0),
                building: None,
                spaces: vec![
                    space("101", "Kitchen", Some(12.5)),
                    space("102", "Store", None),
                ],
                elements: ElementCounts {
                    walls: 4,
                    doors: 2,
                    windows: 3,
                },
            }],
            unplaced_spaces: Vec::new(),
            unplaced: ElementCounts::default(),
            warnings: Vec::new(),
        })
    }

    fn parse_sequence(
        &self,
        _bytes: &[u8],
        request: &SequenceRequest,
    ) -> Result<SequenceSpec, FormatError> {
        self.record(
            "parse_sequence",
            request.path.as_deref().unwrap_or("<inline>"),
        )?;
        Ok(SequenceSpec {
            title: None,
            path: request.path.clone(),
            format: SequenceFormat::Genbank,
            name: Some("pFake".into()),
            description: Some("a fake plasmid".into()),
            accession: None,
            organism: None,
            alphabet: Alphabet::Dna,
            topology: request.topology.unwrap_or(Topology::Circular),
            sequence: "ATGAAATAAGAATTCCCCGGGTTTTAAAAC".into(),
            features: vec![SeqFeature {
                kind: "CDS".into(),
                label: Some("orf".into()),
                segments: vec![SeqSpan { start: 0, end: 9 }],
                strand: Strand::Forward,
                qualifiers: Vec::new(),
            }],
            cut_sites: if request.enzymes {
                vec![CutSite {
                    enzyme: "EcoRI".into(),
                    site: "GAATTC".into(),
                    start: 9,
                    cut: 10,
                    strand: Strand::Forward,
                }]
            } else {
                Vec::new()
            },
            enzyme_source: request.enzymes.then(|| "REBASE".to_owned()),
            records: 1,
            record: 0,
            warnings: Vec::new(),
        })
    }

    fn depict_reaction(&self, smiles: &str, _coefficients: Option<&[u32]>) -> ReactionSpec {
        let _ = self.record("depict_reaction", smiles);
        let mut balance = Computed::new(
            "ch_fake",
            "chem.reaction_balance",
            "Atom and charge balance",
            Engine {
                name: "crest-chem".into(),
                version: "0.0.0".into(),
                backend: None,
                platform: None,
            },
        );
        let mut spec = ReactionSpec {
            title: None,
            smiles: smiles.to_owned(),
            conditions: None,
            components: Vec::new(),
            elements: Vec::new(),
            charge: (0, 0),
            stoichiometry: Stoichiometry::NotChecked,
            balance: balance.clone(),
            error: None,
        };
        if smiles == "bad" {
            spec.error = Some("invalid reaction SMILES: bad".into());
            spec.balance.status = ComputedStatus::Error;
            return spec;
        }
        let component = |role, formula: &str, atoms: &[(&str, u32)]| ReactionComponent {
            role,
            molecule: MoleculeSpec {
                smiles: formula.to_owned(),
                name: None,
                svg: Some(FAKE_SVG.to_owned()),
                formula: Some(formula.to_owned()),
                mw: Some(1.0),
                charge: Some(0),
                canonical: Some(formula.to_owned()),
                error: None,
                claimed_formula: None,
                claimed_matches: None,
            },
            coefficient: 1,
            atoms: atoms.iter().map(|(e, n)| ((*e).to_owned(), *n)).collect(),
        };
        spec.components = vec![
            component(ReactionRole::Reactant, "H2", &[("H", 2)]),
            component(ReactionRole::Reactant, "O2", &[("O", 2)]),
            component(ReactionRole::Product, "H2O", &[("H", 2), ("O", 1)]),
        ];
        spec.elements = vec![
            ElementBalance {
                element: "H".into(),
                reactants: 2,
                products: 2,
            },
            ElementBalance {
                element: "O".into(),
                reactants: 2,
                products: 1,
            },
        ];
        spec.stoichiometry = Stoichiometry::Suggested {
            coefficients: vec![2, 1, 2],
        };
        balance.status = ComputedStatus::Warning;
        let mut flag = ComputedValue::new(0.0);
        flag.display = Some("no".into());
        balance.values.insert("balanced".into(), flag);
        balance.values.insert("delta.O".into(), ComputedValue::new(-1.0));
        spec.balance = balance;
        spec
    }

    fn molecule_grid(
        &self,
        request: &MoleculeGridRequest,
    ) -> Result<MoleculeGridSpec, FormatError> {
        self.record("molecule_grid", &request.molecules.len().to_string())?;
        if request.molecules.is_empty() {
            return Err(FormatError::Invalid("no molecules given".into()));
        }
        let mut envelope = Computed::new(
            "ch_grid",
            "chem.descriptors",
            "Molecular descriptors",
            Engine {
                name: "crest-chem".into(),
                version: "0.0.0".into(),
                backend: None,
                platform: None,
            },
        );
        let molecules = request
            .molecules
            .iter()
            .map(|e| {
                let molecule = self.depict_molecule(&e.smiles, None);
                let descriptors = molecule.error.is_none().then(|| Descriptors {
                    mw: 46.069,
                    heavy_atoms: 3,
                    hbd: 1,
                    hba: 1,
                    rotatable_bonds: 0,
                    rings: 0,
                    aromatic_rings: 0,
                    logp: -0.0014,
                    tpsa: 20.23,
                });
                if descriptors.is_some() {
                    envelope
                        .values
                        .insert(format!("{}.mw", e.id), ComputedValue::new(46.069));
                }
                GridMolecule {
                    id: e.id.clone(),
                    molecule: MoleculeSpec {
                        name: e.name.clone(),
                        ..molecule
                    },
                    activity: e.activity,
                    descriptors,
                }
            })
            .collect();
        Ok(MoleculeGridSpec {
            title: None,
            activity_label: request.activity_label.clone(),
            activity_unit: request.activity_unit.clone(),
            direction: request.direction,
            molecules,
            descriptors: envelope,
        })
    }

    fn parse_spectrum(
        &self,
        _bytes: &[u8],
        request: &SpectrumRequest,
    ) -> Result<SpectrumSpec, FormatError> {
        self.record("parse_spectrum", &request.path)?;
        let mut picking = Computed::new(
            "ch_peaks",
            "chem.peak_picking",
            "Peak picking by prominence",
            Engine {
                name: "crest-chem".into(),
                version: "0.0.0".into(),
                backend: None,
                platform: None,
            },
        );
        picking
            .values
            .insert("peak.1.x".into(), ComputedValue::new(1715.0));
        Ok(SpectrumSpec {
            title: None,
            path: request.path.clone(),
            format: SpectrumFormat::Jcamp,
            kind: SpectrumKind::Infrared,
            name: Some("acetone".into()),
            x_label: "wavenumber (cm⁻¹)".into(),
            y_label: "transmittance".into(),
            x_reversed: true,
            x: vec![4000.0, 3000.0, 1715.0, 1000.0, 400.0],
            y: vec![0.95, 0.9, 0.2, 0.9, 0.95],
            sticks: false,
            peaks: vec![Peak {
                x: 1715.0,
                y: 0.2,
                prominence: Some(0.7),
            }],
            sense: PeakSense::Minima,
            picking,
            metadata: vec![("origin".into(), "crest".into())],
            blocks: 1,
            block: 0,
            warnings: Vec::new(),
        })
    }

    fn molecule_3d(&self, source: &Molecule3dSource) -> Result<Molecule3dSpec, FormatError> {
        let (arg, smiles, path, coordinates) = match source {
            Molecule3dSource::Smiles(s) => (
                s.clone(),
                Some(s.clone()),
                None,
                CoordinateSource::Embedded {
                    method: "fake embedding".into(),
                },
            ),
            Molecule3dSource::File { path, .. } => (
                path.clone(),
                None,
                Some(path.clone()),
                CoordinateSource::File {
                    format: StructureFormat::Xyz,
                },
            ),
        };
        self.record("molecule_3d", &arg)?;
        if arg == "bad" {
            return Err(FormatError::Invalid("invalid SMILES: bad".into()));
        }
        let atom = |element: &str, position: [f64; 3]| Atom3d {
            element: element.to_owned(),
            position,
            name: None,
            residue: None,
        };
        Ok(Molecule3dSpec {
            title: None,
            name: Some("water".into()),
            smiles,
            path,
            source: coordinates,
            atoms: vec![
                atom("O", [0.0, 0.0, 0.0]),
                atom("H", [0.9572, 0.0, 0.0]),
                atom("H", [-0.24, 0.9266, 0.0]),
            ],
            bonds: vec![
                Bond3d { a: 0, b: 1, order: 1 },
                Bond3d { a: 0, b: 2, order: 1 },
            ],
            bonds_inferred: false,
            warnings: Vec::new(),
        })
    }
}
