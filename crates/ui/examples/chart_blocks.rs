//! Chart blocks of the four kinds (bar, line, area, pie), including the
//! layouts the renderer picks on its own: few bars, groups, negatives,
//! many categories with long labels (horizontal bars) and a dense line.
//! Each is toggled to its data table and back with `block.toggle_view`
//! (`t`).
//!
//! `cargo run -p ui --example chart_blocks [--quit]`

#[path = "support/demo.rs"]
mod demo;
#[path = "support/mod.rs"]
mod support;

use domain::block::{Block, BlockKind, ChartKind, ChartSpec, Series};

fn series(name: &str, values: &[f64]) -> Series {
    Series {
        name: name.into(),
        values: values.to_vec(),
    }
}

fn chart(title: &str, kind: ChartKind, labels: &[&str], series: Vec<Series>) -> Block {
    Block::new(BlockKind::Chart(ChartSpec {
        title: title.into(),
        kind,
        labels: labels.iter().map(|l| (*l).to_owned()).collect(),
        series,
    }))
}

fn main() {
    let exts = ["rs", "toml", "md", "lua", "json"];
    let months = ["Jan", "Feb", "Mar", "Apr", "May", "Jun"];
    let crates: Vec<String> = (0..30)
        .map(|i| format!("crates/subsystem_{i:02}/src/module"))
        .collect();
    let crates: Vec<&str> = crates.iter().map(String::as_str).collect();
    let hours: Vec<String> = (0..24).map(|h| format!("{h:02}:00")).collect();
    let hours: Vec<&str> = hours.iter().map(String::as_str).collect();
    let wave = |scale: f64, phase: f64| -> Vec<f64> {
        (0..24)
            .map(|h| (scale * (1.5 + (f64::from(h) / 3.0 + phase).sin())).round())
            .collect()
    };
    let blocks = vec![
        chart(
            "Open issues by label",
            ChartKind::Bar,
            &["bug", "feature", "docs"],
            vec![series("open", &[14.0, 23.0, 6.0])],
        ),
        chart(
            "Tests by crate and kind",
            ChartKind::Bar,
            &["domain", "application", "adapters", "ui", "crest"],
            vec![
                series("unit", &[180.0, 240.0, 95.0, 310.0, 40.0]),
                series("integration", &[12.0, 60.0, 48.0, 25.0, 30.0]),
                series("gpui", &[0.0, 0.0, 0.0, 120.0, 8.0]),
            ],
        ),
        chart(
            "Net lines per week",
            ChartKind::Bar,
            &["w1", "w2", "w3", "w4", "w5", "w6"],
            vec![series("net", &[420.0, -180.0, 960.0, -640.0, 75.0, 310.0])],
        ),
        chart(
            "Lines by module",
            ChartKind::Bar,
            &crates,
            vec![series(
                "lines",
                &(0..30)
                    .map(|i| f64::from((i * 7919) % 4000 + 200))
                    .collect::<Vec<_>>(),
            )],
        ),
        chart(
            "Requests per hour",
            ChartKind::Line,
            &hours,
            vec![
                series("api", &wave(120.0, 0.0)),
                series("web", &wave(80.0, 1.0)),
                series("jobs", &wave(30.0, 2.0)),
            ],
        ),
        chart(
            "Size by extension (KB)",
            ChartKind::Bar,
            &exts,
            vec![
                series("src", &[812.0, 9.8, 54.0, 12.5, 30.1]),
                series("tests", &[240.0, 1.2, 3.0, 4.4, 18.0]),
            ],
        ),
        chart(
            "Commits per month",
            ChartKind::Line,
            &months,
            vec![
                series("core", &[34.0, 41.0, 28.0, 55.0, 61.0, 47.0]),
                series("ui", &[12.0, 18.0, 25.0, 31.0, 22.0, 40.0]),
            ],
        ),
        chart(
            "Lines of code",
            ChartKind::Area,
            &months,
            vec![
                series("total", &[8.0, 11.5, 14.0, 18.2, 21.0, 25.5]),
                series("tests", &[2.0, 3.1, 4.4, 6.0, 7.2, 9.9]),
            ],
        ),
        chart(
            "Files by extension",
            ChartKind::Pie,
            &exts,
            vec![series("files", &[142.0, 12.0, 7.0, 9.0, 21.0])],
        ),
    ];
    let count = blocks.len();
    let mut script = vec!["block.first"];
    for _ in 0..count {
        script.extend(["block.toggle_view", "block.toggle_view", "block.next"]);
    }
    demo::run("crest — chart blocks", blocks, script);
}
