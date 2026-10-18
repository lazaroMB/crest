//! The side panel with the workspace tree (`<leader>e`, action
//! `panel.tree`).
//!
//! The tree comes from [`FileSearchPort::tree`], the same port the agent's
//! `tree` tool uses, so it lists what the agent sees (`.gitignore`d files
//! and `.git/` excluded). [`DirWatcher`] is the only piece that touches the
//! file system: it pings on structural changes and the panel reloads from
//! the port.

use std::path::{Component, Path};

use futures::channel::mpsc;
use notify::{EventKind, RecursiveMode, Watcher as _};

/// Why the workspace cannot be watched.
#[derive(Debug, thiserror::Error)]
pub enum WatchError {
    /// The platform watcher refused the directory.
    #[error("cannot watch {path}: {source}")]
    Watch {
        /// Directory that was to be watched.
        path: String,
        /// Error of the platform watcher.
        source: notify::Error,
    },
}

/// Whether a file-system event can change the tree: files or directories
/// created, removed or renamed outside `.git/`. Content and metadata
/// changes and plain accesses are ignored.
pub fn changes_tree(event: &notify::Event) -> bool {
    use notify::event::ModifyKind;
    let structural = match event.kind {
        EventKind::Create(_) | EventKind::Remove(_) | EventKind::Any | EventKind::Other => true,
        EventKind::Modify(kind) => matches!(kind, ModifyKind::Name(_) | ModifyKind::Any),
        EventKind::Access(_) => false,
    };
    structural && !event.paths.iter().all(|path| in_git_dir(path))
}

fn in_git_dir(path: &Path) -> bool {
    path.components()
        .any(|c| matches!(c, Component::Normal(name) if name == ".git"))
}

/// Watches a directory recursively and pings a channel on every event that
/// [`changes_tree`]. Watching stops when it is dropped.
pub struct DirWatcher {
    _watcher: notify::RecommendedWatcher,
}

impl DirWatcher {
    /// Starts watching `root`. Pings arrive from the watcher's own thread;
    /// many may arrive for one change, so the reader debounces.
    pub fn new(root: &Path) -> Result<(Self, mpsc::UnboundedReceiver<()>), WatchError> {
        let (tx, rx) = mpsc::unbounded();
        let error = |source| WatchError::Watch {
            path: root.display().to_string(),
            source,
        };
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                // A watcher error (e.g. queue overflow) may hide changes: reload.
                if event.as_ref().map_or(true, changes_tree) {
                    let _ = tx.unbounded_send(());
                }
            })
            .map_err(error)?;
        watcher
            .watch(root, RecursiveMode::Recursive)
            .map_err(error)?;
        Ok((Self { _watcher: watcher }, rx))
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    use futures::StreamExt as _;
    use notify::event::{AccessKind, CreateKind, DataChange, ModifyKind, RenameMode};

    use super::*;

    fn event(kind: EventKind, path: &str) -> notify::Event {
        notify::Event::new(kind).add_path(PathBuf::from(path))
    }

    #[test]
    fn only_structural_changes_count() {
        let create = EventKind::Create(CreateKind::File);
        assert!(changes_tree(&event(create, "/ws/src/new.rs")));
        assert!(changes_tree(&event(
            EventKind::Modify(ModifyKind::Name(RenameMode::Both)),
            "/ws/a.rs"
        )));
        assert!(!changes_tree(&event(
            EventKind::Modify(ModifyKind::Data(DataChange::Content)),
            "/ws/a.rs"
        )));
        assert!(!changes_tree(&event(
            EventKind::Access(AccessKind::Any),
            "/ws/a.rs"
        )));
        assert!(!changes_tree(&event(create, "/ws/.git/index.lock")));
    }

    /// Waits up to five seconds for a ping.
    fn wait_ping(rx: &mut mpsc::UnboundedReceiver<()>) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if rx.try_recv().is_ok() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn pings_when_a_file_is_created() {
        let dir = tempfile::tempdir().unwrap();
        let (watcher, mut rx) = DirWatcher::new(dir.path()).unwrap();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/lib.rs"), "fn a() {}\n").unwrap();
        assert!(wait_ping(&mut rx), "a ping after creating files");
        // Dropping the watcher drops its sender, which ends the stream.
        drop(watcher);
        futures::executor::block_on(rx.collect::<Vec<_>>());
    }

    #[test]
    fn missing_directory_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing");
        assert!(matches!(
            DirWatcher::new(&missing),
            Err(WatchError::Watch { .. })
        ));
    }
}
