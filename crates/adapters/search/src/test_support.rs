//! Helpers shared by the adapter's tests.

#[cfg(unix)]
use std::path::Path;
use std::path::PathBuf;

use domain::workspace::{WorkspacePath, WorkspaceRoot};

use crate::RipgrepSearch;

/// A temporary workspace directory.
pub struct Workspace {
    _dir: tempfile::TempDir,
    pub root: PathBuf,
}

impl Workspace {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        Self { _dir: dir, root }
    }

    /// Writes `content` to `path` (relative), creating parent directories.
    pub fn write(&self, path: &str, content: impl AsRef<[u8]>) -> &Self {
        let full = self.root.join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, content).unwrap();
        self
    }

    /// Runs `git init` in the workspace.
    pub fn git_init(&self) -> &Self {
        let status = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(&self.root)
            .status()
            .unwrap();
        assert!(status.success());
        self
    }

    pub fn search(&self) -> RipgrepSearch {
        RipgrepSearch::new(WorkspaceRoot::new(&self.root).unwrap())
    }
}

pub fn p(s: &str) -> WorkspacePath {
    WorkspacePath::parse(s).unwrap()
}

pub fn strs(paths: &[WorkspacePath]) -> Vec<&str> {
    paths.iter().map(WorkspacePath::as_str).collect()
}

/// Writes an executable shell script named `name` in `dir` and returns
/// its path. It waits until the script can be executed: a process forked
/// by another test while the file was open for writing keeps it "busy"
/// (`ETXTBSY`) until that process calls `exec`.
#[cfg(unix)]
pub fn fake_tool(dir: &Path, name: &str, script: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    for _ in 0..100 {
        match std::process::Command::new(&path)
            .arg("--crest-probe")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(mut child) => {
                let _ = child.kill();
                let _ = child.wait();
                return path;
            }
            Err(e) if e.raw_os_error() == Some(26) => {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(e) => panic!("cannot run {}: {e}", path.display()),
        }
    }
    panic!("{} stayed busy", path.display());
}
