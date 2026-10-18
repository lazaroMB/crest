//! Chart blocks of the four kinds (bar, line, area, pie), each toggled to
//! its data table and back with `block.toggle_view` (`t`).
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
    let months = ["ene", "feb", "mar", "abr", "may", "jun"];
    let blocks = vec![
        chart(
            "Tamaño por extensión (KB)",
            ChartKind::Bar,
            &exts,
            vec![
                series("src", &[812.0, 9.8, 54.0, 12.5, 30.1]),
                series("tests", &[240.0, 1.2, 3.0, 4.4, 18.0]),
            ],
        ),
        chart(
            "Commits por mes",
            ChartKind::Line,
            &months,
            vec![
                series("core", &[34.0, 41.0, 28.0, 55.0, 61.0, 47.0]),
                series("ui", &[12.0, 18.0, 25.0, 31.0, 22.0, 40.0]),
            ],
        ),
        chart(
            "Líneas de código",
            ChartKind::Area,
            &months,
            vec![
                series("total", &[8.0, 11.5, 14.0, 18.2, 21.0, 25.5]),
                series("tests", &[2.0, 3.1, 4.4, 6.0, 7.2, 9.9]),
            ],
        ),
        chart(
            "Archivos por extensión",
            ChartKind::Pie,
            &exts,
            vec![series("archivos", &[142.0, 12.0, 7.0, 9.0, 21.0])],
        ),
    ];
    let mut script = vec!["block.first"];
    for _ in 0..4 {
        script.extend(["block.toggle_view", "block.toggle_view", "block.next"]);
    }
    demo::run("crest — chart blocks", blocks, script);
}
