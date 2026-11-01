//! The light half of crest's application layer: the port traits that
//! adapters implement, the agent loop and the use cases around it, and the
//! tool registry.
//!
//! The heavy half (the research and implementation flows, the file,
//! compute and formats tools) is the `application` crate, which re-exports
//! these modules. The UI and most adapters depend on this crate only, so a
//! change to a tool does not rebuild them.

pub mod cost;
pub mod csv;
pub mod decide;
pub mod keymap;
pub mod llm_retry;
pub mod ports;
pub mod privacy;
pub mod providers;
pub mod rtk;
pub mod shell;
// The fakes live in the application-testing crate so that other crates'
// tests get them without a feature on this crate (a feature would build
// this crate, and every crate above it, a second time for tests). They are
// written against `crate::ports`, so the same files compile here as well,
// for this crate's own unit tests (and in `application`, for its own).
#[cfg(test)]
#[path = "../../app-testing/src/fakes/mod.rs"]
pub mod testing;
pub mod theme;
pub mod tools;
pub mod usecases;
pub mod web;
pub mod window;
