//! The `/` commands the session runs itself. Their rows, with the
//! controller's, are the catalog of `application-core`
//! ([`CATALOG`](application_core::usecases::commands::CATALOG)), the source
//! of the help's Commands section and of the hint for an unknown command.

pub use application_core::usecases::commands::{CommandInfo, unknown_command_hint};

/// Commands that run a builtin action of another name: the help names
/// the action's keys after them.
pub const COMMAND_ACTIONS: &[(&str, &str)] = &[("permissions", "permissions.open")];
