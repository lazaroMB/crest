//! Spawning, finding and killing child processes on every platform.
//!
//! - [`background`] keeps a console program started by the GUI from
//!   flashing a console window on Windows.
//! - [`own_group`] and [`kill_group`] kill a command together with every
//!   process it started. On Unix the command leads a new process group,
//!   killed with the system `kill -KILL -- -<pgid>` (the workspace forbids
//!   the `unsafe` a `kill(2)` with a negative pid needs). On Windows the
//!   tree under the command is killed with `taskkill /F /T`, which only
//!   finds descendants while the command itself is alive.
//! - [`find_program`] looks a program up on a `PATH`, trying the `PATHEXT`
//!   extensions on Windows (`npx` is `npx.cmd`).
//! - [`open_url`] opens a page in the default browser.
//! - [`same_env_name`] compares environment variable names as the platform
//!   does (ignoring case on Windows).

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// `CREATE_NO_WINDOW`: the child gets no console window.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Marks `command` as a background helper: on Windows it gets no console
/// window (git, rg or node started from the GUI would flash one). Nothing
/// on other platforms. For a [`tokio::process::Command`], pass
/// `command.as_std_mut()`.
pub fn background(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = command;
}

/// Makes `command` the leader of its own process group on Unix, so
/// [`kill_group`] reaches its children; on Windows the same as
/// [`background`] (the tree is found by parent process instead).
pub fn own_group(command: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    background(command);
}

/// Kills the command whose process id is `leader` (spawned with
/// [`own_group`]) and every process it started. Blocks while the system
/// utility runs; failures are logged, never returned (the processes may
/// well be gone already).
pub fn kill_group(leader: u32) {
    let mut command = kill_command(leader);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    background(&mut command);
    match command.status() {
        // Fails when the group or the process is already gone.
        Ok(status) if !status.success() => {
            tracing::trace!(leader, %status, "no process left to kill");
        }
        Ok(_) => {}
        Err(err) => tracing::warn!(leader, %err, "could not run the kill utility"),
    }
}

#[cfg(unix)]
fn kill_command(leader: u32) -> Command {
    let mut command = Command::new("kill");
    command.args(["-KILL", "--", &format!("-{leader}")]);
    command
}

#[cfg(windows)]
fn kill_command(leader: u32) -> Command {
    let mut command = Command::new("taskkill");
    command.args(["/F", "/T", "/PID", &leader.to_string()]);
    command
}

#[cfg(not(any(unix, windows)))]
fn kill_command(leader: u32) -> Command {
    let mut command = Command::new("kill");
    command.args(["-KILL", &leader.to_string()]);
    command
}

/// Opens `url` in the default browser without waiting for it: `open` on
/// macOS, `rundll32 url.dll,FileProtocolHandler` on Windows (no shell, so
/// the `&` of a query string stays literal), `xdg-open` elsewhere. Returns
/// whether the opener started.
#[must_use]
pub fn open_url(url: &str) -> bool {
    let mut command = if cfg!(target_os = "macos") {
        Command::new("open")
    } else if cfg!(windows) {
        let mut command = Command::new("rundll32");
        command.arg("url.dll,FileProtocolHandler");
        command
    } else {
        Command::new("xdg-open")
    };
    command
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    background(&mut command);
    command.spawn().is_ok()
}

/// The executable `program` in the absolute directories of `path` (the
/// process `PATH` when `None`). Relative entries (`.`, empty) are skipped,
/// so a repository cannot shadow a program with its own file. On Unix a
/// candidate must have an execute bit; on Windows a name without an
/// extension is tried with each `PATHEXT` extension (`.COM;.EXE;.BAT;.CMD`
/// when unset), so `npx` finds `npx.cmd`.
#[must_use]
pub fn find_program(program: &str, path: Option<OsString>) -> Option<PathBuf> {
    let path = path.or_else(|| std::env::var_os("PATH"))?;
    let names = candidate_names(program, std::env::var_os("PATHEXT"));
    std::env::split_paths(&path)
        .filter(|dir| dir.is_absolute())
        .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
        .find(|candidate| is_executable(candidate))
}

/// The file names [`find_program`] tries for `program` in each directory.
fn candidate_names(program: &str, pathext: Option<OsString>) -> Vec<OsString> {
    let mut names = vec![OsString::from(program)];
    if cfg!(windows) && Path::new(program).extension().is_none() {
        let pathext = pathext.unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".into());
        names.extend(
            pathext
                .to_string_lossy()
                .split(';')
                .filter(|ext| ext.starts_with('.'))
                .map(|ext| OsString::from(format!("{program}{}", ext.to_ascii_lowercase()))),
        );
        // A bare `npx` file next to `npx.cmd` (a shell script for Git Bash)
        // cannot be started by Windows: try the extensions first.
        names.rotate_left(1);
    }
    names
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

/// Whether two environment variable names are the same variable: ignoring
/// ASCII case on Windows, exactly elsewhere.
#[must_use]
pub fn same_env_name(a: &OsStr, b: &str) -> bool {
    if cfg!(windows) {
        a.to_str().is_some_and(|a| a.eq_ignore_ascii_case(b))
    } else {
        a == b
    }
}

/// The name of a program without its directory or (on Windows) its
/// extension: `C:\Git\cmd\git.exe` and `git` are both `git`.
#[must_use]
pub fn program_name(program: &str) -> &str {
    let name = program
        .rsplit(|c| c == '/' || (cfg!(windows) && c == '\\'))
        .next()
        .unwrap_or(program);
    if cfg!(windows) {
        let lower = name.to_ascii_lowercase();
        for ext in [".exe", ".cmd", ".bat", ".com"] {
            if lower.ends_with(ext) {
                return &name[..name.len() - ext.len()];
            }
        }
    }
    name
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn program_name_strips_the_directory() {
        assert_eq!(program_name("git"), "git");
        assert_eq!(program_name("/usr/bin/git"), "git");
    }

    #[cfg(windows)]
    #[test]
    fn program_name_strips_windows_directories_and_extensions() {
        assert_eq!(program_name(r"C:\Git\cmd\git.exe"), "git");
        assert_eq!(program_name("rtk.EXE"), "rtk");
        assert_eq!(program_name("npx.cmd"), "npx");
    }

    #[cfg(unix)]
    #[test]
    fn unix_tries_the_name_alone() {
        assert_eq!(candidate_names("npx", Some(".CMD".into())), ["npx"]);
    }

    #[cfg(windows)]
    #[test]
    fn windows_tries_pathext_before_the_bare_name() {
        assert_eq!(
            candidate_names("npx", Some(".EXE;.CMD".into())),
            ["npx.exe", "npx.cmd", "npx"]
        );
        assert_eq!(candidate_names("git.exe", None), ["git.exe"]);
    }

    #[test]
    fn find_program_skips_relative_directories() {
        let dir = tempfile::tempdir().unwrap();
        let name = if cfg!(windows) { "tool.exe" } else { "tool" };
        let file = dir.path().join(name);
        std::fs::write(&file, "").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let path = std::env::join_paths([dir.path()]).unwrap();
        assert_eq!(find_program("tool", Some(path)), Some(file));
        assert_eq!(find_program("tool", Some(".".into())), None);
        assert_eq!(find_program("missing", Some(dir.path().into())), None);
    }

    #[cfg(unix)]
    #[test]
    fn find_program_needs_an_execute_bit_on_unix() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("plain"), "").unwrap();
        assert_eq!(find_program("plain", Some(dir.path().into())), None);
    }

    #[test]
    fn env_names_compare_as_the_platform_does() {
        assert!(same_env_name(OsStr::new("PATH"), "PATH"));
        assert_eq!(same_env_name(OsStr::new("Path"), "PATH"), cfg!(windows));
    }
}
