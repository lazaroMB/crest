//! Blocks of the investigative flow: a stage header with progress, the
//! sources a node cited (click a file or URL to open it) and images with
//! their source, one of them failing to load.
//!
//! `cargo run -p ui --example research_blocks [--quit]`

#[path = "support/demo.rs"]
mod demo;
#[path = "support/mod.rs"]
mod support;

use domain::block::{Block, BlockKind, ImageData, SourceRef};

/// A 1×1 PNG, enough to show the inline path.
const PIXEL: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";

fn main() {
    let blocks = vec![
        Block::new(BlockKind::StageHeader {
            stage: "Hypothesis".into(),
            index: 5,
            total: 14,
        }),
        Block::new(BlockKind::Text {
            markdown: "The parser drops the last token [1], introduced in [2]; see [3].".into(),
        }),
        Block::new(BlockKind::Sources {
            sources: vec![
                SourceRef::File {
                    path: "crates/domain/src/block.rs".into(),
                    line_start: Some(523),
                    line_end: Some(561),
                },
                SourceRef::Commit {
                    sha: "5368c50e0c0ffee".into(),
                    summary: "feat(ui): draw stage headers with a progress bar".into(),
                },
                SourceRef::Url {
                    url: "https://docs.rs/gpui-component/latest".into(),
                    title: Some("gpui-component docs".into()),
                },
                SourceRef::Command {
                    command: "cargo test -p ui".into(),
                    output_excerpt: "test result: ok. 99 passed\nfinished in 0.4s".into(),
                },
            ],
        }),
        Block::new(BlockKind::Image {
            media_type: "image/png".into(),
            data_or_path: ImageData::Base64 { data: PIXEL.into() },
            source: Some(SourceRef::Url {
                url: "https://example.com/screenshot".into(),
                title: Some("captura".into()),
            }),
        }),
        Block::new(BlockKind::Image {
            media_type: "image/png".into(),
            data_or_path: ImageData::Path {
                path: "/nonexistent/chart.png".into(),
            },
            source: None,
        }),
        Block::new(BlockKind::StageHeader {
            stage: "Experimento".into(),
            index: 6,
            total: 14,
        }),
    ];
    demo::run(
        "crest — research blocks",
        blocks,
        vec!["block.first", "block.next", "block.next", "block.last"],
    );
}
