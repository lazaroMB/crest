//! Functions of the `app` table that plugins use to extend the agent:
//! `app.tools.register` ([`tools`]), `app.on` ([`hooks`]), the block
//! constructors of `app.ui` ([`ui`]), the MCP servers of `app.mcp`
//! ([`mcp`]) and `app.keymap.set` / `app.commands.register` ([`keymap`]).
//!
//! The declarative rest of the `app` table (options, models) lives in the
//! config loader.

pub(crate) mod hooks;
pub(crate) mod keymap;
pub(crate) mod mcp;
pub(crate) mod tools;
pub(crate) mod ui;
