//! ShellPort adapter over tokio::process.
//!
//! [`LocalShell`] runs `run_command` requests as child processes of crest:
//! the program is spawned directly (never through `sh -c`, so arguments are
//! passed verbatim and nothing is expanded), with the workspace root as
//! working directory and an empty stdin. Stdout and stderr are captured up
//! to [`MAX_CAPTURE_BYTES`] each; the rest is discarded and the output is
//! marked `truncated`.
//!
//! Every command runs in its own process group. When the leader exits, or
//! the timeout elapses, or the `run` future is dropped, the whole group is
//! killed (see [`group`](crate::group) for how, without `unsafe`), so
//! background children never outlive the command nor keep its pipes open.
//! On Windows the tree is killed instead, and only while the command is
//! alive; a program found on `PATH` through `PATHEXT` (`npm.cmd`) runs as
//! well as an `.exe`, and no console window opens. On timeout the output read so far is returned with
//! `timed_out` set and no exit status.
//!
//! Commands inherit crest's environment minus [`SCRUBBED_ENV_VARS`]: the
//! credentials crest itself uses (and those of other LLM providers), so a
//! command the model asks for cannot read or exfiltrate the API key.

mod capture;
mod group;
mod rtk;

use std::ffi::OsString;
use std::io;
use std::pin::pin;
use std::process::Stdio;
use std::time::Duration;

use application::ports::shell::{
    CommandOutput, RunCommandRequest, ShellError, ShellPort, ShellSpec,
};
use async_trait::async_trait;
use domain::workspace::WorkspaceRoot;
use tokio::process::{Child, Command};

use crate::capture::Capture;
pub use crate::capture::MAX_CAPTURE_BYTES;
use crate::group::ProcessGroup;
pub use crate::rtk::RtkRewriter;

/// Environment variables removed from the inherited environment: API keys
/// and tokens of LLM providers, starting with the one crest reads, and the
/// key of the Brave search backend.
pub const SCRUBBED_ENV_VARS: [&str; 7] = [
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "OPENAI_API_KEY",
    "GEMINI_API_KEY",
    "GOOGLE_API_KEY",
    "MISTRAL_API_KEY",
    "BRAVE_API_KEY",
];

/// How long the pipes may stay open after the process group was killed on
/// exit. Only a process that left the group (`setsid`) can hold them past
/// that; whatever it writes later is not captured.
const DRAIN_GRACE: Duration = Duration::from_millis(500);

/// [`ShellPort`] backed by [`tokio::process`], running every command in the
/// workspace root.
#[derive(Debug, Clone)]
pub struct LocalShell {
    root: WorkspaceRoot,
    /// The shell for whole command lines ([`ShellPort::line_shell`]).
    line_shell: ShellSpec,
    /// Environment to inherit instead of crest's own (tests only).
    inherited_env: Option<Vec<(OsString, OsString)>>,
}

impl LocalShell {
    /// A shell whose commands run in `root` (already canonicalized when the
    /// workspace was opened).
    pub fn new(root: WorkspaceRoot) -> Self {
        Self {
            root,
            line_shell: ShellSpec::default(),
            inherited_env: None,
        }
    }

    /// Uses `shell` for whole command lines (`app.opt.shell`, or `pwsh`
    /// when installed on Windows).
    #[must_use]
    pub fn with_line_shell(mut self, shell: ShellSpec) -> Self {
        self.line_shell = shell;
        self
    }

    /// Uses `env` as the inherited environment, so tests can check the
    /// scrubbing without touching the process environment.
    #[cfg(test)]
    fn with_inherited_env<K, V>(mut self, env: impl IntoIterator<Item = (K, V)>) -> Self
    where
        K: Into<OsString>,
        V: Into<OsString>,
    {
        self.inherited_env = Some(env.into_iter().map(|(k, v)| (k.into(), v.into())).collect());
        self
    }

    fn environment(&self) -> Vec<(OsString, OsString)> {
        let inherited = match &self.inherited_env {
            Some(env) => env.clone(),
            None => std::env::vars_os().collect(),
        };
        inherited
            .into_iter()
            .filter(|(key, _)| {
                !SCRUBBED_ENV_VARS
                    .iter()
                    .any(|scrubbed| platform::process::same_env_name(key, scrubbed))
            })
            .collect()
    }

    /// The working directory of every command.
    pub fn root(&self) -> &WorkspaceRoot {
        &self.root
    }

    fn command(&self, request: &RunCommandRequest) -> Command {
        let mut command = Command::new(program(&request.command));
        command
            .args(&request.args)
            .current_dir(self.root.as_path())
            .env_clear()
            .envs(self.environment())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        platform::process::own_group(command.as_std_mut());
        command
    }

    async fn spawn(&self, request: &RunCommandRequest) -> Result<Child, ShellError> {
        match self.command(request).spawn() {
            Ok(child) => Ok(child),
            // ENOENT is also what a missing working directory produces.
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                if tokio::fs::metadata(self.root.as_path()).await.is_err() {
                    Err(ShellError::Spawn(format!(
                        "workspace root {} does not exist",
                        self.root.as_path().display()
                    )))
                } else {
                    Err(ShellError::NotFound {
                        command: request.command.clone(),
                    })
                }
            }
            Err(err) => Err(ShellError::Spawn(format!("{}: {err}", request.command))),
        }
    }
}

/// The program to spawn for `command`. Windows only starts a bare name
/// that is an `.exe`, so there a name without a directory is looked up on
/// `PATH` with the `PATHEXT` extensions first (`npm` is `npm.cmd`); a
/// miss is left to the spawn to report.
fn program(command: &str) -> std::path::PathBuf {
    let bare = !command.contains(['/', '\\']);
    if cfg!(windows)
        && bare
        && let Some(found) = platform::process::find_program(command, None)
    {
        return found;
    }
    command.into()
}

/// Rejects requests no process could be started for.
fn validate(request: &RunCommandRequest) -> Result<(), ShellError> {
    if request.command.trim().is_empty() {
        return Err(ShellError::InvalidCommand("the command is empty".into()));
    }
    let has_nul = std::iter::once(&request.command)
        .chain(&request.args)
        .any(|part| part.contains('\0'));
    if has_nul {
        return Err(ShellError::InvalidCommand(
            "the command or an argument contains a NUL byte".into(),
        ));
    }
    Ok(())
}

#[async_trait]
impl ShellPort for LocalShell {
    fn line_shell(&self) -> ShellSpec {
        self.line_shell.clone()
    }

    async fn run(&self, request: RunCommandRequest) -> Result<CommandOutput, ShellError> {
        validate(&request)?;
        let mut child = self.spawn(&request).await?;
        let mut group = ProcessGroup::of(&child);
        let out_pipe = child.stdout.take();
        let err_pipe = child.stderr.take();
        let mut stdout = Capture::new(MAX_CAPTURE_BYTES);
        let mut stderr = Capture::new(MAX_CAPTURE_BYTES);

        let work = async {
            let mut reads = pin!(async {
                let (read_out, read_err) =
                    tokio::join!(stdout.read_from(out_pipe), stderr.read_from(err_pipe));
                read_out.and(read_err)
            });
            let mut drained = false;
            let status = tokio::select! {
                status = child.wait() => status?,
                read = &mut reads => {
                    read?;
                    drained = true;
                    child.wait().await?
                }
            };
            // Leftover members of the group (backgrounded children) would
            // keep the pipes open: kill them and read what is left.
            group.kill();
            if !drained && let Ok(read) = tokio::time::timeout(DRAIN_GRACE, &mut reads).await {
                read?;
            }
            Ok::<_, io::Error>(status)
        };
        let io = |err: io::Error| ShellError::Io(err.to_string());
        let (status, timed_out) = match tokio::time::timeout(request.timeout(), work).await {
            Ok(status) => (status.map_err(io)?.code(), false),
            Err(_) => {
                group.kill();
                // Fallback in case the kill utility could not run; harmless
                // otherwise.
                if let Err(err) = child.start_kill() {
                    tracing::trace!(%err, "leader already gone");
                }
                child.wait().await.map_err(io)?;
                tracing::debug!(command = %request.display(), "command timed out");
                (None, true)
            }
        };
        Ok(CommandOutput {
            status,
            truncated: stdout.truncated() || stderr.truncated(),
            stdout: stdout.text(),
            stderr: stderr.text(),
            timed_out,
        })
    }
}

// The tests drive POSIX tools (`sh`, `sleep`, `env`, `pwd`).
#[cfg(test)]
#[cfg(unix)]
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

    fn with_timeout(command: &str, args: &[&str], timeout_s: u64) -> RunCommandRequest {
        RunCommandRequest {
            timeout_s,
            ..RunCommandRequest::new(command, args.iter().copied())
        }
    }

    #[tokio::test]
    async fn timeout_kills_the_process() {
        let dir = tempfile::tempdir().unwrap();
        let start = std::time::Instant::now();
        let out = shell(&dir)
            .run(with_timeout("sleep", &["30"], 1))
            .await
            .unwrap();
        assert!(out.timed_out);
        assert_eq!(out.status, None);
        assert!(start.elapsed() < Duration::from_secs(3));
    }

    #[tokio::test]
    async fn timeout_keeps_partial_output() {
        let dir = tempfile::tempdir().unwrap();
        let out = shell(&dir)
            .run(with_timeout(
                "sh",
                &["-c", "echo partial; exec sleep 30"],
                1,
            ))
            .await
            .unwrap();
        assert!(out.timed_out);
        assert_eq!(out.stdout, "partial\n");
    }

    #[tokio::test]
    async fn background_children_do_not_hold_the_command() {
        let dir = tempfile::tempdir().unwrap();
        let start = std::time::Instant::now();
        let out = shell(&dir)
            .run(with_timeout("sh", &["-c", "sleep 30 & echo started"], 20))
            .await
            .unwrap();
        assert!(!out.timed_out);
        assert!(out.success());
        assert_eq!(out.stdout, "started\n");
        assert!(start.elapsed() < Duration::from_secs(3));
    }

    /// Pid of the running process whose command line is exactly `args`.
    #[cfg(target_os = "linux")]
    async fn wait_for_pid(args: &[&str]) -> u32 {
        let wanted: Vec<u8> = args
            .iter()
            .flat_map(|a| [a.as_bytes(), b"\0"].concat())
            .collect();
        for _ in 0..250 {
            for entry in std::fs::read_dir("/proc").unwrap().flatten() {
                let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
                    continue;
                };
                if std::fs::read(entry.path().join("cmdline")).is_ok_and(|c| c == wanted) {
                    return pid;
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("process {args:?} never started");
    }

    /// Whether `pid` is gone or a zombie (killed, waiting to be reaped).
    #[cfg(target_os = "linux")]
    async fn wait_until_dead(pid: u32) -> bool {
        for _ in 0..250 {
            match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
                Err(_) => return true,
                Ok(stat) => {
                    let state = stat.rsplit_once(')').map(|(_, rest)| rest.trim_start());
                    if state.is_some_and(|s| s.starts_with('Z')) {
                        return true;
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        false
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn timeout_kills_the_whole_process_group() {
        let dir = tempfile::tempdir().unwrap();
        let shell = shell(&dir);
        let request = with_timeout("sh", &["-c", "sleep 30.4251 & wait"], 1);
        let (out, pid) = tokio::join!(shell.run(request), wait_for_pid(&["sleep", "30.4251"]));
        assert!(out.unwrap().timed_out);
        assert!(wait_until_dead(pid).await, "grandchild {pid} survived");
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn dropping_the_future_kills_the_process_group() {
        let dir = tempfile::tempdir().unwrap();
        let shell = shell(&dir);
        let request = with_timeout("sh", &["-c", "sleep 30.4252 & wait"], 60);
        let mut fut = shell.run(request);
        let pid = tokio::select! {
            _ = &mut fut => panic!("command finished early"),
            pid = wait_for_pid(&["sleep", "30.4252"]) => pid,
        };
        drop(fut);
        assert!(
            wait_until_dead(pid).await,
            "grandchild {pid} survived the drop"
        );
    }

    #[tokio::test]
    async fn credentials_are_not_inherited() {
        let dir = tempfile::tempdir().unwrap();
        let shell = shell(&dir).with_inherited_env([
            ("ANTHROPIC_API_KEY", "placeholder-not-a-key"),
            ("OPENAI_API_KEY", "placeholder-not-a-key"),
            ("CREST_TEST_VAR", "kept"),
        ]);
        let out = shell
            .run(RunCommandRequest::new("env", Vec::<String>::new()))
            .await
            .unwrap();
        assert!(out.success());
        assert_eq!(out.stdout, "CREST_TEST_VAR=kept\n");
    }

    #[tokio::test]
    async fn the_real_environment_is_inherited() {
        let dir = tempfile::tempdir().unwrap();
        let out = shell(&dir)
            .run(RunCommandRequest::new("env", Vec::<String>::new()))
            .await
            .unwrap();
        assert!(out.stdout.lines().any(|line| line.starts_with("PATH=")));
        assert!(!out.stdout.contains("ANTHROPIC_API_KEY="));
    }

    #[tokio::test]
    async fn missing_program_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let err = shell(&dir)
            .run(RunCommandRequest::new("crest-no-such-program", ["x"]))
            .await
            .unwrap_err();
        assert_eq!(
            err,
            ShellError::NotFound {
                command: "crest-no-such-program".into()
            }
        );
    }

    #[tokio::test]
    async fn empty_command_is_invalid() {
        let dir = tempfile::tempdir().unwrap();
        for command in ["", "  "] {
            let err = shell(&dir)
                .run(RunCommandRequest::new(command, ["x"]))
                .await
                .unwrap_err();
            assert!(matches!(err, ShellError::InvalidCommand(_)), "{err:?}");
        }
    }

    #[tokio::test]
    async fn nul_bytes_are_invalid() {
        let dir = tempfile::tempdir().unwrap();
        let err = shell(&dir)
            .run(RunCommandRequest::new("echo", ["a\0b"]))
            .await
            .unwrap_err();
        assert!(matches!(err, ShellError::InvalidCommand(_)), "{err:?}");
    }

    #[tokio::test]
    async fn missing_root_is_a_spawn_error() {
        let root = WorkspaceRoot::new("/nonexistent/crest-root").unwrap();
        let err = LocalShell::new(root)
            .run(RunCommandRequest::new("echo", ["x"]))
            .await
            .unwrap_err();
        assert!(matches!(err, ShellError::Spawn(_)), "{err:?}");
    }
}

#[cfg(test)]
#[cfg(windows)]
mod windows_tests {
    use std::time::Instant;

    use super::*;

    fn shell(dir: &tempfile::TempDir) -> LocalShell {
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let root = root
            .to_string_lossy()
            .trim_start_matches(r"\\?\")
            .to_owned();
        LocalShell::new(WorkspaceRoot::new(root).unwrap())
    }

    #[tokio::test]
    async fn runs_a_program_found_on_path() {
        let dir = tempfile::tempdir().unwrap();
        let out = shell(&dir)
            .run(RunCommandRequest::new("cmd", ["/C", "echo", "hello"]))
            .await
            .unwrap();
        assert!(out.success(), "{out:?}");
        assert_eq!(out.stdout.trim(), "hello");
    }

    #[tokio::test]
    async fn timeout_kills_the_tree() {
        let dir = tempfile::tempdir().unwrap();
        let started = Instant::now();
        let request = RunCommandRequest {
            timeout_s: 1,
            ..RunCommandRequest::new("cmd", ["/C", "ping -n 30 127.0.0.1 > NUL"])
        };
        let out = shell(&dir).run(request).await.unwrap();
        assert!(out.timed_out);
        assert!(started.elapsed() < Duration::from_secs(15));
    }

    #[tokio::test]
    async fn missing_program_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let err = shell(&dir)
            .run(RunCommandRequest::new(
                "crest-no-such-program",
                Vec::<String>::new(),
            ))
            .await
            .unwrap_err();
        assert!(matches!(err, ShellError::NotFound { .. }), "{err:?}");
    }

    #[test]
    fn credentials_are_scrubbed_whatever_the_case() {
        let dir = tempfile::tempdir().unwrap();
        let env = shell(&dir)
            .with_inherited_env([("anthropic_api_key", "k"), ("Path", r"C:\bin")])
            .environment();
        assert_eq!(env, [("Path".into(), r"C:\bin".into())]);
    }
}
