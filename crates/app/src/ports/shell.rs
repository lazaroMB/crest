//! Shell port: running external commands inside the workspace.
//!
//! Commands are spawned directly (never through `sh -c`) with the workspace
//! root as working directory, so arguments are never reinterpreted by a
//! shell.
//!
//! Cancellation is done by dropping the returned future; adapters must kill
//! the child process (and its process group when possible) on drop, for
//! example with `kill_on_drop(true)`.

use std::time::Duration;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Default of [`RunCommandRequest::timeout_s`].
pub const DEFAULT_COMMAND_TIMEOUT_S: u64 = 120;

fn default_timeout_s() -> u64 {
    DEFAULT_COMMAND_TIMEOUT_S
}

/// Errors of [`ShellPort`].
///
/// Process failures are carried as rendered messages so the error stays
/// `Clone + PartialEq` (`std::io::Error` is neither).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ShellError {
    /// The command is malformed (e.g. empty, or contains a NUL byte).
    #[error("invalid command: {0}")]
    InvalidCommand(String),
    /// The program was not found on `PATH`.
    #[error("command not found: {command}")]
    NotFound {
        /// The program that was looked up.
        command: String,
    },
    /// The process could not be started for another reason.
    #[error("could not start command: {0}")]
    Spawn(String),
    /// The timeout elapsed and the process could not be stopped or its
    /// output collected. A timeout that still yields partial output is
    /// reported as a successful [`CommandOutput`] with `timed_out` set.
    #[error("command timed out after {after:?}")]
    Timeout {
        /// The timeout that elapsed.
        after: Duration,
    },
    /// Reading the process output or waiting for it failed.
    #[error("command I/O error: {0}")]
    Io(String),
}

/// Input of the `run_command` tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RunCommandRequest {
    /// Program to run, looked up on `PATH`; no shell is involved, so pipes,
    /// redirections and globs are not interpreted.
    pub command: String,
    /// Arguments passed to the program as-is.
    #[serde(default)]
    pub args: Vec<String>,
    /// Seconds before the process is killed (default 120).
    #[serde(default = "default_timeout_s")]
    pub timeout_s: u64,
}

impl RunCommandRequest {
    /// A request for `command` with `args` and the default timeout.
    pub fn new<I, S>(command: impl Into<String>, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            command: command.into(),
            args: args.into_iter().map(Into::into).collect(),
            timeout_s: DEFAULT_COMMAND_TIMEOUT_S,
        }
    }

    /// The timeout as a [`Duration`].
    pub fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_s)
    }

    /// The command line as shown in approval dialogs: the program and its
    /// arguments separated by spaces, quoting arguments that need it.
    pub fn display(&self) -> String {
        std::iter::once(&self.command)
            .chain(&self.args)
            .map(|part| quote(part))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

fn quote(part: &str) -> String {
    let plain = !part.is_empty()
        && part
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./=:,+@%".contains(c));
    if plain {
        part.to_owned()
    } else {
        format!("'{}'", part.replace('\'', r"'\''"))
    }
}

/// Result of a finished (or timed out) command.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CommandOutput {
    /// Exit code; `None` when the process was killed by a signal or by the
    /// timeout.
    pub status: Option<i32>,
    /// Captured standard output, decoded lossily as UTF-8.
    pub stdout: String,
    /// Captured standard error, decoded lossily as UTF-8.
    pub stderr: String,
    /// Whether the process was killed because the timeout elapsed.
    pub timed_out: bool,
    /// Whether `stdout` or `stderr` were cut to the adapter's size limit.
    pub truncated: bool,
}

impl CommandOutput {
    /// Whether the command exited with status 0.
    pub fn success(&self) -> bool {
        self.status == Some(0)
    }
}

/// Runs external commands in the workspace.
#[async_trait]
pub trait ShellPort: Send + Sync {
    /// Runs `request.command` with its arguments in the workspace root,
    /// capturing stdout and stderr, and kills it after
    /// [`RunCommandRequest::timeout`] (returning the partial output with
    /// `timed_out` set). A non-zero exit is a successful result, not an
    /// error. Approval is the caller's job: this method does not ask.
    async fn run(&self, request: RunCommandRequest) -> Result<CommandOutput, ShellError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_takes_documented_defaults() {
        let req: RunCommandRequest = serde_json::from_str(r#"{"command":"ls"}"#).unwrap();
        assert_eq!(req, RunCommandRequest::new("ls", Vec::<String>::new()));
        assert_eq!(req.timeout(), Duration::from_secs(120));
    }

    #[test]
    fn display_quotes_arguments_that_need_it() {
        let req = RunCommandRequest::new("git", ["log", "--format=%h %s", "it's", ""]);
        assert_eq!(req.display(), r"git log '--format=%h %s' 'it'\''s' ''");
    }
}
