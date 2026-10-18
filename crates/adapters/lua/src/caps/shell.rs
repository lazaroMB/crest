//! `app.shell.run`: running the programs a plugin declared with `shell:`
//! capabilities.
//!
//! ```lua
//! local out = app.shell.run("jq", { "-r", ".name" }, {
//!   stdin = '{"name":"crest"}', cwd = "sub/dir", timeout = 10, env = { NO_COLOR = "1" },
//! })
//! -- out = { status = 0, stdout = "crest\n", stderr = "", timed_out = false, truncated = false }
//! ```
//!
//! There is no shell: the program, which must be the name of a granted
//! `shell:` capability, is looked up in the absolute directories of `PATH`
//! and run with the arguments as given. It runs in the workspace root (or
//! `cwd`, which must stay inside it), in its own process group, which is
//! killed when the timeout (default [`DEFAULT_TIMEOUT_S`] seconds) elapses.
//! It inherits crest's environment minus the LLM credentials and every
//! `CREST_PLUGIN_*` secret. Stdout and stderr keep at most
//! [`MAX_CAPTURE_BYTES`] each.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use application::ports::plugin::Capability;
use mlua::{Lua, Table, Value};
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWriteExt as _};
use tokio::process::Command;

use super::{Context, authorize, context};
use crate::config::api::Origin;

/// Default limit of one run.
pub(crate) const DEFAULT_TIMEOUT_S: f64 = 60.0;
/// Longest `timeout` accepted.
const MAX_TIMEOUT_S: f64 = 3600.0;
/// Bytes kept from each of stdout and stderr.
pub(crate) const MAX_CAPTURE_BYTES: usize = 1024 * 1024;
/// How long the pipes may stay open once the process group is gone.
const DRAIN_GRACE: Duration = Duration::from_millis(500);

/// Variables removed from the inherited environment: the credentials of
/// LLM providers, as `adapter-shell` does for `run_command`.
pub(crate) const SCRUBBED_ENV_VARS: [&str; 6] = [
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "OPENAI_API_KEY",
    "GEMINI_API_KEY",
    "GOOGLE_API_KEY",
    "MISTRAL_API_KEY",
];
/// Prefix of the plugin secrets kept in the environment, also removed.
pub(crate) const SECRET_ENV_PREFIX: &str = "CREST_PLUGIN_";

const API: &str = "app.shell.run";

fn error(message: impl std::fmt::Display) -> mlua::Error {
    mlua::Error::runtime(format!("{API}: {message}"))
}

/// A run as read from Lua.
#[derive(Debug)]
struct Run {
    program: PathBuf,
    args: Vec<String>,
    cwd: PathBuf,
    stdin: Option<Vec<u8>>,
    /// Inherited environment, already without the credentials.
    inherited: Vec<(OsString, OsString)>,
    env: Vec<(String, String)>,
    timeout: Duration,
}

/// What came back.
#[derive(Debug, Default)]
struct Output {
    status: Option<i32>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    timed_out: bool,
    truncated: bool,
}

/// Builds the `app.shell` table for one phase.
pub(crate) fn table(lua: &Lua, origin: &Arc<Origin>) -> mlua::Result<Table> {
    let shell = lua.create_table()?;
    let origin = Arc::clone(origin);
    shell.set(
        "run",
        lua.create_function(
            move |lua, (program, args, opts): (String, Option<Table>, Option<Table>)| {
                let wanted = Capability::Shell(program.clone());
                authorize(
                    lua,
                    &origin,
                    API,
                    Some((&format!("shell:{program}"), &|c: &Capability| *c == wanted)),
                )?;
                let ctx: Arc<Context> = context(lua)?;
                let run = read_run(&ctx, program, args, opts)?;
                let output = ctx.block_on(execute(run))??;
                to_lua(lua, output)
            },
        )?,
    )?;
    Ok(shell)
}

fn read_run(
    ctx: &Context,
    program: String,
    args: Option<Table>,
    opts: Option<Table>,
) -> mlua::Result<Run> {
    let root = ctx
        .workspace
        .as_deref()
        .ok_or_else(|| error("there is no workspace to run commands in"))?;
    let program = resolve(&program, ctx.var("PATH"))
        .ok_or_else(|| error(format!("`{program}` was not found on PATH")))?;
    let mut list = Vec::new();
    if let Some(args) = args {
        for arg in args.sequence_values::<Value>() {
            list.push(match arg? {
                Value::String(s) => s.to_str()?.to_owned(),
                Value::Integer(n) => n.to_string(),
                Value::Number(n) => n.to_string(),
                other => {
                    return Err(error(format!(
                        "arguments must be strings or numbers, got {}",
                        other.type_name()
                    )));
                }
            });
        }
    }
    if list.iter().any(|a| a.contains('\0')) {
        return Err(error("an argument contains a NUL byte"));
    }
    let opts = match opts {
        Some(opts) => opts,
        None => {
            return Ok(Run {
                inherited: environment(ctx.vars().into_iter()),
                ..Run::new(program, list, root.to_path_buf())
            });
        }
    };
    let cwd = match opts.get::<Option<String>>("cwd")? {
        Some(dir) => inside(root, &dir)?,
        None => root.to_path_buf(),
    };
    let stdin = match opts.get::<Value>("stdin")? {
        Value::Nil => None,
        Value::String(s) => Some(s.as_bytes().to_vec()),
        other => {
            return Err(error(format!(
                "`stdin` must be a string, got {}",
                other.type_name()
            )));
        }
    };
    let mut env = Vec::new();
    if let Some(table) = opts.get::<Option<Table>>("env")? {
        for pair in table.pairs::<String, String>() {
            let (key, value) = pair?;
            if key.is_empty() || key.contains(['=', '\0']) || value.contains('\0') {
                return Err(error(format!("invalid environment variable `{key}`")));
            }
            env.push((key, value));
        }
    }
    let timeout = match opts.get::<Option<f64>>("timeout")? {
        None => DEFAULT_TIMEOUT_S,
        Some(t) if t > 0.0 && t <= MAX_TIMEOUT_S => t,
        Some(t) => {
            return Err(error(format!(
                "`timeout` must be between 0 and {MAX_TIMEOUT_S} seconds, got {t}"
            )));
        }
    };
    Ok(Run {
        inherited: environment(ctx.vars().into_iter()),
        cwd,
        stdin,
        env,
        timeout: Duration::from_secs_f64(timeout),
        ..Run::new(program, list, root.to_path_buf())
    })
}

impl Run {
    fn new(program: PathBuf, args: Vec<String>, cwd: PathBuf) -> Self {
        Self {
            program,
            args,
            cwd,
            stdin: None,
            inherited: Vec::new(),
            env: Vec::new(),
            timeout: Duration::from_secs_f64(DEFAULT_TIMEOUT_S),
        }
    }
}

/// `dir` (relative to `root`, or absolute) if it is an existing directory
/// inside `root` once links are resolved.
fn inside(root: &Path, dir: &str) -> mlua::Result<PathBuf> {
    let root = root
        .canonicalize()
        .map_err(|e| error(format!("cannot open the workspace {}: {e}", root.display())))?;
    let wanted = root.join(dir);
    let real = wanted
        .canonicalize()
        .map_err(|e| error(format!("cannot use `{dir}` as cwd: {e}")))?;
    if !real.starts_with(&root) {
        return Err(error(format!(
            "cwd `{dir}` is outside the workspace {}",
            root.display()
        )));
    }
    if !real.is_dir() {
        return Err(error(format!("cwd `{dir}` is not a directory")));
    }
    Ok(real)
}

/// The executable `program` in the absolute directories of `path`; relative
/// entries (`.`, empty) are skipped, so a repository cannot shadow a
/// program with its own file.
fn resolve(program: &str, path: Option<OsString>) -> Option<PathBuf> {
    std::env::split_paths(&path?)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(program))
        .find(|candidate| is_executable(candidate))
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

/// crest's environment minus the credentials.
pub(crate) fn environment(
    inherited: impl Iterator<Item = (OsString, OsString)>,
) -> Vec<(OsString, OsString)> {
    inherited
        .filter(|(key, _)| {
            let key = key.to_string_lossy();
            !SCRUBBED_ENV_VARS.contains(&key.as_ref()) && !key.starts_with(SECRET_ENV_PREFIX)
        })
        .collect()
}

async fn execute(run: Run) -> mlua::Result<Output> {
    let mut command = Command::new(&run.program);
    command
        .args(&run.args)
        .current_dir(&run.cwd)
        .env_clear()
        .envs(run.inherited.iter().map(|(k, v)| (k, v)))
        .envs(run.env.iter().map(|(k, v)| (k, v)))
        .stdin(if run.stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    let shown = run.program.display().to_string();
    let mut child = command
        .spawn()
        .map_err(|e| error(format!("cannot start {shown}: {e}")))?;
    let mut group = Group(child.id());
    let stdin_pipe = child.stdin.take();
    let (out_pipe, err_pipe) = (child.stdout.take(), child.stderr.take());
    let mut stdout = Capture::default();
    let mut stderr = Capture::default();

    let work = async {
        let feed = async {
            if let (Some(mut pipe), Some(bytes)) = (stdin_pipe, run.stdin.as_deref()) {
                // A program that exits without reading stdin closes the pipe.
                let _ = pipe.write_all(bytes).await;
                drop(pipe);
            }
        };
        let mut reads = std::pin::pin!(async {
            let ((), out, err) = tokio::join!(feed, stdout.read(out_pipe), stderr.read(err_pipe));
            out.and(err)
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
        group.kill();
        if !drained && let Ok(read) = tokio::time::timeout(DRAIN_GRACE, &mut reads).await {
            read?;
        }
        Ok::<_, std::io::Error>(status)
    };
    let (status, timed_out) = match tokio::time::timeout(run.timeout, work).await {
        Ok(status) => (
            status.map_err(|e| error(format!("{shown}: {e}")))?.code(),
            false,
        ),
        Err(_) => {
            group.kill();
            let _ = child.start_kill();
            let _ = child.wait().await;
            (None, true)
        }
    };
    Ok(Output {
        status,
        truncated: stdout.truncated || stderr.truncated,
        stdout: stdout.bytes,
        stderr: stderr.bytes,
        timed_out,
    })
}

/// The process group of the child, killed with the system `kill` (the
/// workspace forbids `unsafe`, so no `kill(2)` with a negative pid).
struct Group(Option<u32>);

impl Group {
    fn kill(&mut self) {
        let Some(pgid) = self.0.take() else {
            return;
        };
        #[cfg(unix)]
        {
            let result = std::process::Command::new("kill")
                .args(["-KILL", "--", &format!("-{pgid}")])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            if let Err(err) = result {
                tracing::warn!(pgid, %err, "could not run kill for a plugin's process group");
            }
        }
        #[cfg(not(unix))]
        let _ = pgid;
    }
}

impl Drop for Group {
    fn drop(&mut self) {
        self.kill();
    }
}

#[derive(Debug, Default)]
struct Capture {
    bytes: Vec<u8>,
    truncated: bool,
}

impl Capture {
    async fn read<R: AsyncRead + Unpin>(&mut self, reader: Option<R>) -> std::io::Result<()> {
        let Some(mut reader) = reader else {
            return Ok(());
        };
        let mut chunk = [0u8; 8192];
        loop {
            let n = reader.read(&mut chunk).await?;
            if n == 0 {
                return Ok(());
            }
            let room = MAX_CAPTURE_BYTES.saturating_sub(self.bytes.len());
            if n > room {
                self.truncated = true;
            }
            self.bytes.extend_from_slice(&chunk[..n.min(room)]);
        }
    }
}

fn to_lua(lua: &Lua, output: Output) -> mlua::Result<Table> {
    let out = lua.create_table()?;
    out.set("status", output.status)?;
    out.set("stdout", lua.create_string(&output.stdout)?)?;
    out.set("stderr", lua.create_string(&output.stderr)?)?;
    out.set("timed_out", output.timed_out)?;
    out.set("truncated", output.truncated)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_are_not_inherited() {
        let env = environment(
            [
                ("PATH", "/bin"),
                ("ANTHROPIC_API_KEY", "sk"),
                ("CREST_PLUGIN_JIRA_TOKEN", "t"),
                ("HOME", "/h"),
            ]
            .into_iter()
            .map(|(k, v)| (OsString::from(k), OsString::from(v))),
        );
        let keys: Vec<_> = env.iter().map(|(k, _)| k.to_string_lossy()).collect();
        assert_eq!(keys, ["PATH", "HOME"]);
    }

    #[test]
    fn programs_are_found_only_in_absolute_path_entries() {
        let dir = tempfile::tempdir().unwrap();
        let path = std::env::join_paths([Path::new("."), Path::new("/usr/bin"), Path::new("/bin")])
            .unwrap();
        let found = resolve("sh", Some(path.clone())).unwrap();
        assert!(found.is_absolute(), "{found:?}");
        let local = dir.path().join("only-here");
        std::fs::write(&local, "").unwrap();
        assert_eq!(resolve("only-here", Some(path)), None);
        assert_eq!(resolve("sh", None), None);
    }

    #[test]
    fn cwd_must_stay_in_the_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("ws");
        std::fs::create_dir_all(root.join("sub")).unwrap();
        assert!(inside(&root, "sub").unwrap().ends_with("ws/sub"));
        assert!(inside(&root, ".").is_ok());
        for escape in ["..", "../", "/", "sub/../.."] {
            let err = inside(&root, escape).unwrap_err().to_string();
            assert!(err.contains("outside the workspace"), "{escape}: {err}");
        }
        assert!(inside(&root, "missing").is_err());
    }
}
