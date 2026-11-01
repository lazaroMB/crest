//! The tool registry and result budgets the agent loop runs on, and the
//! tools the loop itself knows by name (`ask_user`, the web tools).
//!
//! The rest of the tools (files, compute, formats, ...) live in
//! `application`, whose `tools` module re-exports these.

pub mod budget;
pub mod form;
pub mod registry;
pub mod web;
