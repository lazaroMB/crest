//! Mermaid blocks: a flowchart and a sequence diagram (drag to pan,
//! ctrl+wheel or +/- to zoom), then invalid sources that show their code
//! and the error. The script toggles each block's source view with `t`.
//!
//! `cargo run -p ui --example mermaid_block [--quit]`

#[path = "support/demo.rs"]
mod demo;
#[path = "support/mod.rs"]
mod support;

use domain::block::{Block, BlockKind, MermaidSpec};
use ui::blocks::mermaid::diagram;

const FLOWCHART: &str = "flowchart LR
    U[User] -->|prompt| A(Agent)
    A --> T{Tool?}
    T -->|yes| R[rg / fzf]
    T -->|no| M[Model]
    R --> A
    M -.->|answer| U
";

const SEQUENCE: &str = "sequenceDiagram
    participant UI
    participant Core
    participant API as Anthropic
    UI->>Core: send prompt
    Core->>API: messages.create (stream)
    API-->>Core: text_delta
    Core-->>UI: BlockUpdated
    Note over UI,Core: the timeline repaints
";

fn mermaid(title: &str, source: &str) -> Block {
    Block::new(BlockKind::Mermaid(MermaidSpec {
        source: source.to_owned(),
        title: Some(title.to_owned()),
    }))
}

fn main() {
    let garbage = String::from_utf8_lossy(&[0, 0xff, 0xfe, 7, 27, b'g', b'r', 0x80, b'x']);
    let diagrams = [
        ("Agent flow", FLOWCHART.to_owned()),
        ("Streaming", SEQUENCE.to_owned()),
        ("Invalid", "flowchart LR\n  A --> \n  [[[".to_owned()),
        ("Empty", String::new()),
        ("Binary garbage", garbage.into_owned()),
    ];
    for (title, source) in &diagrams {
        match diagram::parse(source) {
            Ok(d) => eprintln!(
                "mermaid_block: {title}: {} shapes, {:.0}x{:.0}",
                d.shapes.len(),
                d.width,
                d.height
            ),
            Err(err) => eprintln!("mermaid_block: {title}: error: {err}"),
        }
    }
    let blocks = diagrams
        .iter()
        .map(|(title, source)| mermaid(title, source))
        .collect();
    let mut script = vec!["block.first"];
    for _ in 0..diagrams.len() {
        script.extend(["block.toggle_view", "block.toggle_view", "block.next"]);
    }
    demo::run("crest — mermaid block", blocks, script);
}
