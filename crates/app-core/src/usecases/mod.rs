//! Use cases: the agent loop and the flows built on it. `simulate`, which
//! builds on the research flow, lives in `application` (its `usecases`
//! module re-exports these and adds it).

pub mod approval;
pub mod cancel;
pub mod commands;
pub mod compact;
pub mod compute;
pub mod evidence;
pub mod git_status;
pub mod login;
pub mod model3d;
pub mod open_file;
pub mod permissions;
pub mod project_picker;
pub mod resume;
pub mod routing;
pub mod run_turn;
pub mod shell_command;
pub mod sidebar;
pub mod user_blocks;
pub mod web_sources;
