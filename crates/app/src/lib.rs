//! The heavy half of crest's application layer: the research and
//! implementation flows, the tools the model calls and `/simulate`.
//!
//! The ports, the agent loop, the use cases and the tool registry live in
//! `application-core` and are re-exported here under their old paths, so
//! `application::ports::...` and `application::usecases::run_turn::...`
//! keep working for the binary and the Lua adapter.

pub use application_core::{
    cost, csv, decide, keymap, llm_retry, ports, privacy, providers, rtk, shell, theme, web, window,
};

pub mod flows;
// The same fakes `application-core` mounts (see there): written against
// `crate::ports`, which resolves to the re-exported core ports here.
#[cfg(test)]
#[path = "../../app-testing/src/fakes/mod.rs"]
pub mod testing;
pub mod tools;
pub mod usecases;
