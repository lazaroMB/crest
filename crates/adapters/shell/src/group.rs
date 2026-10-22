//! Killing a command together with every process it started.
//!
//! Each command is spawned with [`platform::process::own_group`]: on Unix
//! it leads a new process group, so its children and grandchildren share
//! its group id unless they explicitly leave it (`setsid`, `setpgid`); on
//! Windows its tree is found through the parent process ids. See
//! [`platform::process::kill_group`] for how either is killed.
//!
//! A process group id stays reserved while any member of the group is
//! alive, so the signal cannot reach an unrelated group as long as some
//! process of ours is left; once all of them are gone the id could in
//! theory be reused, which is why the group is killed as soon as the
//! leader exits and only once.

use tokio::process::Child;

/// The process group of a spawned command. Dropping it kills the group, so
/// a cancelled `run` (its future dropped) leaves no process behind.
#[derive(Debug)]
pub struct ProcessGroup {
    pgid: Option<u32>,
}

impl ProcessGroup {
    /// The group led by `child`, which must have been spawned with
    /// [`platform::process::own_group`].
    pub fn of(child: &Child) -> Self {
        Self { pgid: child.id() }
    }

    /// Kills every process of the group. Only the first call does anything.
    pub fn kill(&mut self) {
        if let Some(pgid) = self.pgid.take() {
            platform::process::kill_group(pgid);
        }
    }
}

impl Drop for ProcessGroup {
    fn drop(&mut self) {
        self.kill();
    }
}
