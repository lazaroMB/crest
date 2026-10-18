//! Functions of the `app` table that plugins use to extend the agent:
//! `app.tools.register` ([`tools`]) and `app.on` ([`hooks`]).
//!
//! The rest of the `app` table (options, keymaps, commands) lives in the
//! config loader; these modules hold the parts the agent loop calls into.

pub(crate) mod tools;
