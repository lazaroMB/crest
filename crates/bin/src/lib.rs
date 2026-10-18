//! Composition root of crest: the only crate that depends on every layer.
//!
//! [`args`] parses the command line. The binary (`src/main.rs`) only turns
//! the parsed arguments into a run of the UI ([`gui`]) or of the headless
//! mode ([`headless`]).

pub mod args;
pub mod gui;
pub mod headless;
pub mod keys;
pub mod mcp;
pub mod plugin_trust;
pub mod research;
pub mod root;
pub mod startup;
pub mod telemetry;
