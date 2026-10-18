//! Running external tools (`rg`, `fzf`) without a shell.
//!
//! Every process is spawned directly with [`tokio::process::Command`], never
//! through `sh -c`, with `kill_on_drop(true)`: dropping a future returned by
//! this module kills the processes it started, which is how a search is
//! cancelled. Each run takes a timeout; when it elapses the processes are
//! killed and the output read so far is returned with `timed_out = true`.

use std::ffi::OsString;
use std::io;
use std::path::PathBuf;
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, Command};

/// A program to run: executable, arguments, working directory and extra
/// environment variables. No shell is involved, so arguments are passed
/// verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessSpec {
    /// Executable name (looked up in `PATH`) or path.
    pub program: OsString,
    /// Arguments, passed as-is.
    pub args: Vec<OsString>,
    /// Working directory of the process.
    pub cwd: PathBuf,
    /// Variables added to (or overriding) the inherited environment.
    pub env: Vec<(OsString, OsString)>,
}

impl ProcessSpec {
    /// A spec for `program` with no arguments, running in `cwd`.
    pub fn new(program: impl Into<OsString>, cwd: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            cwd: cwd.into(),
            env: Vec::new(),
        }
    }

    /// Appends one argument.
    #[must_use]
    pub fn arg(mut self, arg: impl Into<OsString>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// Appends several arguments.
    #[must_use]
    pub fn args<I, A>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = A>,
        A: Into<OsString>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    /// Sets an environment variable for the process.
    #[must_use]
    pub fn env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    fn program_name(&self) -> String {
        self.program.to_string_lossy().into_owned()
    }

    fn command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command
            .args(&self.args)
            .current_dir(&self.cwd)
            .envs(self.env.iter().map(|(k, v)| (k, v)))
            .kill_on_drop(true);
        command
    }

    async fn spawn(&self, mut command: Command) -> Result<Child, ProcessError> {
        match command.spawn() {
            Ok(child) => Ok(child),
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                // ENOENT is also what a missing working directory produces.
                if tokio::fs::metadata(&self.cwd).await.is_err() {
                    Err(ProcessError::Spawn(format!(
                        "working directory {} does not exist",
                        self.cwd.display()
                    )))
                } else {
                    Err(ProcessError::NotFound {
                        program: self.program_name(),
                    })
                }
            }
            Err(err) => Err(ProcessError::Spawn(format!(
                "{}: {err}",
                self.program_name()
            ))),
        }
    }
}

/// What a finished (or killed) process produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessOutput {
    /// Exit status; after a timeout, the status of the killed process.
    pub status: ExitStatus,
    /// Everything read from stdout (partial when `timed_out`).
    pub stdout: Vec<u8>,
    /// Everything read from stderr (partial when `timed_out`).
    pub stderr: Vec<u8>,
    /// Whether the timeout elapsed and the process was killed.
    pub timed_out: bool,
}

/// Failures to run a process. A non-zero exit status is not an error: it is
/// reported in [`ProcessOutput::status`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProcessError {
    /// The executable does not exist or is not in `PATH`.
    #[error("program `{program}` not found")]
    NotFound {
        /// The program that was looked up.
        program: String,
    },
    /// The process could not be started for another reason.
    #[error("could not start process: {0}")]
    Spawn(String),
    /// Reading from or writing to the process failed.
    #[error("process I/O error: {0}")]
    Io(String),
}

impl From<io::Error> for ProcessError {
    fn from(err: io::Error) -> Self {
        Self::Io(err.to_string())
    }
}

/// Runs `spec` to completion, feeding it `stdin` (or an empty stdin when
/// `None`) and collecting stdout and stderr.
///
/// When `timeout` elapses the process is killed and the output read so far
/// is returned with `timed_out = true`.
pub async fn run(
    spec: &ProcessSpec,
    stdin: Option<Vec<u8>>,
    timeout: Duration,
) -> Result<ProcessOutput, ProcessError> {
    let mut command = spec.command();
    command
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = spec.spawn(command).await?;
    let child_stdin = child.stdin.take();
    let mut out = child.stdout.take();
    let mut err = child.stderr.take();

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let work = async {
        let feed = async {
            if let (Some(mut pipe), Some(data)) = (child_stdin, stdin) {
                // The process may exit without reading all of its input.
                match pipe.write_all(&data).await {
                    Err(e) if e.kind() != io::ErrorKind::BrokenPipe => return Err(e),
                    _ => {}
                }
                // Dropping the pipe closes it, signalling end of input.
            }
            Ok(())
        };
        let (fed, read_out, read_err) = tokio::join!(
            feed,
            read_all(out.as_mut(), &mut stdout),
            read_all(err.as_mut(), &mut stderr)
        );
        fed?;
        read_out?;
        read_err?;
        child.wait().await
    };
    let result = tokio::time::timeout(timeout, work).await;
    let (status, timed_out) = match result {
        Ok(status) => (status?, false),
        Err(_) => (kill(&mut child).await?, true),
    };
    Ok(ProcessOutput {
        status,
        stdout,
        stderr,
        timed_out,
    })
}

/// Appends everything `reader` yields to `buf`. Reads chunk by chunk so a
/// cancelled call (on timeout) leaves the bytes read so far in `buf`.
async fn read_all<R: AsyncRead + Unpin>(
    reader: Option<&mut R>,
    buf: &mut Vec<u8>,
) -> io::Result<()> {
    let Some(reader) = reader else {
        return Ok(());
    };
    let mut chunk = [0u8; 8192];
    loop {
        let n = reader.read(&mut chunk).await?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}

/// Kills `child` (if still running) and reaps it.
async fn kill(child: &mut Child) -> io::Result<ExitStatus> {
    if let Err(err) = child.start_kill() {
        tracing::debug!(%err, "could not kill child process");
    }
    child.wait().await
}

#[cfg(test)]
mod tests {
    use super::*;

    const LONG: Duration = Duration::from_secs(10);

    fn spec(program: &str) -> ProcessSpec {
        ProcessSpec::new(program, std::env::temp_dir())
    }

    #[tokio::test]
    async fn run_collects_stdout_and_status() {
        let out = run(&spec("printf").arg("hello\\n"), None, LONG)
            .await
            .unwrap();
        assert_eq!(out.stdout, b"hello\n");
        assert!(out.status.success());
        assert!(!out.timed_out);
    }

    #[tokio::test]
    async fn run_feeds_stdin() {
        let out = run(&spec("cat"), Some(b"a\nb\n".to_vec()), LONG)
            .await
            .unwrap();
        assert_eq!(out.stdout, b"a\nb\n");
    }

    #[tokio::test]
    async fn run_uses_cwd_and_env() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("f.txt"), "content").unwrap();
        let out = run(
            &ProcessSpec::new("cat", dir.path()).arg("f.txt"),
            None,
            LONG,
        )
        .await
        .unwrap();
        assert_eq!(out.stdout, b"content");

        let out = run(
            &spec("printenv")
                .arg("CREST_TEST_VAR")
                .env("CREST_TEST_VAR", "x"),
            None,
            LONG,
        )
        .await
        .unwrap();
        assert_eq!(out.stdout, b"x\n");
    }

    #[tokio::test]
    async fn run_reports_failure_status_and_stderr() {
        let out = run(&spec("cat").arg("/nonexistent/crest"), None, LONG)
            .await
            .unwrap();
        assert!(!out.status.success());
        assert!(!out.stderr.is_empty());
    }

    #[tokio::test]
    async fn missing_program_is_not_found() {
        let err = run(&spec("crest-no-such-program"), None, LONG)
            .await
            .unwrap_err();
        assert_eq!(
            err,
            ProcessError::NotFound {
                program: "crest-no-such-program".into()
            }
        );
    }

    #[tokio::test]
    async fn missing_cwd_is_a_spawn_error() {
        let spec = ProcessSpec::new("cat", "/nonexistent/crest-dir");
        let err = run(&spec, None, LONG).await.unwrap_err();
        assert!(matches!(err, ProcessError::Spawn(_)), "{err:?}");
    }
}
