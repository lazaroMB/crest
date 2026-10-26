//! The timeline's reading column: blocks are centered and capped at one of
//! two widths, a narrow one for prose and a wider one for rich blocks
//! (charts, tables, diagrams). Below the cap a block takes the whole row, so
//! narrow windows need no breakpoints.

use domain::block::BlockKind;
use gpui::{Pixels, px};

/// Widest a prose block (text, thinking, tool calls) gets, in pixels.
pub const PROSE_MAX_WIDTH: f32 = 760.0;

/// Widest a rich block (charts, tables, diffs, diagrams) gets, in pixels.
pub const WIDE_MAX_WIDTH: f32 = 1100.0;

/// Horizontal space kept on each side of the column, in pixels.
pub const COLUMN_GUTTER: f32 = 16.0;

/// Which of the column's two widths a block uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockWidth {
    /// Text and other blocks read line by line.
    Prose,
    /// Charts, tables and other blocks that use the extra width.
    Wide,
}

impl BlockWidth {
    /// The largest width a block of this kind is drawn at.
    pub fn max_width(self) -> Pixels {
        match self {
            Self::Prose => px(PROSE_MAX_WIDTH),
            Self::Wide => px(WIDE_MAX_WIDTH),
        }
    }
}

/// The column width of a block kind.
pub fn block_width(kind: &BlockKind) -> BlockWidth {
    match kind {
        BlockKind::Chart(_)
        | BlockKind::Table(_)
        | BlockKind::Diff(_)
        | BlockKind::Mermaid(_)
        | BlockKind::Custom { .. }
        | BlockKind::Image { .. }
        | BlockKind::Shell { .. }
        | BlockKind::Computed { .. }
        | BlockKind::Plot(_)
        | BlockKind::TestReport(_)
        | BlockKind::Findings(_)
        | BlockKind::Dataset(_)
        | BlockKind::Plate(_)
        | BlockKind::Trace(_)
        | BlockKind::FlameGraph(_)
        | BlockKind::Logs(_)
        | BlockKind::QueryPlan(_)
        | BlockKind::Har(_)
        | BlockKind::TerraformPlan(_)
        | BlockKind::Diagram(_)
        | BlockKind::Benchmarks(_)
        | BlockKind::Waveform(_)
        | BlockKind::Netlist(_)
        | BlockKind::Material(_)
        | BlockKind::CadModel(_)
        | BlockKind::Drawing(_)
        | BlockKind::IfcSchedule(_)
        | BlockKind::Sequence(_)
        | BlockKind::Reaction(_)
        | BlockKind::MoleculeGrid(_)
        | BlockKind::Spectrum(_)
        | BlockKind::Molecule3d(_)
        | BlockKind::Ghs(_)
        | BlockKind::GeoMap(_)
        | BlockKind::Gantt(_)
        | BlockKind::ArrayFile(_) => BlockWidth::Wide,
        BlockKind::Text { .. }
        | BlockKind::Thinking { .. }
        | BlockKind::ToolCall { .. }
        | BlockKind::Error { .. }
        | BlockKind::Markdown { .. }
        | BlockKind::Sources { .. }
        | BlockKind::StageHeader { .. }
        | BlockKind::Form(_)
        | BlockKind::Molecule(_) => BlockWidth::Prose,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::block::{ChartKind, ChartSpec};

    #[test]
    fn rich_blocks_are_wide_and_text_is_prose() {
        let chart = BlockKind::Chart(ChartSpec {
            title: String::new(),
            kind: ChartKind::Bar,
            labels: vec![],
            series: vec![],
        });
        let text = BlockKind::Text {
            markdown: "hi".into(),
        };
        assert_eq!(block_width(&chart), BlockWidth::Wide);
        assert_eq!(block_width(&text), BlockWidth::Prose);
        assert!(BlockWidth::Prose.max_width() < BlockWidth::Wide.max_width());
    }
}
