//! The integration tests of this crate, as modules of one test binary.
//!
//! One binary instead of one per file: each test binary links the whole
//! dependency tree again, which dominated the build time of `cargo test`.
//! Add a new test file here as a module.

mod commands_during_turn;
mod headless_demo;
mod help_snapshot;
mod implement_demo;
mod mcp_servers;
mod model_selection;
mod plugin_caps;
mod plugin_modal;
mod research_demo;
mod resume;
mod simulate_demo;
mod ui_session;
mod web_research;
