//! Functions of the `app` table that plugins use to extend the agent:
//! `app.tools.register` ([`tools`]), `app.on` ([`hooks`]) and the block
//! constructors of `app.ui` ([`ui`]).
//!
//! The rest of the `app` table (options, keymaps, commands) lives in the
//! config loader; these modules hold the parts the agent loop calls into.

pub(crate) mod hooks;
pub(crate) mod tools;
pub(crate) mod ui;
