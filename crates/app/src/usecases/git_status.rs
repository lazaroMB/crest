//! The git state of the workspace for the status bar: runs
//! `git status --porcelain=v2 --branch` through the [`ShellPort`] (a read
//! the permission policy always allows) and parses it.

use domain::git::GitStatus;

use crate::ports::shell::{RunCommandRequest, ShellPort};

/// Seconds `git status` may take before the status bar gives up on it.
const TIMEOUT_S: u64 = 10;

/// The git state of the workspace, or `None` when it is not a repository
/// or git is unavailable.
pub async fn git_status(shell: &dyn ShellPort) -> Option<GitStatus> {
    let mut request = RunCommandRequest::new(
        "git",
        [
            "status",
            "--porcelain=v2",
            "--branch",
            "--untracked-files=normal",
        ],
    );
    request.timeout_s = TIMEOUT_S;
    let output = shell.run(request).await.ok()?;
    (output.success() && !output.timed_out).then(|| GitStatus::parse(&output.stdout))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::shell::CommandOutput;
    use crate::testing::fs::ScriptedShell;

    fn exit(code: i32, stdout: &str) -> CommandOutput {
        CommandOutput {
            status: Some(code),
            stdout: stdout.into(),
            ..CommandOutput::default()
        }
    }

    #[tokio::test]
    async fn reads_the_status_of_the_repository() {
        let args = [
            "status",
            "--porcelain=v2",
            "--branch",
            "--untracked-files=normal",
        ];
        let shell = ScriptedShell::new().on(
            "git",
            args,
            exit(0, "# branch.oid abc\n# branch.head main\n? x\n"),
        );
        let status = git_status(&shell).await.unwrap();
        assert_eq!(status.to_string(), "main ?1");

        let outside = ScriptedShell::new().on("git", args, exit(128, ""));
        assert_eq!(git_status(&outside).await, None);
    }
}
