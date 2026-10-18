//! ShellPort adapter over tokio::process.
//!
//! [`LocalShell`] runs `run_command` requests as child processes of crest:
//! the program is spawned directly (never through `sh -c`, so arguments are
//! passed verbatim and nothing is expanded), with the workspace root as
//! working directory and an empty stdin. Stdout and stderr are captured up
//! to [`MAX_CAPTURE_BYTES`] each; the rest is discarded and the output is
//! marked `truncated`.

mod capture;

use std::process::Stdio;

use application::ports::shell::{CommandOutput, RunCommandRequest, ShellError, ShellPort};
use async_trait::async_trait;
use domain::workspace::WorkspaceRoot;
use tokio::process::Command;

use crate::capture::Capture;
pub use crate::capture::MAX_CAPTURE_BYTES;

/// [`ShellPort`] backed by [`tokio::process`], running every command in the
/// workspace root.
#[derive(Debug, Clone)]
pub struct LocalShell {
    root: WorkspaceRoot,
}

impl LocalShell {
    /// A shell whose commands run in `root` (already canonicalized when the
    /// workspace was opened).
    pub fn new(root: WorkspaceRoot) -> Self {
        Self { root }
    }

    /// The working directory of every command.
    pub fn root(&self) -> &WorkspaceRoot {
        &self.root
    }

    fn command(&self, request: &RunCommandRequest) -> Command {
        let mut command = Command::new(&request.command);
        command
            .args(&request.args)
            .current_dir(self.root.as_path())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        command
    }
}

#[async_trait]
impl ShellPort for LocalShell {
    async fn run(&self, request: RunCommandRequest) -> Result<CommandOutput, ShellError> {
        let mut child = self
            .command(&request)
            .spawn()
            .map_err(|err| ShellError::Spawn(format!("{}: {err}", request.command)))?;
        let mut stdout = Capture::new(MAX_CAPTURE_BYTES);
        let mut stderr = Capture::new(MAX_CAPTURE_BYTES);
        let (read_out, read_err) = tokio::join!(
            stdout.read_from(child.stdout.take()),
            stderr.read_from(child.stderr.take())
        );
        let io = |err: std::io::Error| ShellError::Io(err.to_string());
        read_out.map_err(io)?;
        read_err.map_err(io)?;
        let status = child.wait().await.map_err(io)?;
        Ok(CommandOutput {
            status: status.code(),
            truncated: stdout.truncated() || stderr.truncated(),
            stdout: stdout.text(),
            stderr: stderr.text(),
            timed_out: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shell(dir: &tempfile::TempDir) -> LocalShell {
        let root = dir.path().canonicalize().unwrap();
        LocalShell::new(WorkspaceRoot::new(root).unwrap())
    }

    #[tokio::test]
    async fn arguments_are_not_interpreted_by_a_shell() {
        let dir = tempfile::tempdir().unwrap();
        let out = shell(&dir)
            .run(RunCommandRequest::new("echo", ["$HOME;ls", "*", "a b"]))
            .await
            .unwrap();
        assert_eq!(out.stdout, "$HOME;ls * a b\n");
        assert!(out.success());
        assert!(!out.timed_out);
        assert!(!out.truncated);
    }

    #[tokio::test]
    async fn runs_in_the_workspace_root() {
        let dir = tempfile::tempdir().unwrap();
        let shell = shell(&dir);
        let out = shell
            .run(RunCommandRequest::new("pwd", Vec::<String>::new()))
            .await
            .unwrap();
        assert_eq!(
            out.stdout.trim_end(),
            shell.root().as_path().to_str().unwrap()
        );
    }

    #[tokio::test]
    async fn non_zero_exit_is_a_result_with_stderr() {
        let dir = tempfile::tempdir().unwrap();
        let out = shell(&dir)
            .run(RunCommandRequest::new("ls", ["crest-missing-file"]))
            .await
            .unwrap();
        assert!(!out.success());
        assert!(out.status.is_some_and(|code| code != 0));
        assert!(out.stderr.contains("crest-missing-file"), "{}", out.stderr);
    }

    #[tokio::test]
    async fn huge_output_is_truncated() {
        let dir = tempfile::tempdir().unwrap();
        let out = shell(&dir)
            .run(RunCommandRequest::new(
                "head",
                ["-c", "5000000", "/dev/zero"],
            ))
            .await
            .unwrap();
        assert!(out.success());
        assert!(out.truncated);
        assert_eq!(out.stdout.len(), MAX_CAPTURE_BYTES);
    }
}
