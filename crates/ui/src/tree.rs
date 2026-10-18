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
use std::sync::Arc;

use application::keymap::Action;
use application::ports::search::{
    FileSearchPort, SearchError, TreeNode, TreeNodeKind, TreeRequest,
};
use domain::workspace::WorkspacePath;
use futures::channel::mpsc;
use gpui::{
    AppContext as _, Context, Entity, EventEmitter, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Subscription, Task, Window, div, px,
};
use gpui_component::list::ListItem;
use gpui_component::tree::{TreeEvent, TreeItem, TreeState, tree};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};
use notify::{EventKind, RecursiveMode, Watcher as _};

/// Action that shows or hides the panel.
pub const TOGGLE_ACTION: &str = "panel.tree";

/// Levels of the tree the panel asks the port for.
pub const PANEL_TREE_DEPTH: usize = 16;

/// Width of the panel.
const PANEL_WIDTH: f32 = 280.0;

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

/// What the panel asks the rest of the UI to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreePanelEvent {
    /// A file was clicked.
    Open(WorkspacePath),
}

/// The side panel. Hidden until toggled; it loads the tree when created and
/// on [`TreePanel::reload`].
pub struct TreePanel {
    search: Arc<dyn FileSearchPort>,
    state: Entity<TreeState>,
    root: Option<TreeNode>,
    expanded: HashSet<SharedString>,
    error: Option<String>,
    visible: bool,
    loading: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<TreePanelEvent> for TreePanel {}

impl TreePanel {
    /// A hidden panel over `search`; the first load starts right away.
    pub fn new(search: Arc<dyn FileSearchPort>, cx: &mut Context<Self>) -> Self {
        let state = cx.new(|cx| TreeState::new(cx));
        // Remember open directories so a reload keeps them open.
        let expansion = cx.subscribe(&state, |this: &mut Self, _, event: &TreeEvent, _| {
            match event {
                TreeEvent::Expanded(id) => this.expanded.insert(id.clone()),
                TreeEvent::Collapsed(id) => this.expanded.remove(id),
            };
        });
        let mut panel = Self {
            search,
            state,
            root: None,
            expanded: HashSet::new(),
            error: None,
            visible: false,
            loading: None,
            _subscriptions: vec![expansion],
        };
        panel.reload(cx);
        panel
    }

    /// Whether the panel is shown.
    pub fn is_visible(&self) -> bool {
        self.visible
    }

    /// Shows or hides the panel.
    pub fn toggle(&mut self, cx: &mut Context<Self>) {
        self.visible = !self.visible;
        cx.notify();
    }

    /// Runs `panel.tree`. Returns whether the action belongs to the panel.
    pub fn handle_action(&mut self, action: &Action, cx: &mut Context<Self>) -> bool {
        match action {
            Action::Builtin(name) if name == TOGGLE_ACTION => {
                self.toggle(cx);
                true
            }
            _ => false,
        }
    }

    /// The tree last loaded.
    pub fn root(&self) -> Option<&TreeNode> {
        self.root.as_ref()
    }

    /// The error of the last load, if it failed.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Whether a load is in flight.
    pub fn is_loading(&self) -> bool {
        self.loading.is_some()
    }

    /// Paths of the directories shown open.
    pub fn expanded(&self) -> &HashSet<SharedString> {
        &self.expanded
    }

    /// Opens or closes a directory by path.
    pub fn set_expanded(&mut self, path: &str, open: bool, cx: &mut Context<Self>) {
        let id = SharedString::from(path.to_owned());
        if open {
            self.expanded.insert(id);
        } else {
            self.expanded.remove(&id);
        }
        self.rebuild(cx);
    }

    /// Loads the tree again from the port. A load in flight is dropped,
    /// which cancels its search.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let search = self.search.clone();
        let request = TreeRequest {
            path: None,
            depth: PANEL_TREE_DEPTH,
        };
        self.loading = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { search.tree(request).await })
                .await;
            this.update(cx, |this, cx| this.loaded(result, cx)).ok();
        }));
    }

    fn loaded(&mut self, result: Result<TreeNode, SearchError>, cx: &mut Context<Self>) {
        self.loading = None;
        match result {
            Ok(root) => {
                self.error = None;
                self.root = Some(root);
                self.rebuild(cx);
            }
            // Keep the last good tree; show the error above it.
            Err(err) => self.error = Some(err.to_string()),
        }
        cx.notify();
    }

    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let items = match &self.root {
            Some(root) => tree_items(root, &self.expanded),
            None => Vec::new(),
        };
        self.state
            .update(cx, |state, cx| state.set_items(items, cx));
        cx.notify();
    }
}

impl Render for TreePanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.visible {
            return div().into_any_element();
        }
        let theme = cx.theme();
        let title = self
            .root
            .as_ref()
            .map_or_else(|| "workspace".to_owned(), |root| root.name.clone());
        let mut panel = v_flex()
            .w(px(PANEL_WIDTH))
            .h_full()
            .flex_none()
            .border_r_1()
            .border_color(theme.border)
            .bg(theme.sidebar)
            .text_color(theme.sidebar_foreground)
            .child(
                div()
                    .px_2()
                    .py_1()
                    .text_color(theme.muted_foreground)
                    .child(SharedString::from(title)),
            );
        if let Some(error) = &self.error {
            panel = panel.child(
                div()
                    .px_2()
                    .text_color(theme.danger)
                    .child(SharedString::from(error.clone())),
            );
        }
        let weak = cx.entity().downgrade();
        let muted = theme.muted_foreground;
        panel
            .child(
                tree(&self.state, move |ix, entry, _selected, _window, _cx| {
                    let item = entry.item();
                    let marker = match (entry.is_folder(), entry.is_expanded()) {
                        (true, true) => "▾",
                        (true, false) => "▸",
                        (false, _) => " ",
                    };
                    let mut row = ListItem::new(ix)
                        .pl(px(8. + 12. * entry.depth() as f32))
                        .child(
                            h_flex()
                                .gap_1()
                                .child(div().text_color(muted).child(marker))
                                .child(item.label.clone()),
                        );
                    if !entry.is_folder() && !entry.is_disabled() {
                        let weak = weak.clone();
                        let id = item.id.clone();
                        row = row.on_click(move |_, _, cx| {
                            if let Ok(path) = WorkspacePath::parse(&id) {
                                weak.update(cx, |_, cx| cx.emit(TreePanelEvent::Open(path)))
                                    .ok();
                            }
                        });
                    }
                    row
                })
                .flex_1(),
            )
            .into_any_element()
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

    // -- panel --------------------------------------------------------------

    use application::testing::fs::InMemoryFs;
    use domain::workspace::WorkspaceRoot;
    use gpui::{TestAppContext, VisualTestContext};

    fn p(path: &str) -> WorkspacePath {
        WorkspacePath::parse(path).unwrap()
    }

    fn fake_fs() -> Arc<InMemoryFs> {
        Arc::new(
            InMemoryFs::new(WorkspaceRoot::new("/ws").unwrap())
                .with_file("Cargo.toml", "[workspace]\n")
                .with_file("src/lib.rs", "")
                .with_file("src/ports/fs.rs", ""),
        )
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

    fn panel(
        search: Arc<dyn FileSearchPort>,
        cx: &mut TestAppContext,
    ) -> (Entity<TreePanel>, &mut VisualTestContext) {
        cx.update(gpui_component::init);
        let (panel, cx) = cx.add_window_view(|_, cx| TreePanel::new(search, cx));
        cx.run_until_parked();
        (panel, cx)
    }

    /// Ids of the rows the tree shows (open directories only).
    fn visible(panel: &Entity<TreePanel>, cx: &mut VisualTestContext) -> Vec<String> {
        panel.read_with(cx, |panel, cx| {
            let state = panel.state.read(cx);
            (0..)
                .map_while(|ix| state.entry(ix))
                .map(|entry| entry.item().id.to_string())
                .collect()
        })
    }

    #[gpui::test]
    fn loads_the_tree_from_the_port(cx: &mut TestAppContext) {
        let (panel, cx) = panel(fake_fs(), cx);
        panel.read_with(cx, |panel, _| {
            assert!(!panel.is_loading());
            assert!(panel.error().is_none());
            assert_eq!(panel.root().map(|root| root.children.len()), Some(2));
        });
        assert_eq!(visible(&panel, cx), ["src", "Cargo.toml"]);
    }

    #[gpui::test]
    fn panel_tree_action_toggles_it(cx: &mut TestAppContext) {
        let (panel, cx) = panel(fake_fs(), cx);
        panel.update(cx, |panel, cx| {
            assert!(!panel.is_visible(), "hidden by default");
            assert!(panel.handle_action(&Action::builtin("panel.tree"), cx));
            assert!(panel.is_visible());
            assert!(!panel.handle_action(&Action::builtin("block.next"), cx));
            panel.handle_action(&Action::builtin("panel.tree"), cx);
            assert!(!panel.is_visible());
        });
    }

    #[gpui::test]
    fn reload_keeps_open_directories(cx: &mut TestAppContext) {
        let fs = fake_fs();
        let (panel, cx) = panel(fs.clone(), cx);
        panel.update(cx, |panel, cx| panel.set_expanded("src", true, cx));
        assert_eq!(
            visible(&panel, cx),
            ["src", "src/ports", "src/lib.rs", "Cargo.toml"]
        );
        fs.set_file("src/main.rs", "");
        panel.update(cx, |panel, cx| panel.reload(cx));
        cx.run_until_parked();
        assert_eq!(
            visible(&panel, cx),
            [
                "src",
                "src/ports",
                "src/lib.rs",
                "src/main.rs",
                "Cargo.toml"
            ]
        );
    }

    struct Failing;

    #[async_trait::async_trait]
    impl FileSearchPort for Failing {
        async fn list_files(
            &self,
            _: application::ports::search::ListFilesRequest,
        ) -> Result<application::ports::search::FileList, SearchError> {
            Err(SearchError::Io("disk on fire".into()))
        }
        async fn find_file(
            &self,
            _: application::ports::search::FindFileRequest,
        ) -> Result<application::ports::search::FileList, SearchError> {
            Err(SearchError::Io("disk on fire".into()))
        }
        async fn tree(&self, _: TreeRequest) -> Result<TreeNode, SearchError> {
            Err(SearchError::Io("disk on fire".into()))
        }
    }

    #[gpui::test]
    fn failed_load_shows_the_error(cx: &mut TestAppContext) {
        let (panel, cx) = panel(Arc::new(Failing), cx);
        panel.read_with(cx, |panel, _| {
            assert!(panel.root().is_none());
            assert!(panel.error().is_some_and(|e| e.contains("disk on fire")));
        });
    }

    #[gpui::test]
    fn draws_when_visible(cx: &mut TestAppContext) {
        let (panel, cx) = panel(fake_fs(), cx);
        panel.update(cx, |panel, cx| {
            panel.toggle(cx);
            panel.set_expanded("src", true, cx);
        });
        cx.draw(
            gpui::point(gpui::px(0.), gpui::px(0.)),
            gpui::size(gpui::px(400.), gpui::px(600.)),
            |_, _| div().size_full().child(panel.clone()),
        );
    }
}
