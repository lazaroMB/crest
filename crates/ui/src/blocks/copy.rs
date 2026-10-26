//! Copy-as: the text forms a block can be copied in, and the row of
//! buttons the selected block shows for them.
//!
//! | block | forms |
//! | --- | --- |
//! | table | TSV, CSV |
//! | chart | CSV, TSV (its data table, [`ChartSpec::to_table`]) |
//! | plot | the `PlotSpec` JSON |
//! | computed card | APA line, reproduce lines, the card's tables as CSV (distribution bins, contingency cells, power curve, A/B arms, bootstrap intervals, forecast, LP solution, equation plug-back, Monte Carlo tornado, cash-flow periods, decision matrix, frequency response, Karnaugh SOP / POS / truth table, beam diagrams, tolerance chain, costed BOM), envelope JSON (in the card's own footer) |
//! | mermaid | source |
//! | test report | failing test names |
//! | findings | `file:line: rule message` lines |
//! | shell | the output without ANSI escapes |
//! | dataset | column profile CSV, preview rows CSV |
//! | molecule | SMILES, canonical SMILES, formula, SVG |
//! | plate | `well,label,value` CSV |
//! | waveform | value changes CSV, every signal's value at the end |
//! | netlist | `net,component,pin` CSV, components CSV, the graph as DOT |
//! | material | properties CSV with the source of each value (or `unverified`) |
//! | cad model (STEP) | solids CSV (faces, triangles, box), the summary line |
//! | drawing (DXF) | layers CSV, the drawing's texts, the summary line |
//! | IFC schedule | spaces CSV (storey, name, area, unit, source), the tree as text |
//! | sequence | FASTA, features CSV, cut sites CSV (when searched) |
//! | reaction | reaction SMILES, the equation (and the balanced one), balance CSV |
//! | molecule grid | CSV of ids, SMILES, activities and descriptors; SMILES list |
//! | spectrum | points CSV, peaks CSV |
//! | custom | its widgets: timeline → spans CSV, filter list → all items as lines (its own copy button takes the visible ones), graph → DOT, tree → indented text |
//!
//! Inline math chips copy their LaTeX themselves ([`super::math`]). `y` on
//! a selected block still copies its default text.

use domain::block::{BlockKind, Cell, ChartSpec, CustomNode, SourceRef, TableSpec};
use domain::computed::{Computed, ComputedBody, narrative};
use domain::devops::flame::FlameSpec;
use domain::devops::trace::TraceSpec;
use domain::report::{FindingsSpec, TestOutcome, TestReportSpec};
use domain::science::MoleculeSpec;
use domain::widget::timeline::{TimeUnit, format_duration};
use gpui::{
    AnyElement, App, ClipboardItem, ElementId, InteractiveElement as _, IntoElement as _,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, div,
};
use gpui_component::{ActiveTheme as _, h_flex};

use crate::timeline::TimelineEntry;

/// One way to copy a block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyFormat {
    /// Stable key, for element ids (`tsv`, `csv`, `json`, `lua`…).
    pub key: &'static str,
    /// Button label (`TSV`, `Lua`…).
    pub label: &'static str,
    /// What goes to the clipboard.
    pub text: String,
}

impl CopyFormat {
    fn new(key: &'static str, label: &'static str, text: String) -> Self {
        Self { key, label, text }
    }
}

/// A cell as text: numbers in their shortest form, `null` empty.
#[must_use]
pub fn cell_text(cell: &Cell) -> String {
    match cell {
        Cell::Null => String::new(),
        Cell::Bool(b) => b.to_string(),
        Cell::Number(n) => n.to_string(),
        Cell::Text(text) => text.clone(),
    }
}

/// A CSV field (RFC 4180): quoted when it holds a comma, a quote or a
/// line break, with quotes doubled.
#[must_use]
pub fn csv_field(text: &str) -> String {
    if text.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text.to_owned()
    }
}

/// A TSV field: tabs and line breaks, which TSV cannot escape, become
/// spaces.
#[must_use]
pub fn tsv_field(text: &str) -> String {
    text.replace("\r\n", " ").replace(['\t', '\n', '\r'], " ")
}

fn delimited(table: &TableSpec, separator: &str, field: fn(&str) -> String) -> String {
    let header = table
        .columns
        .iter()
        .map(|c| field(&c.name))
        .collect::<Vec<_>>()
        .join(separator);
    let rows = table.rows.iter().map(|row| {
        row.iter()
            .map(|cell| field(&cell_text(cell)))
            .collect::<Vec<_>>()
            .join(separator)
    });
    std::iter::once(header)
        .chain(rows)
        .map(|line| line + "\n")
        .collect()
}

/// The table as CSV, header first, one line per row.
#[must_use]
pub fn csv(table: &TableSpec) -> String {
    delimited(table, ",", csv_field)
}

/// The table as TSV, header first, one line per row.
#[must_use]
pub fn tsv(table: &TableSpec) -> String {
    delimited(table, "\t", tsv_field)
}

/// The chart's data table: labels down the first column, one column per
/// series.
#[must_use]
pub fn chart_table(chart: &ChartSpec) -> TableSpec {
    chart.to_table()
}

/// The failing (failed or errored) tests, one per line as
/// `class::name`.
#[must_use]
pub fn failing_tests(spec: &TestReportSpec) -> String {
    spec.suites
        .iter()
        .flat_map(|s| &s.cases)
        .filter(|c| matches!(c.outcome, TestOutcome::Failed | TestOutcome::Errored))
        .map(|c| match &c.classname {
            Some(class) => format!("{class}::{}\n", c.name),
            None => format!("{}\n", c.name),
        })
        .collect()
}

/// The findings as compiler-style lines, `file:line: rule message`.
#[must_use]
pub fn finding_lines(spec: &FindingsSpec) -> String {
    spec.findings
        .iter()
        .map(|f| {
            let at = match &f.location {
                Some(SourceRef::File {
                    path,
                    line_start: Some(line),
                    ..
                }) => format!("{path}:{line}"),
                Some(SourceRef::File { path, .. }) => path.clone(),
                _ => "-".to_owned(),
            };
            let first_line = f.message.lines().next().unwrap_or_default();
            match &f.rule {
                Some(rule) => format!("{at}: {rule} {first_line}\n"),
                None => format!("{at}: {first_line}\n"),
            }
        })
        .collect()
}

/// The copy forms of a computed card: the APA line, the reproduce lines
/// it has and the envelope JSON.
#[must_use]
pub fn computed(envelope: &Computed, body: &ComputedBody) -> Vec<CopyFormat> {
    let mut formats = Vec::new();
    if let Some(line) = narrative::apa_line(envelope, body) {
        formats.push(CopyFormat::new("apa", "APA", line.plain()));
    }
    let reproduce = &envelope.reproduce;
    for (key, label, line) in [
        ("lua", "Lua", &reproduce.lua),
        ("python", "Python", &reproduce.python),
        ("r", "R", &reproduce.r),
        ("shell", "shell", &reproduce.shell),
    ] {
        if let Some(line) = line {
            formats.push(CopyFormat::new(key, label, line.clone()));
        }
    }
    formats.extend(computed_body(body));
    formats.push(CopyFormat::new(
        "json",
        "JSON",
        serde_json::to_string_pretty(envelope).unwrap_or_default(),
    ));
    formats
}

/// The tables of a phase-2 card body as CSV, e.g. the histogram bins of a
/// distribution.
#[must_use]
pub fn computed_body(body: &ComputedBody) -> Vec<CopyFormat> {
    let mut formats = Vec::new();
    if let ComputedBody::Distribution(d) = body {
        let mut csv = String::from("group,lo,hi,count\n");
        for group in &d.groups {
            let name = csv_field(group.name.as_deref().unwrap_or(&d.variable));
            let h = &group.histogram;
            for (i, count) in h.counts.iter().enumerate() {
                csv.push_str(&format!(
                    "{name},{},{},{count}\n",
                    h.edges[i],
                    h.edges[i + 1]
                ));
            }
        }
        formats.push(CopyFormat::new("bins", "bins CSV", csv));
    }
    if let ComputedBody::Contingency(t) = body {
        let mut csv =
            String::from("row,column,observed,expected,row_percent,column_percent,std_residual\n");
        for (i, row) in t.row_levels.iter().enumerate() {
            for (j, col) in t.column_levels.iter().enumerate() {
                csv.push_str(&format!(
                    "{},{},{},{},{},{},{}\n",
                    csv_field(row),
                    csv_field(col),
                    t.observed[i][j],
                    t.expected[i][j],
                    t.row_percent[i][j],
                    t.column_percent[i][j],
                    t.std_residuals[i][j]
                ));
            }
        }
        formats.push(CopyFormat::new("cells", "cells CSV", csv));
    }
    if let ComputedBody::Power(p) = body {
        let mut csv = String::from("n,power\n");
        for (n, power) in p.curve.x.iter().zip(&p.curve.y) {
            csv.push_str(&format!("{n},{power}\n"));
        }
        formats.push(CopyFormat::new("curve", "power curve CSV", csv));
    }
    if let ComputedBody::AbTest(t) = body {
        let mut csv = String::from("arm,visitors,conversions,rate,rate_lo,rate_hi\n");
        for arm in &t.arms {
            csv.push_str(&format!(
                "{},{},{},{},{},{}\n",
                csv_field(&arm.name),
                arm.visitors,
                arm.conversions,
                arm.rate,
                arm.rate_lo,
                arm.rate_hi
            ));
        }
        formats.push(CopyFormat::new("arms", "arms CSV", csv));
    }
    if let ComputedBody::Bootstrap(b) = body {
        let mut csv = String::from("method,lo,hi,lo_mcse,hi_mcse\n");
        let opt = |m: Option<f64>| m.map(|m| m.to_string()).unwrap_or_default();
        for i in &b.intervals {
            csv.push_str(&format!(
                "{},{},{},{},{}\n",
                i.method.label(),
                i.lo,
                i.hi,
                opt(i.lo_mcse),
                opt(i.hi_mcse)
            ));
        }
        formats.push(CopyFormat::new("intervals", "intervals CSV", csv));
    }
    if let ComputedBody::Forecast(f) = body {
        let mut csv = String::from("t,forecast,lo80,hi80,lo95,hi95\n");
        let n = f.values.len();
        for (i, point) in f.forecast.iter().enumerate() {
            let at = |v: &[f64]| v.get(i).map(ToString::to_string).unwrap_or_default();
            csv.push_str(&format!(
                "{},{point},{},{},{},{}\n",
                n + i,
                at(&f.lo80),
                at(&f.hi80),
                at(&f.lo95),
                at(&f.hi95)
            ));
        }
        formats.push(CopyFormat::new("forecast", "forecast CSV", csv));
    }
    if let ComputedBody::Optimization(o) = body {
        let opt = |x: Option<f64>| x.map(|x| x.to_string()).unwrap_or_default();
        let mut csv = String::from("kind,name,value,slack_or_lower,dual_or_reduced_cost\n");
        for v in &o.variables {
            csv.push_str(&format!(
                "variable,{},{},{},{}\n",
                csv_field(&v.name),
                opt(v.value),
                opt(v.lower),
                opt(v.reduced_cost)
            ));
        }
        for k in &o.constraints {
            csv.push_str(&format!(
                "constraint,{},{},{},{}\n",
                csv_field(&k.name),
                opt(k.activity),
                opt(k.slack),
                opt(k.dual)
            ));
        }
        formats.push(CopyFormat::new("solution", "solution CSV", csv));
    }
    if let ComputedBody::Equations(e) = body {
        let mut csv = String::from("equation,lhs,rhs,residual\n");
        for p in &e.plug_back {
            csv.push_str(&format!(
                "{},{},{},{}\n",
                csv_field(&p.equation),
                p.lhs,
                p.rhs,
                p.residual
            ));
        }
        formats.push(CopyFormat::new("plug_back", "plug-back CSV", csv));
    }
    if let ComputedBody::Simulation(m) = body {
        let mut csv =
            String::from("input,distribution,p10,p50,p90,output_at_p10,output_at_p90,swing\n");
        for i in &m.inputs {
            let bar = m.tornado.iter().find(|b| b.input == i.name);
            let opt = |f: fn(&domain::computed::TornadoBar) -> f64| {
                bar.map(|b| f(b).to_string()).unwrap_or_default()
            };
            csv.push_str(&format!(
                "{},{},{},{},{},{},{},{}\n",
                csv_field(&i.name),
                csv_field(&i.distribution),
                i.p10,
                i.p50,
                i.p90,
                opt(|b| b.at_p10),
                opt(|b| b.at_p90),
                opt(|b| b.swing)
            ));
        }
        formats.push(CopyFormat::new("tornado", "tornado CSV", csv));
    }
    if let ComputedBody::CashFlow(f) = body {
        let mut csv = String::from(
            "period,cash_flow,discount_factor,present_value,cumulative,cumulative_pv\n",
        );
        for r in &f.rows {
            csv.push_str(&format!(
                "{},{},{},{},{},{}\n",
                r.period,
                r.cash_flow,
                r.discount_factor,
                r.present_value,
                r.cumulative,
                r.cumulative_pv
            ));
        }
        formats.push(CopyFormat::new("periods", "periods CSV", csv));
    }
    if let ComputedBody::DecisionMatrix(d) = body {
        let mut csv = String::from("option");
        for c in &d.criteria {
            csv.push(',');
            csv.push_str(&csv_field(&c.name));
        }
        csv.push_str(",total,rank\n");
        for o in &d.options {
            csv.push_str(&csv_field(&o.name));
            for x in &o.normalized {
                csv.push_str(&format!(",{x}"));
            }
            csv.push_str(&format!(",{},{}\n", o.total, o.rank));
        }
        formats.push(CopyFormat::new("matrix", "matrix CSV", csv));
    }
    if let ComputedBody::Bode(b) = body {
        let mut csv = format!(
            "w_{},magnitude_db,phase_deg,re,im\n",
            b.unit.symbol().replace('/', "_per_")
        );
        for p in &b.points {
            csv.push_str(&format!(
                "{},{},{},{},{}\n",
                b.unit.from_rad(p.w),
                p.magnitude_db,
                p.phase_deg,
                p.re,
                p.im
            ));
        }
        formats.push(CopyFormat::new("response", "frequency response CSV", csv));
    }
    if let ComputedBody::Karnaugh(k) = body {
        formats.push(CopyFormat::new("sop", "SOP", k.sop_expression.clone()));
        formats.push(CopyFormat::new("pos", "POS", k.pos_expression.clone()));
        formats.push(CopyFormat::new(
            "truth",
            "truth table CSV",
            k.truth_table_csv(),
        ));
    }
    if let ComputedBody::Beam(b) = body {
        formats.push(CopyFormat::new(
            "diagrams",
            "shear/moment/deflection CSV",
            b.to_csv(),
        ));
    }
    if let ComputedBody::Tolerance(t) = body {
        formats.push(CopyFormat::new("chain", "stack chain CSV", t.to_csv()));
    }
    if let ComputedBody::Bom(b) = body {
        formats.push(CopyFormat::new("bom", "costed BOM CSV", b.to_csv()));
    }
    formats
}

/// The copy forms of a molecule: the SMILES as given and what was
/// computed from it.
#[must_use]
pub fn molecule(spec: &MoleculeSpec) -> Vec<CopyFormat> {
    let mut formats = vec![CopyFormat::new("smiles", "SMILES", spec.smiles.clone())];
    for (key, label, value) in [
        ("canonical", "canonical SMILES", &spec.canonical),
        ("formula", "formula", &spec.formula),
        ("svg", "SVG", &spec.svg),
    ] {
        if let Some(value) = value {
            formats.push(CopyFormat::new(key, label, value.clone()));
        }
    }
    formats
}

/// The copy forms of one widget node of a `Custom` tree.
fn widget(node: &CustomNode) -> Vec<CopyFormat> {
    match node {
        CustomNode::Timeline(spec) => {
            vec![CopyFormat::new("spans", "spans CSV", spec.to_csv())]
        }
        CustomNode::FilterList(spec) => {
            vec![CopyFormat::new("lines", "lines", spec.all_text())]
        }
        CustomNode::Graph(spec) => vec![CopyFormat::new("dot", "DOT", spec.to_dot())],
        CustomNode::Tree(spec) => vec![CopyFormat::new("tree", "indented text", spec.to_text())],
        _ => Vec::new(),
    }
}

/// The copy forms of the widgets in a `Custom` tree, depth first; when
/// two widgets have a form with the same key, the first one's is kept.
#[must_use]
pub fn custom(tree: &CustomNode) -> Vec<CopyFormat> {
    fn walk(node: &CustomNode, out: &mut Vec<CopyFormat>) {
        match node {
            CustomNode::Row { children }
            | CustomNode::Col { children }
            | CustomNode::List { items: children } => {
                children.iter().for_each(|child| walk(child, out));
            }
            other => {
                for format in widget(other) {
                    if !out.iter().any(|f| f.key == format.key) {
                        out.push(format);
                    }
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(tree, &mut out);
    out
}

/// The forms `kind` can be copied in; empty for kinds without any.
#[must_use]
pub fn formats(kind: &BlockKind) -> Vec<CopyFormat> {
    match kind {
        BlockKind::Table(table) => vec![
            CopyFormat::new("tsv", "TSV", tsv(table)),
            CopyFormat::new("csv", "CSV", csv(table)),
        ],
        BlockKind::Chart(chart) => {
            let table = chart_table(chart);
            vec![
                CopyFormat::new("csv", "CSV", csv(&table)),
                CopyFormat::new("tsv", "TSV", tsv(&table)),
            ]
        }
        BlockKind::Plot(spec) => vec![CopyFormat::new(
            "json",
            "JSON",
            serde_json::to_string_pretty(spec).unwrap_or_default(),
        )],
        BlockKind::Computed { envelope, body } => computed(envelope, body),
        BlockKind::Mermaid(spec) => vec![CopyFormat::new("source", "source", spec.source.clone())],
        BlockKind::TestReport(spec) => {
            let failing = failing_tests(spec);
            if failing.is_empty() {
                Vec::new()
            } else {
                vec![CopyFormat::new("failing", "failing tests", failing)]
            }
        }
        BlockKind::Findings(spec) if !spec.findings.is_empty() => {
            vec![CopyFormat::new(
                "lines",
                "file:line list",
                finding_lines(spec),
            )]
        }
        BlockKind::Shell { stdout, stderr, .. } => {
            let plain = super::ansi::parse(&format!("{stdout}{stderr}")).text;
            vec![CopyFormat::new("plain", "plain output", plain)]
        }
        BlockKind::Dataset(spec) => vec![
            CopyFormat::new("profile", "profile CSV", super::dataset::profile_csv(spec)),
            CopyFormat::new("csv", "preview CSV", csv(&spec.preview)),
        ],
        BlockKind::Molecule(spec) => molecule(spec),
        BlockKind::Plate(spec) => {
            vec![CopyFormat::new("csv", "CSV", super::plate::plate_csv(spec))]
        }
        BlockKind::Custom { tree, .. } => custom(tree),
        BlockKind::Trace(spec) => vec![
            CopyFormat::new("spans", "spans CSV", spec.timeline.to_csv()),
            CopyFormat::new("critical", "critical path", critical_path_text(spec)),
        ],
        BlockKind::FlameGraph(spec) => vec![
            CopyFormat::new("folded", "folded stacks", spec.to_folded()),
            CopyFormat::new("top", "self-time CSV", frame_stats_csv(spec)),
        ],
        BlockKind::Logs(spec) => {
            vec![CopyFormat::new("lines", "lines", spec.list.all_text())]
        }
        BlockKind::QueryPlan(spec) => vec![
            CopyFormat::new("tree", "tree", spec.to_tree().to_text()),
            CopyFormat::new("nodes", "nodes CSV", spec.to_csv()),
        ],
        BlockKind::Benchmarks(spec) => {
            vec![CopyFormat::new("csv", "CSV", spec.to_csv())]
        }
        BlockKind::Waveform(spec) => vec![
            CopyFormat::new("changes", "changes CSV", spec.to_csv()),
            CopyFormat::new("end", "values at end", spec.values_at(spec.end, spec.radix)),
        ],
        BlockKind::Netlist(spec) => vec![
            CopyFormat::new("nets", "nets CSV", spec.nets_csv()),
            CopyFormat::new("components", "components CSV", spec.components_csv()),
            CopyFormat::new("dot", "DOT", spec.to_graph().to_dot()),
        ],
        BlockKind::Material(spec) => vec![CopyFormat::new("csv", "properties CSV", spec.to_csv())],
        BlockKind::CadModel(spec) => vec![
            CopyFormat::new("solids", "solids CSV", spec.to_csv()),
            CopyFormat::new("summary", "summary", super::cad::summary_text(spec)),
        ],
        BlockKind::Drawing(spec) => vec![
            CopyFormat::new("layers", "layers CSV", spec.layers_csv()),
            CopyFormat::new("texts", "texts", spec.texts().join("\n")),
            CopyFormat::new("summary", "summary", super::drawing::summary_text(spec)),
        ],
        BlockKind::IfcSchedule(spec) => vec![
            CopyFormat::new("spaces", "spaces CSV", spec.to_csv()),
            CopyFormat::new("tree", "schedule", spec.to_tree().to_text()),
        ],
        BlockKind::Sequence(spec) => {
            let mut formats = vec![
                CopyFormat::new("fasta", "FASTA", spec.to_fasta()),
                CopyFormat::new("features", "features CSV", spec.features_csv()),
            ];
            if spec.enzyme_source.is_some() {
                formats.push(CopyFormat::new("sites", "cut sites CSV", spec.sites_csv()));
            }
            formats
        }
        BlockKind::Spectrum(spec) => vec![
            CopyFormat::new("peaks", "peaks CSV", spec.peaks_csv()),
            CopyFormat::new("points", "points CSV", spec.to_csv()),
        ],
        BlockKind::MoleculeGrid(spec) => vec![
            CopyFormat::new("csv", "CSV", spec.to_csv()),
            CopyFormat::new(
                "smiles",
                "SMILES",
                spec.molecules
                    .iter()
                    .map(|m| format!("{} {}", m.molecule.smiles, m.id))
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
        ],
        BlockKind::Reaction(spec) => {
            let mut formats = vec![
                CopyFormat::new("smiles", "reaction SMILES", spec.smiles.clone()),
                CopyFormat::new("equation", "equation", spec.equation(None)),
            ];
            if let domain::science::reaction::Stoichiometry::Suggested { coefficients } =
                &spec.stoichiometry
            {
                formats.push(CopyFormat::new(
                    "balanced",
                    "balanced equation",
                    spec.equation(Some(coefficients)),
                ));
            }
            formats.push(CopyFormat::new("balance", "balance CSV", spec.balance_csv()));
            formats
        }
        BlockKind::Diagram(spec) if spec.tables.is_empty() => {
            vec![CopyFormat::new("dot", "DOT", spec.graph.to_dot())]
        }
        BlockKind::Diagram(spec) => vec![
            CopyFormat::new("mermaid", "Mermaid", spec.to_mermaid()),
            CopyFormat::new("dot", "DOT", spec.graph.to_dot()),
            CopyFormat::new("tables", "tables", spec.tables_tree().to_text()),
        ],
        BlockKind::TerraformPlan(spec) => vec![
            CopyFormat::new("tree", "tree", spec.to_tree().to_text()),
            CopyFormat::new("changes", "changes CSV", spec.to_csv()),
        ],
        BlockKind::Har(spec) => vec![
            CopyFormat::new("requests", "requests CSV", spec.to_csv()),
            CopyFormat::new(
                "urls",
                "URLs",
                spec.entries
                    .iter()
                    .map(|e| format!("{} {}\n", e.method, domain::devops::har::redact_url(&e.url)))
                    .collect(),
            ),
        ],
        _ => Vec::new(),
    }
}

/// The critical path of a trace, one `service: name<TAB>duration` line
/// per span, root first.
#[must_use]
pub fn critical_path_text(spec: &TraceSpec) -> String {
    let times = spec.times(None);
    spec.critical_path
        .iter()
        .filter_map(|id| spec.timeline.rows.iter().position(|r| &r.id == id))
        .map(|ix| {
            format!(
                "{}{}\t{}\n",
                "  ".repeat(spec.timeline.rows[ix].depth as usize),
                spec.span_name(ix),
                format_duration(times[ix].duration, TimeUnit::Ms)
            )
        })
        .collect()
}

/// Self and total value per function of a flame graph as CSV, most self
/// first.
#[must_use]
pub fn frame_stats_csv(spec: &FlameSpec) -> String {
    let mut out = format!("function,self_{0},total_{0}\n", spec.unit.replace(' ', "_"));
    for stat in spec.frame_stats() {
        out.push_str(&format!(
            "{},{},{}\n",
            csv_field(&stat.name),
            stat.self_value,
            stat.total
        ));
    }
    out
}

/// The copy row of the selected block, or `None` when it has no forms or
/// shows its own buttons (computed cards).
pub fn row(entry: &TimelineEntry, cx: &App) -> Option<AnyElement> {
    if matches!(entry.block.kind, BlockKind::Computed { .. }) {
        return None;
    }
    let formats = formats(&entry.block.kind);
    if formats.is_empty() {
        return None;
    }
    let theme = cx.theme();
    let id = entry.block.id;
    let mut row = h_flex()
        .gap_1()
        .pt_1()
        .text_xs()
        .text_color(theme.muted_foreground)
        .debug_selector(|| "copy-row".to_owned())
        .child("copy as");
    for format in formats {
        let key = format.key;
        let text = format.text;
        row = row.child(
            div()
                .id(ElementId::Name(SharedString::from(format!(
                    "copy-{key}-{id}"
                ))))
                .debug_selector(move || format!("copy-{key}"))
                .px_1p5()
                .rounded_sm()
                .border_1()
                .border_color(theme.border)
                .cursor_pointer()
                .hover(|style| style.bg(theme.secondary_hover))
                .child(format.label)
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                }),
        );
    }
    Some(row.into_any_element())
}

#[cfg(test)]
mod tests {
    use domain::block::{ChartKind, Column, MermaidSpec, Series};
    use domain::report::{Finding, FindingSeverity, TestCase, TestSuite};

    use super::*;

    fn table() -> TableSpec {
        TableSpec {
            columns: vec![Column::new("name"), Column::new("note, quoted")],
            rows: vec![
                vec![Cell::Text("a\tb".into()), Cell::Text("say \"hi\"".into())],
                vec![Cell::Number(1.5), Cell::Null],
                vec![Cell::Bool(true), Cell::Text("two\nlines".into())],
            ],
        }
    }

    #[test]
    fn csv_quotes_and_tsv_flattens() {
        assert_eq!(
            csv(&table()),
            "name,\"note, quoted\"\na\tb,\"say \"\"hi\"\"\"\n1.5,\ntrue,\"two\nlines\"\n"
        );
        assert_eq!(
            tsv(&table()),
            "name\tnote, quoted\na b\tsay \"hi\"\n1.5\t\ntrue\ttwo lines\n"
        );
        assert_eq!(csv_field("plain"), "plain");
        assert_eq!(csv_field("a\r\nb"), "\"a\r\nb\"");
        assert_eq!(tsv_field("a\r\nb"), "a b");
    }

    #[test]
    fn charts_copy_their_data_table() {
        let chart = ChartSpec {
            title: "t".into(),
            kind: ChartKind::Bar,
            labels: vec!["rs".into(), "md".into()],
            series: vec![Series {
                name: "bytes".into(),
                values: vec![1200.0, 300.0],
            }],
        };
        let formats = formats(&BlockKind::Chart(chart));
        assert_eq!(formats[0].key, "csv");
        assert_eq!(formats[0].text, ",bytes\nrs,1200\nmd,300\n");
    }

    #[test]
    fn card_bodies_copy_their_tables_as_csv() {
        let keys = |body: ComputedBody| {
            computed_body(&body)
                .into_iter()
                .map(|f| (f.key, f.text))
                .collect::<Vec<_>>()
        };
        let bins = keys(ComputedBody::Distribution(
            super::super::computed::distribution::tests::body(),
        ));
        assert_eq!(bins[0].0, "bins");
        assert!(
            bins[0].1.starts_with("group,lo,hi,count\n0,0,1,3\n"),
            "{}",
            bins[0].1
        );
        let lp = keys(ComputedBody::Optimization(
            super::super::computed::optimization::tests::body(),
        ));
        assert!(
            lp[0].1.contains("constraint,plant2,12,0,1.5"),
            "{}",
            lp[0].1
        );
        let matrix = keys(ComputedBody::DecisionMatrix(
            super::super::computed::decision_matrix::tests::body(),
        ));
        assert!(
            matrix[0]
                .1
                .starts_with("option,price,quality,total,rank\nA,1,0,0.6,1"),
            "{}",
            matrix[0].1
        );
        let flows = keys(ComputedBody::CashFlow(
            super::super::computed::cash_flow::tests::body(),
        ));
        assert_eq!(flows[0].1.lines().count(), 4);
    }

    #[test]
    fn devops_blocks_copy_spans_stacks_and_lines() {
        use domain::devops::flame::{FlameTree, ProfileFormat};
        let mut tree = FlameTree::new();
        tree.add(["main", "work"], 3);
        tree.add(["main"], 1);
        let flame = tree.finish(ProfileFormat::Folded, "cpu nanoseconds");
        let forms = formats(&BlockKind::FlameGraph(Box::new(flame)));
        assert_eq!(forms[0].text, "main 1\nmain;work 3\n");
        assert_eq!(
            forms[1].text,
            "function,self_cpu_nanoseconds,total_cpu_nanoseconds\nwork,3,3\nmain,1,4\n"
        );
        let trace: TraceSpec = serde_json::from_value(serde_json::json!({
            "format": "otlp", "traces": 1, "total_spans": 2,
            "timeline": {"rows": [
                {"id": "a", "label": "GET /", "spans": [{"start": 0, "end": 10,
                    "attrs": {"service": "api"}}]},
                {"id": "b", "label": "query", "depth": 1, "spans": [{"start": 2, "end": 7}]}
            ]},
            "critical_path": ["a", "b"]
        }))
        .unwrap();
        assert_eq!(
            critical_path_text(&trace),
            "api: GET /\t10 ms\n  query\t5 ms\n"
        );
        let keys: Vec<_> = formats(&BlockKind::Trace(Box::new(trace)))
            .iter()
            .map(|f| f.key)
            .collect();
        assert_eq!(keys, ["spans", "critical"]);
    }

    #[test]
    fn lab_blocks_copy_their_values() {
        let spec = MoleculeSpec {
            smiles: "OCC".into(),
            name: None,
            svg: None,
            formula: Some("C2H6O".into()),
            mw: Some(46.07),
            charge: Some(0),
            canonical: Some("CCO".into()),
            error: None,
            claimed_formula: None,
            claimed_matches: None,
        };
        let keys: Vec<_> = formats(&BlockKind::Molecule(spec))
            .iter()
            .map(|f| (f.key, f.text.clone()))
            .collect();
        assert_eq!(
            keys,
            [
                ("smiles", "OCC".to_owned()),
                ("canonical", "CCO".to_owned()),
                ("formula", "C2H6O".to_owned())
            ]
        );
        let plate = domain::science::PlateSpec {
            title: None,
            format: 96,
            value_label: None,
            wells: vec![domain::science::Well {
                well: "a2".into(),
                label: None,
                value: Some(3.0),
            }],
            scale: domain::science::PlateScale::Sequential,
            center: None,
        };
        let plate = formats(&BlockKind::Plate(plate));
        assert_eq!(plate[0].text, "well,label,value\nA2,,3\n");
    }

    #[test]
    fn reports_copy_what_failed() {
        let case = |name: &str, outcome| TestCase {
            name: name.into(),
            classname: Some("m".into()),
            outcome,
            time_secs: None,
            message: None,
            details: None,
            location: None,
        };
        let report = TestReportSpec {
            title: None,
            suites: vec![TestSuite {
                name: "s".into(),
                time_secs: None,
                cases: vec![
                    case("ok", TestOutcome::Passed),
                    case("bad", TestOutcome::Failed),
                    case("boom", TestOutcome::Errored),
                ],
            }],
        };
        assert_eq!(failing_tests(&report), "m::bad\nm::boom\n");
        let findings = FindingsSpec {
            tool: None,
            findings: vec![
                Finding {
                    rule: Some("clippy::unwrap_used".into()),
                    severity: FindingSeverity::Warning,
                    message: "used unwrap\nmore".into(),
                    location: Some(SourceRef::File {
                        path: "src/a.rs".into(),
                        line_start: Some(4),
                        line_end: Some(6),
                    }),
                    snippet: None,
                    help_uri: None,
                    fix: None,
                    fixes: Vec::new(),
                },
                Finding {
                    rule: None,
                    severity: FindingSeverity::Note,
                    message: "no place".into(),
                    location: None,
                    snippet: None,
                    help_uri: None,
                    fix: None,
                    fixes: Vec::new(),
                },
            ],
        };
        assert_eq!(
            finding_lines(&findings),
            "src/a.rs:4: clippy::unwrap_used used unwrap\n-: no place\n"
        );
        let mermaid = BlockKind::Mermaid(MermaidSpec {
            source: "graph TD; a-->b".into(),
            title: None,
        });
        assert_eq!(formats(&mermaid)[0].text, "graph TD; a-->b");
        let shell = BlockKind::Shell {
            command: "ls".into(),
            exit_code: Some(0),
            stdout: "\x1b[31mred\x1b[0m\n".into(),
            stderr: String::new(),
            timed_out: false,
            truncated: false,
        };
        assert_eq!(formats(&shell)[0].text, "red\n");
        assert!(
            formats(&BlockKind::Text {
                markdown: "x".into()
            })
            .is_empty()
        );
    }

    #[gpui::test]
    fn the_selected_block_copies_as_csv(cx: &mut gpui::TestAppContext) {
        use application::keymap::Action;
        use application::ports::events::DomainEvent;
        use domain::block::Block;
        use gpui::{Modifiers, point, px, size};

        use crate::timeline::TimelineView;

        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, cx| TimelineView::new(cx));
        let block = Block::new(BlockKind::Table(table()));
        view.update(cx, |view, cx| {
            view.apply_all([DomainEvent::BlockAdded { block }], cx);
        });
        let draw = |cx: &mut gpui::VisualTestContext| {
            cx.draw(point(px(0.), px(0.)), size(px(800.), px(600.)), |_, _| {
                div().size_full().child(view.clone())
            });
            cx.run_until_parked();
        };
        draw(cx);
        assert!(cx.debug_bounds("copy-row").is_none(), "only when selected");
        view.update(cx, |view, cx| {
            view.handle_action(&Action::builtin("block.last"), cx);
        });
        draw(cx);
        let button = cx
            .debug_bounds("copy-csv")
            .unwrap_or_else(|| panic!("no copy button"));
        cx.simulate_click(button.center(), Modifiers::none());
        let copied = cx.read_from_clipboard().and_then(|item| item.text());
        assert_eq!(copied.as_deref(), Some(csv(&table()).as_str()));
    }
}
