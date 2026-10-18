//! The side panel with the workspace tree (`<leader>e`, action
//! `panel.tree`).
//!
//! The tree comes from [`FileSearchPort::tree`], the same port the agent's
//! `tree` tool uses, so it lists what the agent sees (`.gitignore`d files
//! and `.git/` excluded). [`DirWatcher`] is the only piece that touches the
//! file system: it pings on structural changes and the panel reloads from
//! the port.

use std::collections::HashSet;
use std::path::{Component, Path};

use application::ports::search::{TreeNode, TreeNodeKind};
use futures::channel::mpsc;
use gpui::SharedString;
use gpui_component::tree::TreeItem;
use notify::{EventKind, RecursiveMode, Watcher as _};

/// Suffix of the id of the "N more files" row under a directory cut by
/// the depth limit.
const OMITTED_SUFFIX: &str = "/\u{2026}";

/// The rows of the panel for `root`: its children, with directories open
/// when their id (their path) is in `expanded`.
pub fn tree_items(root: &TreeNode, expanded: &HashSet<SharedString>) -> Vec<TreeItem> {
    root.children
        .iter()
        .map(|node| tree_item(node, expanded))
        .collect()
}

fn tree_item(node: &TreeNode, expanded: &HashSet<SharedString>) -> TreeItem {
    let id = SharedString::from(node.path.as_str().to_owned());
    let label = SharedString::from(node.name.clone());
    if node.kind == TreeNodeKind::File {
        return TreeItem::new(id, label);
    }
    let mut children: Vec<TreeItem> = node
        .children
        .iter()
        .map(|child| tree_item(child, expanded))
        .collect();
    if node.omitted_files > 0 {
        let omitted = format!("{}{OMITTED_SUFFIX}", node.path);
        children
            .push(TreeItem::new(omitted, format!("… {} files", node.omitted_files)).disabled(true));
    }
    let open = expanded.contains(&id);
    TreeItem::new(id, label).children(children).expanded(open)
}

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

    use domain::workspace::WorkspacePath;

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

    // -- items --------------------------------------------------------------

    fn p(path: &str) -> WorkspacePath {
        WorkspacePath::parse(path).unwrap()
    }

    fn ids(items: &[TreeItem]) -> Vec<String> {
        items.iter().map(|item| item.id.to_string()).collect()
    }

    #[test]
    fn items_mirror_the_tree_and_its_expansion() {
        let files = [p("Cargo.toml"), p("src/lib.rs"), p("src/ports/fs.rs")];
        let root = TreeNode::from_files(&WorkspacePath::root(), &files, 2);
        let expanded = HashSet::from([SharedString::from("src")]);
        let items = tree_items(&root, &expanded);
        assert_eq!(ids(&items), ["src", "Cargo.toml"]);
        assert!(items[0].is_expanded());
        assert_eq!(ids(&items[0].children), ["src/ports", "src/lib.rs"]);
        // `src/ports` is cut by the depth: one disabled "1 files" row.
        let ports = &items[0].children[0];
        assert!(!ports.is_expanded());
        assert_eq!(ports.children.len(), 1);
        assert!(ports.children[0].is_disabled());
        assert_eq!(ports.children[0].label.as_ref(), "… 1 files");
    }
}
