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

/// How long the first process of a [`pipe`] may keep running after the
/// second one finished before it is killed.
const PIPE_GRACE: Duration = Duration::from_millis(500);

/// Result of a [`pipe`]: the second process's output plus the status and
/// stderr of the first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PipeOutput {
    /// Output of the second process. `timed_out` covers the whole pipe.
    pub output: ProcessOutput,
    /// Exit status of the first process. It may have been killed (or died
    /// of `SIGPIPE`) because the second one stopped reading early.
    pub first_status: ExitStatus,
    /// Everything the first process wrote to stderr.
    pub first_stderr: Vec<u8>,
}

/// Runs `first | second` without a shell: the stdout of `first` is
/// connected directly to the stdin of `second`.
///
/// If `second` finishes before `first` (for example because it only takes
/// the first lines), `first` gets `EPIPE`/`SIGPIPE` on its next write; if it
/// is still running after a short grace period it is killed, so it never
/// lingers. On timeout both processes are killed and the partial output is
/// returned with `timed_out = true`.
pub async fn pipe(
    first: &ProcessSpec,
    second: &ProcessSpec,
    timeout: Duration,
) -> Result<PipeOutput, ProcessError> {
    let mut command = first.command();
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut first_child = first.spawn(command).await?;
    let link: Stdio = match first_child.stdout.take() {
        Some(stdout) => stdout.try_into()?,
        None => Stdio::null(),
    };
    let mut command = second.command();
    command
        .stdin(link)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // `spawn` consumes `command`, which held our copy of the pipe's read
    // end: once the second process exits, the first one sees EPIPE.
    let mut second_child = match second.spawn(command).await {
        Ok(child) => child,
        Err(err) => {
            kill(&mut first_child).await?;
            return Err(err);
        }
    };
    let mut first_err = first_child.stderr.take();
    let mut out = second_child.stdout.take();
    let mut err = second_child.stderr.take();

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut first_stderr = Vec::new();
    let work = async {
        let second_then_first = async {
            let (read_out, read_err) = tokio::join!(
                read_all(out.as_mut(), &mut stdout),
                read_all(err.as_mut(), &mut stderr)
            );
            read_out?;
            read_err?;
            let second_status = second_child.wait().await?;
            let first_status = match tokio::time::timeout(PIPE_GRACE, first_child.wait()).await {
                Ok(status) => status?,
                Err(_) => kill(&mut first_child).await?,
            };
            Ok::<_, io::Error>((second_status, first_status))
        };
        let (statuses, read_first_err) = tokio::join!(
            second_then_first,
            read_all(first_err.as_mut(), &mut first_stderr)
        );
        read_first_err?;
        statuses
    };
    let result = tokio::time::timeout(timeout, work).await;
    let (second_status, first_status, timed_out) = match result {
        Ok(statuses) => {
            let (second_status, first_status) = statuses?;
            (second_status, first_status, false)
        }
        Err(_) => {
            let first_status = kill(&mut first_child).await?;
            let second_status = kill(&mut second_child).await?;
            (second_status, first_status, true)
        }
    };
    Ok(PipeOutput {
        output: ProcessOutput {
            status: second_status,
            stdout,
            stderr,
            timed_out,
        },
        first_status,
        first_stderr,
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
    async fn timeout_kills_the_process() {
        let start = std::time::Instant::now();
        let out = run(&spec("sleep").arg("5"), None, Duration::from_millis(200))
            .await
            .unwrap();
        assert!(out.timed_out);
        assert!(!out.status.success());
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[tokio::test]
    async fn timeout_keeps_partial_output() {
        // `tail -f` prints the file and then waits for more forever.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("log"), "partial\n").unwrap();
        let spec = ProcessSpec::new("tail", dir.path()).args(["-f", "log"]);
        let out = run(&spec, None, Duration::from_millis(300)).await.unwrap();
        assert!(out.timed_out);
        assert_eq!(out.stdout, b"partial\n");
    }

    #[tokio::test]
    async fn pipe_connects_stdout_to_stdin() {
        let out = pipe(&spec("printf").arg("b\\na\\n"), &spec("sort"), LONG)
            .await
            .unwrap();
        assert_eq!(out.output.stdout, b"a\nb\n");
        assert!(out.output.status.success());
        assert!(out.first_status.success());
        assert!(!out.output.timed_out);
    }

    #[tokio::test]
    async fn pipe_stops_an_endless_first_process() {
        let start = std::time::Instant::now();
        let out = pipe(&spec("yes"), &spec("head").args(["-n", "3"]), LONG)
            .await
            .unwrap();
        assert_eq!(out.output.stdout, b"y\ny\ny\n");
        assert!(out.output.status.success());
        assert!(!out.output.timed_out);
        // `yes` dies of SIGPIPE (or is killed): it must not keep running.
        assert!(!out.first_status.success());
        assert!(start.elapsed() < Duration::from_secs(5));
    }

    #[tokio::test]
    async fn pipe_collects_stderr_of_both() {
        let out = pipe(
            &spec("cat").arg("/nonexistent/crest-a"),
            &spec("cat").args(["-", "/nonexistent/crest-b"]),
            LONG,
        )
        .await
        .unwrap();
        assert!(String::from_utf8_lossy(&out.first_stderr).contains("crest-a"));
        assert!(String::from_utf8_lossy(&out.output.stderr).contains("crest-b"));
        assert!(!out.first_status.success());
        assert!(!out.output.status.success());
    }

    #[tokio::test]
    async fn pipe_times_out() {
        let start = std::time::Instant::now();
        let out = pipe(
            &spec("sleep").arg("5"),
            &spec("cat"),
            Duration::from_millis(200),
        )
        .await
        .unwrap();
        assert!(out.output.timed_out);
        assert!(!out.first_status.success());
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[tokio::test]
    async fn pipe_with_missing_second_program_fails() {
        let err = pipe(&spec("yes"), &spec("crest-no-such-program"), LONG)
            .await
            .unwrap_err();
        assert!(matches!(err, ProcessError::NotFound { .. }), "{err:?}");
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
