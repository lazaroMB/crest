//! Markdown document block: headings, a GFM table and highlighted code,
//! toggled to its source and back with `block.toggle_view` (`t`).
//!
//! `cargo run -p ui --example markdown_block [--quit]`

#[path = "support/demo.rs"]
mod demo;
#[path = "support/mod.rs"]
mod support;

use domain::block::{Block, BlockKind};

const DOC: &str = r#"# Size report

Workspace summary by **extension**, generated with `rg --files`.

## Table

| Extension | Files | Bytes |
| --- | ---: | ---: |
| `.rs` | 142 | 812 340 |
| `.toml` | 12 | 9 812 |
| `.md` | 7 | 54 001 |

## Code

```rust
fn bytes_by_extension(files: &[PathBuf]) -> BTreeMap<String, u64> {
    let mut sizes = BTreeMap::new();
    for file in files {
        let ext = file.extension().map(|e| e.to_string_lossy().into_owned());
        *sizes.entry(ext.unwrap_or_default()).or_default() += file.metadata().map_or(0, |m| m.len());
    }
    sizes
}
```

```bash
rg --files | awk -F. '{print $NF}' | sort | uniq -c
```

> Files ignored by `.gitignore` do not count.
"#;

fn main() {
    let blocks = vec![
        Block::new(BlockKind::Text {
            markdown: "Here is the report:".into(),
        }),
        Block::new(BlockKind::Markdown {
            title: Some("informe.md".into()),
            source: DOC.into(),
        }),
    ];
    demo::run(
        "crest — markdown block",
        blocks,
        vec![
            "block.last",
            "block.toggle_view",
            "block.toggle_view",
            "block.first",
            "block.toggle_view",
        ],
    );
}
