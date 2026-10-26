//! The session timeline: a pure model ([`TimelineState`]) fed by domain
//! events and its virtualized GPUI view ([`TimelineView`]).

mod column;
mod render;
mod selection;
mod state;
mod view;

pub use column::{BlockWidth, COLUMN_GUTTER, PROSE_MAX_WIDTH, WIDE_MAX_WIDTH, block_width};
pub use render::{copy_text, kind_name, render_block, status_label, tool_call_line};
pub use selection::{CopyHint, copy_chord};
pub use state::{TimelineChange, TimelineEntry, TimelineState, has_alt_view};
pub use view::TimelineView;
