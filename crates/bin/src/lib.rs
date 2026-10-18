//! Composition root of crest: the only crate that depends on every layer.
//!
//! [`args`] parses the command line. The binary (`src/main.rs`) only turns
//! the parsed arguments into a run of the UI or of the headless mode.

pub mod args;
pub mod root;
pub mod startup;
