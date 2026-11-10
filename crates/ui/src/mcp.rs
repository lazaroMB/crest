//! The MCP servers manager: the `/mcp` modal that lists the servers of
//! every layer, shows one with its state, tools and log, acts on it and
//! adds, edits or removes those of `mcp.json`, all while crest runs. The
//! sidebar's dock and the prompt's `+` menu open it too.

mod detail;
mod form;
mod list;
pub mod model;
mod view;

pub use form::{Draft, FormMode};
pub use model::{McpSummary, McpTab, Tone};
pub use view::{McpEvent, McpOpen, McpThen, McpView, mcp_modal_id};

/// The color of a tone, for the sidebar's dock.
pub fn tone_color(tone: Tone, cx: &gpui::App) -> gpui::Hsla {
    view::tone_color(tone, cx)
}
