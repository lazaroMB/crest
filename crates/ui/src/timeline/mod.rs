//! The session timeline: a pure model ([`TimelineState`]) fed by domain
//! events.

mod render;
mod state;

pub use render::{copy_text, kind_name, render_block, status_label, tool_call_line};
pub use state::{TimelineChange, TimelineEntry, TimelineState};
