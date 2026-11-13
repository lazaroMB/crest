//! Tools the model can call, their registry and result budgets.
//!
//! The registry, the budgets, `ask_user` and the web tools live in
//! `application-core` (the agent loop uses them) and are re-exported here.

pub use application_core::tools::{budget, form, registry, web};

pub mod components;
pub mod compute;
pub mod files;
pub mod formats;
pub mod ghs;
pub mod mcp;
pub mod plugin;
pub mod simulate;
pub mod skill;
pub mod task;
