//! Killing a command together with every process it started.
//!
//! Each command is spawned as the leader of a new process group
//! ([`tokio::process::Command::process_group`] with `0`), so its children
//! and grandchildren share its group id unless they explicitly leave it
//! (`setsid`, `setpgid`). Signalling the whole group needs `kill(2)` with a
//! negative pid, which std and tokio only expose through `unsafe` libc
//! calls (forbidden in this workspace) and `nix`/`rustix` are not workspace
//! dependencies. So the group is killed by running the system `kill`
//! utility: `kill -KILL -- -<pgid>`. It costs a fork per command, which is
//! negligible next to the command itself.
//!
//! A process group id stays reserved while any member of the group is
//! alive, so the signal cannot reach an unrelated group as long as some
//! process of ours is left; once all of them are gone the id could in
//! theory be reused, which is why the group is killed as soon as the
//! leader exits and only once.

use std::process::{Command, Stdio};

use tokio::process::Child;

/// The process group of a spawned command. Dropping it kills the group, so
/// a cancelled `run` (its future dropped) leaves no process behind.
#[derive(Debug)]
pub struct ProcessGroup {
    pgid: Option<u32>,
}

impl ProcessGroup {
    /// The group led by `child`, which must have been spawned with
    /// `process_group(0)`.
    pub fn of(child: &Child) -> Self {
        Self { pgid: child.id() }
    }

    /// Sends `SIGKILL` to every process of the group. Only the first call
    /// does anything.
    pub fn kill(&mut self) {
        let Some(pgid) = self.pgid.take() else {
            return;
        };
        let result = Command::new("kill")
            .args(["-KILL", "--", &format!("-{pgid}")])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        match result {
            // `kill` fails with ESRCH when the group is already empty.
            Ok(status) if !status.success() => {
                tracing::trace!(pgid, %status, "kill found no process group");
            }
            Ok(_) => {}
            Err(err) => tracing::warn!(pgid, %err, "could not run kill for process group"),
        }
    }
}

impl Drop for ProcessGroup {
    fn drop(&mut self) {
        self.kill();
    }
}
