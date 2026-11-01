//! Use cases of crest and the port traits that adapters implement.

pub mod cost;
pub mod csv;
pub mod decide;
pub mod flows;
pub mod keymap;
pub mod llm_retry;
pub mod ports;
pub mod privacy;
pub mod providers;
pub mod rtk;
pub mod shell;
// The fakes live in the application-testing crate so that other crates'
// tests get them without a feature on this crate (a feature would build
// application, and every crate above it, a second time for tests). They are
// written against `crate::ports`, so the same files compile here as well,
// for this crate's own unit tests.
#[cfg(test)]
#[path = "../../app-testing/src/fakes/mod.rs"]
pub mod testing;
pub mod theme;
pub mod tools;
pub mod usecases;
pub mod web;
pub mod window;
