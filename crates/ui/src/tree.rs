//! The side panel with the workspace tree (`<leader>e`, action
//! `panel.tree`).
//!
//! Opening it gives it the focus: `Up`/`Down` move, `Right`/`Left` open and
//! close directories and `Enter` on a file references it in the prompt.
//! Typing (or `Esc`) goes back to the prompt. The selection is kept, by
//! path, when the tree reloads; until the first load ends the panel says
//! it is loading, and an empty workspace says so.
//!
//! The tree comes from [`FileSearchPort::tree`], the same port the agent's
//! `tree` tool uses, so it lists what the agent sees (`.gitignore`d files
//! and `.git/` excluded). [`DirWatcher`] is the only piece that touches the
//! file system: it pings on structural changes and the panel reloads from
//! the port. It is started on the background executor and does not watch
//! what the workspace's `.gitignore` leaves out (`target/`, `node_modules/`
//! ...): a build there costs neither watches nor reloads. While the panel
//! is hidden a change only marks it stale; it reloads when shown.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use application_core::keymap::Action;
use application_core::ports::search::{
    FileSearchPort, SearchError, TreeNode, TreeNodeKind, TreeRequest,
};
use domain::workspace::WorkspacePath;
use futures::channel::mpsc;
use futures::{FutureExt as _, Stream, StreamExt as _};
use gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _, Pixels, Render,
    ScrollStrategy, SharedString, Styled as _, Subscription, Task, Window, div, px,
};
use gpui_component::list::ListItem;
use gpui_component::tree::{TreeEvent, TreeItem, TreeState, tree};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use notify::{EventKind, RecursiveMode, Watcher as _};

/// Action that shows or hides the panel.
pub const TOGGLE_ACTION: &str = "panel.tree";

/// Levels of the tree the panel asks the port for.
pub const PANEL_TREE_DEPTH: usize = 16;

/// Quiet time after a change before the panel reloads, so a burst of
/// events (a checkout, a build) costs one search.
pub const REFRESH_DEBOUNCE: Duration = Duration::from_millis(200);

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
/// changes and plain accesses are ignored. On Windows every content change
/// arrives as an unspecified modification, so those are ignored there too.
pub fn changes_tree(event: &notify::Event) -> bool {
    use notify::event::ModifyKind;
    let structural = match event.kind {
        EventKind::Create(_) | EventKind::Remove(_) | EventKind::Any | EventKind::Other => true,
        EventKind::Modify(ModifyKind::Any) => !cfg!(windows),
        EventKind::Modify(kind) => matches!(kind, ModifyKind::Name(_)),
        EventKind::Access(_) => false,
    };
    structural && !event.paths.iter().all(|path| in_git_dir(path))
}

fn in_git_dir(path: &Path) -> bool {
    path.components()
        .any(|c| matches!(c, Component::Normal(name) if name == ".git"))
}

/// What the workspace's ignore files leave out: the root `.gitignore` and
/// `.git/info/exclude`, plus `.git/` itself. Nested `.gitignore` files and
/// the global excludes file are not read; a change under a path only they
/// ignore costs a reload that finds nothing new.
pub struct Ignored {
    root: PathBuf,
    canonical: Option<PathBuf>,
    matcher: Gitignore,
}

impl Ignored {
    /// The rules of the workspace at `root`; none when it has no ignore
    /// file or it does not parse.
    pub fn new(root: &Path) -> Self {
        let mut builder = GitignoreBuilder::new(root);
        for file in [root.join(".gitignore"), root.join(".git/info/exclude")] {
            if file.is_file() {
                // A bad line only loses that rule.
                let _ = builder.add(file);
            }
        }
        let canonical = root.canonicalize().ok().filter(|c| c != root);
        Self {
            root: root.to_owned(),
            canonical,
            matcher: builder.build().unwrap_or_else(|_| Gitignore::empty()),
        }
    }

    /// Whether `path` (absolute, as the watcher reports it) or one of its
    /// directories is ignored. Paths outside the root are not.
    pub fn is_ignored(&self, path: &Path) -> bool {
        // Try the path as given and, on macOS, its canonical form, so an
        // event reported under `/private/var/...` for a root opened as
        // `/var/...` is still recognised.
        if self.is_ignored_one(path) {
            return true;
        }
        if let Some(canonical) = &self.canonical {
            // Rebase the path onto the canonical root if it sits under the
            // given root, then test again.
            if let Ok(relative) = path.strip_prefix(&self.root) {
                let rebased = canonical.join(relative);
                if self.is_ignored_one(&rebased) {
                    return true;
                }
            }
        }
        false
    }

    fn is_ignored_one(&self, path: &Path) -> bool {
        let Ok(relative) = path.strip_prefix(&self.root) else {
            // The path may already be canonical; try that root too.
            if let Some(canonical) = &self.canonical {
                let Ok(relative) = path.strip_prefix(canonical) else {
                    return false;
                };
                if relative.as_os_str().is_empty() {
                    return false;
                }
                return in_git_dir(relative)
                    || self
                        .matcher
                        .matched_path_or_any_parents(relative, path.is_dir())
                        .is_ignore();
            }
            return false;
        };
        if relative.as_os_str().is_empty() {
            return false;
        }
        in_git_dir(relative)
            || self
                .matcher
                .matched_path_or_any_parents(relative, path.is_dir())
                .is_ignore()
    }
}

/// Watches a directory and pings a channel on every event that
/// [`changes_tree`] outside the ignored paths ([`Ignored`]). The root is
/// watched on its own and each top-level directory that is not ignored
/// recursively, so `target/` and the like get no watches at all; a
/// directory that appears at the top later is watched when it does.
/// Watching stops when it is dropped.
pub struct DirWatcher {
    _watcher: Arc<Mutex<Option<notify::RecommendedWatcher>>>,
}

impl DirWatcher {
    /// Starts watching `root`. It walks the directories it watches, so
    /// call it off the UI thread. Pings arrive from the watcher's own
    /// thread; many may arrive for one change, so the reader debounces.
    pub fn new(root: &Path) -> Result<(Self, mpsc::UnboundedReceiver<()>), WatchError> {
        let (tx, rx) = mpsc::unbounded();
        let error = |source| WatchError::Watch {
            path: root.display().to_string(),
            source,
        };
        let ignored = Arc::new(Ignored::new(root));
        let shared: Arc<Mutex<Option<notify::RecommendedWatcher>>> = Arc::default();
        // Weak: the watcher owns this handler, and must go when dropped.
        let (handler_ignored, handler_shared, handler_root) = (
            Arc::clone(&ignored),
            Arc::downgrade(&shared),
            root.to_owned(),
        );
        // macOS's FSEvents reports canonical paths (`/private/var/...` for
        // a root opened as `/var/...`): events are rebased onto the root
        // as given before they are compared with it.
        let canonical_root = root.canonicalize().ok().filter(|c| c != root);
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                let event = match event {
                    Ok(event) => event,
                    // A watcher error (e.g. queue overflow) may hide changes:
                    // reload.
                    Err(_) => {
                        let _ = tx.unbounded_send(());
                        return;
                    }
                };
                if !changes_tree(&event) {
                    return;
                }
                let paths: Vec<PathBuf> = event
                    .paths
                    .iter()
                    .map(|path| rebase(path, &handler_root, canonical_root.as_deref()))
                    .collect();
                let wanted: Vec<&PathBuf> = paths
                    .iter()
                    .filter(|path| !handler_ignored.is_ignored(path))
                    .collect();
                if wanted.is_empty() {
                    return;
                }
                // A new top-level directory: watch it too. Not from this
                // thread, which serves the watcher's own calls.
                let new_dirs: Vec<PathBuf> = wanted
                    .iter()
                    .filter(|path| path.parent() == Some(&handler_root) && path.is_dir())
                    .map(|path| (*path).clone())
                    .collect();
                if !new_dirs.is_empty()
                    && let Some(shared) = handler_shared.upgrade()
                {
                    std::thread::spawn(move || {
                        let mut guard = shared.lock().unwrap_or_else(PoisonError::into_inner);
                        if let Some(watcher) = guard.as_mut() {
                            for dir in new_dirs {
                                let _ = watcher.watch(&dir, RecursiveMode::Recursive);
                            }
                        }
                    });
                }
                let _ = tx.unbounded_send(());
            })
            .map_err(error)?;
        watcher
            .watch(root, RecursiveMode::NonRecursive)
            .map_err(error)?;
        let entries = std::fs::read_dir(root).map_err(|e| error(notify::Error::io(e)))?;
        for entry in entries.flatten() {
            let path = entry.path();
            if entry.file_type().is_ok_and(|t| t.is_dir()) && !ignored.is_ignored(&path) {
                // A directory removed meanwhile is simply not watched.
                let _ = watcher.watch(&path, RecursiveMode::Recursive);
            }
        }
        *shared.lock().unwrap_or_else(PoisonError::into_inner) = Some(watcher);
        Ok((Self { _watcher: shared }, rx))
    }
}

/// `path` under `root` when the watcher reported it under `canonical`,
/// the root's canonical form; any other path as it is.
fn rebase(path: &Path, root: &Path, canonical: Option<&Path>) -> PathBuf {
    canonical
        .and_then(|canonical| path.strip_prefix(canonical).ok())
        .map_or_else(|| path.to_owned(), |relative| root.join(relative))
}

/// What the panel asks the rest of the UI to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreePanelEvent {
    /// A file was clicked or chosen with `Enter`.
    Open(WorkspacePath),
    /// The workspace cannot be watched: the tree will not refresh by
    /// itself.
    WatchFailed(String),
}

/// The side panel. Hidden until toggled; it loads the tree when created and
/// on [`TreePanel::reload`].
pub struct TreePanel {
    search: Arc<dyn FileSearchPort>,
    state: Entity<TreeState>,
    focus_handle: FocusHandle,
    root: Option<TreeNode>,
    expanded: HashSet<SharedString>,
    error: Option<String>,
    visible: bool,
    loading: Option<Task<()>>,
    /// Whether the files changed while the panel was hidden: it reloads
    /// when shown.
    stale: bool,
    watching: Option<Task<()>>,
    /// The watcher being started.
    starting: Option<Task<()>>,
    watcher: Option<DirWatcher>,
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
            focus_handle: cx.focus_handle(),
            root: None,
            expanded: HashSet::new(),
            error: None,
            visible: false,
            loading: None,
            stale: false,
            watching: None,
            starting: None,
            watcher: None,
            _subscriptions: vec![expansion],
        };
        panel.reload(cx);
        panel
    }

    /// Whether the panel is shown.
    pub fn is_visible(&self) -> bool {
        self.visible
    }

    /// How wide the panel is: none of the window while hidden.
    pub fn width(&self) -> Pixels {
        if self.visible {
            px(PANEL_WIDTH)
        } else {
            px(0.)
        }
    }

    /// Shows or hides the panel; shown, it reloads if the files changed
    /// while it was hidden.
    pub fn toggle(&mut self, cx: &mut Context<Self>) {
        self.visible = !self.visible;
        if self.visible && std::mem::take(&mut self.stale) {
            self.reload(cx);
        }
        cx.notify();
    }

    /// Whether the files changed while the panel was hidden.
    pub fn is_stale(&self) -> bool {
        self.stale
    }

    /// Gives the keyboard focus to the tree, selecting its first row when
    /// none is.
    pub fn focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |state, cx| {
            if state.selected_index().is_none() && state.entry(0).is_some() {
                state.set_selected_index(Some(0), cx);
            }
            state.focus(window, cx);
        });
    }

    /// Runs `panel.tree`: shown, the tree takes the focus. Returns whether
    /// the action belongs to the panel.
    pub fn handle_action(
        &mut self,
        action: &Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        match action {
            Action::Builtin(name) if name == TOGGLE_ACTION => {
                self.toggle(cx);
                if self.visible {
                    self.focus(window, cx);
                }
                true
            }
            _ => false,
        }
    }

    /// `Enter` on a file opens it like a click.
    fn on_key(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if event.keystroke.key != "enter" {
            return;
        }
        let file = self
            .state
            .read(cx)
            .selected_entry()
            .filter(|entry| !entry.is_folder() && !entry.is_disabled())
            .map(|entry| entry.item().id.clone());
        if let Some(path) = file.and_then(|id| WorkspacePath::parse(&id).ok()) {
            cx.emit(TreePanelEvent::Open(path));
            cx.stop_propagation();
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
        // "Loading the files…" while nothing was loaded yet.
        cx.notify();
    }

    /// Reloads after each burst of `changes`, once [`REFRESH_DEBOUNCE`]
    /// passes without a new one; while hidden, it only marks the panel
    /// stale. Replaces any previous source.
    pub fn watch(
        &mut self,
        mut changes: impl Stream<Item = ()> + Unpin + 'static,
        cx: &mut Context<Self>,
    ) {
        self.watching = Some(cx.spawn(async move |this, cx| {
            while changes.next().await.is_some() {
                loop {
                    cx.background_executor().timer(REFRESH_DEBOUNCE).await;
                    // Pings during the wait restart it; an ended stream
                    // still gets its last reload.
                    let mut more = false;
                    while let Some(Some(())) = changes.next().now_or_never() {
                        more = true;
                    }
                    if !more {
                        break;
                    }
                }
                let changed = this.update(cx, |this, cx| {
                    if this.visible {
                        this.reload(cx);
                    } else {
                        this.stale = true;
                    }
                });
                if changed.is_err() {
                    break;
                }
            }
        }));
    }

    /// Watches `root` (the workspace directory) with a [`DirWatcher`],
    /// started on the background executor, and reloads on changes until
    /// the panel is dropped. A watcher that cannot start is reported with
    /// [`TreePanelEvent::WatchFailed`].
    pub fn watch_dir(&mut self, root: &Path, cx: &mut Context<Self>) {
        let root = root.to_owned();
        self.starting = Some(cx.spawn(async move |this, cx| {
            let started = cx
                .background_spawn(async move { DirWatcher::new(&root) })
                .await;
            this.update(cx, |this, cx| {
                this.starting = None;
                match started {
                    Ok((watcher, changes)) => {
                        this.watcher = Some(watcher);
                        this.watch(changes, cx);
                    }
                    Err(err) => cx.emit(TreePanelEvent::WatchFailed(format!(
                        "the tree will not refresh by itself: {err}"
                    ))),
                }
            })
            .ok();
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
        self.state.update(cx, |state, cx| {
            // `set_items` drops the selection: find the same path again,
            // or the row that took its place when it is gone.
            let selected = state.selected_item().map(|item| item.id.clone());
            let before = state.selected_index();
            state.set_items(items, cx);
            let Some(before) = before else {
                return;
            };
            let after = selected
                .and_then(|id| state.index_of(&id))
                .or_else(|| (0..=before).rev().find(|ix| state.entry(*ix).is_some()));
            state.set_selected_index(after, cx);
            if let Some(after) = after.filter(|after| *after != before) {
                state.scroll_to_item(after, ScrollStrategy::Nearest);
            }
        });
        cx.notify();
    }

    /// The path of the selected row, if any.
    pub fn selected_path(&self, cx: &App) -> Option<SharedString> {
        self.state
            .read(cx)
            .selected_item()
            .map(|item| item.id.clone())
    }
}

impl Focusable for TreePanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for TreePanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.visible {
            return div().into_any_element();
        }
        let focused = self.focus_handle.contains_focused(window, cx);
        let theme = cx.theme();
        let title = self
            .root
            .as_ref()
            .map_or_else(|| "workspace".to_owned(), |root| root.name.clone());
        let mut panel = v_flex()
            .id("tree-panel")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key))
            .w(px(PANEL_WIDTH))
            .h_full()
            .flex_none()
            .border_r_1()
            .border_color(if focused { theme.primary } else { theme.border })
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
        let note = match &self.root {
            None if self.loading.is_some() => Some(("tree-loading", "Loading the files…")),
            Some(root) if root.children.is_empty() => {
                Some(("tree-empty", "No files here (ignored ones are not listed)"))
            }
            _ => None,
        };
        if let Some((selector, text)) = note {
            panel = panel.child(
                div()
                    .debug_selector(move || selector.to_owned())
                    .px_2()
                    .py_1()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(text),
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
                // Shrinks inside the panel so the last rows are not cut.
                .flex_1()
                .min_h_0(),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

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

    use application_testing::fs::{InMemoryFs, test_root};

    use gpui::{TestAppContext, VisualTestContext};

    fn p(path: &str) -> WorkspacePath {
        WorkspacePath::parse(path).unwrap()
    }

    fn fake_fs() -> Arc<InMemoryFs> {
        Arc::new(
            InMemoryFs::new(test_root())
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
        panel.update_in(cx, |panel, window, cx| {
            assert!(!panel.is_visible(), "hidden by default");
            assert!(panel.handle_action(&Action::builtin("panel.tree"), window, cx));
            assert!(panel.is_visible());
            assert!(!panel.handle_action(&Action::builtin("block.next"), window, cx));
            panel.handle_action(&Action::builtin("panel.tree"), window, cx);
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

    #[gpui::test]
    fn reload_keeps_the_selected_path(cx: &mut TestAppContext) {
        let fs = fake_fs();
        let (panel, cx) = panel(fs.clone(), cx);
        panel.update(cx, |panel, cx| {
            panel.set_expanded("src", true, cx);
            panel.state.update(cx, |state, cx| {
                let ix = state.index_of(&SharedString::from("src/lib.rs"));
                state.set_selected_index(ix, cx);
            });
        });
        // A file above it moves it down a row.
        fs.set_file("src/a.rs", "");
        panel.update(cx, |panel, cx| panel.reload(cx));
        cx.run_until_parked();
        panel.read_with(cx, |panel, cx| {
            assert_eq!(panel.selected_path(cx).as_deref(), Some("src/lib.rs"));
            assert_eq!(panel.state.read(cx).selected_index(), Some(3));
        });
        // Gone: the row that took its place.
        assert!(fs.remove_file("src/lib.rs"));
        panel.update(cx, |panel, cx| panel.reload(cx));
        cx.run_until_parked();
        panel.read_with(cx, |panel, cx| {
            assert_eq!(panel.selected_path(cx).as_deref(), Some("Cargo.toml"));
        });
    }

    /// A port whose tree never comes.
    struct Pending;

    #[async_trait::async_trait]
    impl FileSearchPort for Pending {
        async fn list_files(
            &self,
            _: application_core::ports::search::ListFilesRequest,
        ) -> Result<application_core::ports::search::FileList, SearchError> {
            futures::future::pending().await
        }
        async fn find_file(
            &self,
            _: application_core::ports::search::FindFileRequest,
        ) -> Result<application_core::ports::search::FileList, SearchError> {
            futures::future::pending().await
        }
        async fn tree(&self, _: TreeRequest) -> Result<TreeNode, SearchError> {
            futures::future::pending().await
        }
    }

    fn shown(panel: &Entity<TreePanel>, cx: &mut VisualTestContext) {
        panel.update(cx, |panel, cx| panel.toggle(cx));
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
    }

    #[gpui::test]
    fn says_while_loading(cx: &mut TestAppContext) {
        let (loading, cx) = panel(Arc::new(Pending), cx);
        shown(&loading, cx);
        assert!(cx.debug_bounds("tree-loading").is_some());
        assert!(cx.debug_bounds("tree-empty").is_none());
    }

    #[gpui::test]
    fn says_when_the_workspace_is_empty(cx: &mut TestAppContext) {
        let (empty, cx) = panel(Arc::new(InMemoryFs::new(test_root())), cx);
        shown(&empty, cx);
        assert!(cx.debug_bounds("tree-empty").is_some());
        assert!(cx.debug_bounds("tree-loading").is_none());
    }

    struct Failing;

    #[async_trait::async_trait]
    impl FileSearchPort for Failing {
        async fn list_files(
            &self,
            _: application_core::ports::search::ListFilesRequest,
        ) -> Result<application_core::ports::search::FileList, SearchError> {
            Err(SearchError::Io("disk on fire".into()))
        }
        async fn find_file(
            &self,
            _: application_core::ports::search::FindFileRequest,
        ) -> Result<application_core::ports::search::FileList, SearchError> {
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

    // -- refresh ------------------------------------------------------------

    /// A port that counts the trees it builds.
    struct Counting {
        inner: Arc<dyn FileSearchPort>,
        trees: std::sync::atomic::AtomicUsize,
    }

    impl Counting {
        fn trees(&self) -> usize {
            self.trees.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    #[async_trait::async_trait]
    impl FileSearchPort for Counting {
        async fn list_files(
            &self,
            request: application_core::ports::search::ListFilesRequest,
        ) -> Result<application_core::ports::search::FileList, SearchError> {
            self.inner.list_files(request).await
        }
        async fn find_file(
            &self,
            request: application_core::ports::search::FindFileRequest,
        ) -> Result<application_core::ports::search::FileList, SearchError> {
            self.inner.find_file(request).await
        }
        async fn tree(&self, request: TreeRequest) -> Result<TreeNode, SearchError> {
            self.trees.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.inner.tree(request).await
        }
    }

    #[gpui::test]
    fn bursts_of_changes_reload_once_after_the_debounce(cx: &mut TestAppContext) {
        let fs = fake_fs();
        let port = Arc::new(Counting {
            inner: fs.clone(),
            trees: Default::default(),
        });
        let (panel, cx) = panel(port.clone(), cx);
        assert_eq!(port.trees(), 1, "initial load");
        let (tx, rx) = mpsc::unbounded();
        panel.update(cx, |panel, cx| {
            panel.toggle(cx);
            panel.watch(rx, cx);
        });

        fs.set_file("src/main.rs", "");
        for _ in 0..3 {
            tx.unbounded_send(()).unwrap();
        }
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(150));
        tx.unbounded_send(()).unwrap();
        cx.executor().advance_clock(Duration::from_millis(150));
        cx.run_until_parked();
        assert_eq!(port.trees(), 1, "still inside the debounce window");

        cx.executor().advance_clock(REFRESH_DEBOUNCE);
        cx.run_until_parked();
        assert_eq!(port.trees(), 2, "one reload for the burst");
        panel.update(cx, |panel, cx| panel.set_expanded("src", true, cx));
        assert!(visible(&panel, cx).contains(&"src/main.rs".to_owned()));
    }

    #[gpui::test]
    fn changes_while_hidden_reload_once_when_shown(cx: &mut TestAppContext) {
        let fs = fake_fs();
        let port = Arc::new(Counting {
            inner: fs.clone(),
            trees: Default::default(),
        });
        let (panel, cx) = panel(port.clone(), cx);
        let (tx, rx) = mpsc::unbounded();
        panel.update(cx, |panel, cx| panel.watch(rx, cx));

        fs.set_file("src/main.rs", "");
        for _ in 0..2 {
            tx.unbounded_send(()).unwrap();
            cx.run_until_parked();
            cx.executor().advance_clock(REFRESH_DEBOUNCE);
            cx.run_until_parked();
        }
        assert_eq!(port.trees(), 1, "hidden: no reload");
        assert!(panel.read_with(cx, |panel, _| panel.is_stale()));

        panel.update(cx, |panel, cx| panel.toggle(cx));
        cx.run_until_parked();
        assert_eq!(port.trees(), 2, "shown: one reload");
        assert!(!panel.read_with(cx, |panel, _| panel.is_stale()));
        panel.update(cx, |panel, cx| {
            panel.toggle(cx);
            panel.toggle(cx);
        });
        cx.run_until_parked();
        assert_eq!(port.trees(), 2, "nothing changed since");
    }

    #[test]
    fn the_root_gitignore_and_git_dir_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join(".gitignore"), "target/\n*.log\n").unwrap();
        std::fs::create_dir_all(root.join("target/debug")).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        let ignored = Ignored::new(root);
        assert!(ignored.is_ignored(&root.join("target")));
        assert!(ignored.is_ignored(&root.join("target/debug/app.d")));
        assert!(ignored.is_ignored(&root.join("src/run.log")));
        assert!(ignored.is_ignored(&root.join(".git/index")));
        assert!(!ignored.is_ignored(&root.join("src/main.rs")));
        assert!(!ignored.is_ignored(root));
        assert!(!ignored.is_ignored(Path::new("/elsewhere/target/x")));
        // No ignore file: nothing but `.git/`.
        let bare = tempfile::tempdir().unwrap();
        let none = Ignored::new(bare.path());
        assert!(!none.is_ignored(&bare.path().join("target/x")));
    }

    #[test]
    fn events_under_the_canonical_root_are_rebased_onto_the_given_one() {
        let root = Path::new("/var/folders/x/proj");
        let canonical = Path::new("/private/var/folders/x/proj");
        assert_eq!(
            rebase(&canonical.join("docs/a.md"), root, Some(canonical)),
            root.join("docs/a.md")
        );
        assert_eq!(
            rebase(&root.join("src"), root, Some(canonical)),
            root.join("src")
        );
        assert_eq!(rebase(&root.join("src"), root, None), root.join("src"));
    }

    /// Waits up to `wait` for a ping.
    fn pinged(pings: &mut mpsc::UnboundedReceiver<()>, wait: Duration) -> bool {
        let start = Instant::now();
        while start.elapsed() < wait {
            if pings.try_recv().is_ok() {
                while pings.try_recv().is_ok() {}
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn the_watcher_skips_ignored_paths_and_follows_new_top_level_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join(".gitignore"), "target/\n").unwrap();
        std::fs::create_dir_all(root.join("target/debug")).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        // Let the kernel settle any events produced by creating the
        // directories above before the watcher starts, so the watcher's
        // SinceNow baseline excludes them.
        std::thread::sleep(Duration::from_millis(200));
        let (_watcher, mut pings) = DirWatcher::new(root).unwrap();
        // Drain any stray events FSEvents may still deliver at startup.
        let _ = pinged(&mut pings, Duration::from_millis(300));

        std::fs::write(root.join("target/debug/app.o"), "").unwrap();
        assert!(!pinged(&mut pings, Duration::from_millis(400)), "ignored");

        std::fs::write(root.join("src/lib.rs"), "").unwrap();
        assert!(pinged(&mut pings, Duration::from_secs(5)), "watched");

        std::fs::create_dir(root.join("docs")).unwrap();
        assert!(pinged(&mut pings, Duration::from_secs(5)), "new directory");
        // The new directory is watched from a helper thread: give it a
        // moment, then a file inside it counts.
        std::thread::sleep(Duration::from_millis(200));
        std::fs::write(root.join("docs/a.md"), "").unwrap();
        assert!(pinged(&mut pings, Duration::from_secs(5)), "inside it");
    }

    /// A [`FileSearchPort`] over a real directory, walked with `std::fs`.
    struct DirSearch(std::path::PathBuf);

    impl DirSearch {
        fn files(&self) -> Vec<WorkspacePath> {
            let mut files = Vec::new();
            let mut dirs = vec![self.0.clone()];
            while let Some(dir) = dirs.pop() {
                for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        dirs.push(path);
                    } else if let Ok(rel) = path.strip_prefix(&self.0) {
                        files.extend(WorkspacePath::parse(&rel.to_string_lossy()).ok());
                    }
                }
            }
            files
        }
    }

    #[async_trait::async_trait]
    impl FileSearchPort for DirSearch {
        async fn list_files(
            &self,
            request: application_core::ports::search::ListFilesRequest,
        ) -> Result<application_core::ports::search::FileList, SearchError> {
            Ok(application_core::ports::search::FileList::from_matches(
                self.files(),
                request.limit,
            ))
        }
        async fn find_file(
            &self,
            _: application_core::ports::search::FindFileRequest,
        ) -> Result<application_core::ports::search::FileList, SearchError> {
            Ok(application_core::ports::search::FileList::default())
        }
        async fn tree(&self, request: TreeRequest) -> Result<TreeNode, SearchError> {
            let base = request.path.unwrap_or_else(WorkspacePath::root);
            Ok(TreeNode::from_files(&base, &self.files(), request.depth))
        }
    }

    #[gpui::test]
    fn refreshes_when_notify_sees_a_new_file(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "").unwrap();
        let (panel, cx) = panel(Arc::new(DirSearch(dir.path().to_owned())), cx);
        assert_eq!(visible(&panel, cx), ["a.txt"]);
        // The test scheduler rejects wake-ups from other threads, so the
        // pings of notify's thread are relayed from the test thread; in the
        // app `watch_dir` passes the watcher's channel straight in.
        let (_watcher, mut pings) = DirWatcher::new(dir.path()).unwrap();
        let (relay, changes) = mpsc::unbounded();
        panel.update(cx, |panel, cx| {
            panel.toggle(cx);
            panel.watch(changes, cx);
        });

        std::fs::write(dir.path().join("b.txt"), "").unwrap();
        for _ in 0..250 {
            std::thread::sleep(Duration::from_millis(20));
            while pings.try_recv().is_ok() {
                relay.unbounded_send(()).unwrap();
            }
            cx.executor().advance_clock(REFRESH_DEBOUNCE);
            cx.run_until_parked();
            if visible(&panel, cx).len() == 2 {
                break;
            }
        }
        assert_eq!(visible(&panel, cx), ["a.txt", "b.txt"]);
    }
}
