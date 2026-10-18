//! The session timeline: a pure model ([`TimelineState`]) fed by domain
//! events and its virtualized GPUI view ([`TimelineView`]).

mod render;
mod state;
mod view;

pub use render::{copy_text, kind_name, render_block, status_label, tool_call_line};
pub use state::{TimelineChange, TimelineEntry, TimelineState, has_alt_view};
pub use view::TimelineView;
