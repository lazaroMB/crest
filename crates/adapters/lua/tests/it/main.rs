//! The integration tests of this crate, as modules of one test binary.
//!
//! One binary instead of one per file: each test binary links the whole
//! dependency tree again, which dominated the build time of `cargo test`.
//! Add a new test file here as a module.

mod common;
mod example_themes;
mod plugin_caps;
mod plugin_hooks;
mod plugin_modal;
mod plugin_tools;
mod plugin_ui;
mod showcase_plugin;
