//! Copy-as: the text forms a block can be copied in, and the row of
//! buttons the selected block shows for them.
//!
//! | block | forms |
//! | --- | --- |
//! | table | TSV, CSV |
//! | chart | CSV, TSV (its data table, [`ChartSpec::to_table`]) |
//! | plot | the `PlotSpec` JSON |
//! | computed card | APA line, reproduce lines, envelope JSON (in the card's own footer) |
//! | mermaid | source |
//! | test report | failing test names |
//! | findings | `file:line: rule message` lines |
//! | shell | the output without ANSI escapes |
//! | dataset | column profile CSV, preview rows CSV |
//! | molecule | SMILES, canonical SMILES, formula, SVG |
//! | plate | `well,label,value` CSV |
//! | custom | its widgets: timeline → spans CSV |
//!
//! Inline math chips copy their LaTeX themselves ([`super::math`]). `y` on
//! a selected block still copies its default text.

use domain::block::{BlockKind, Cell, ChartSpec, CustomNode, SourceRef, TableSpec};
use domain::computed::{Computed, ComputedBody, narrative};
use domain::report::{FindingsSpec, TestOutcome, TestReportSpec};
use domain::science::MoleculeSpec;
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
    formats.push(CopyFormat::new(
        "json",
        "JSON",
        serde_json::to_string_pretty(envelope).unwrap_or_default(),
    ));
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
        _ => Vec::new(),
    }
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
                },
                Finding {
                    rule: None,
                    severity: FindingSeverity::Note,
                    message: "no place".into(),
                    location: None,
                    snippet: None,
                    help_uri: None,
                    fix: None,
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
