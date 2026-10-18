//! The session timeline: a pure model ([`TimelineState`]) fed by domain
//! events.

mod state;

pub use state::{TimelineChange, TimelineEntry, TimelineState};
