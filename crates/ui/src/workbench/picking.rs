//! The directory picker for the rows of the sidebar's menus ("New
//! project...", "Add directory..."): the open session waits out of sight,
//! and the directory chosen runs the row.

use std::path::{Path, PathBuf};

use gpui::{Context, Window};

use super::{Content, Workbench};
use crate::chats::{ChatOp, Organize, PickFor, ProjectOp};

impl Workbench {
    /// The directory of the open chat, while a session shows.
    fn open_dir(&self) -> Option<PathBuf> {
        match &self.content {
            Content::Session(live) => Some(live.dir.clone()),
            _ => None,
        }
    }

    /// Shows the picker for a menu row that needs a directory. The open
    /// session waits out of sight; `Esc` or Back returns to it, choosing a
    /// directory runs the row and returns too.
    pub(super) fn pick_directory(
        &mut self,
        pick: PickFor,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(dir) = self.open_dir() else {
            return;
        };
        let at = match &pick {
            PickFor::AddDirectory { project } => self
                .chats
                .read(cx)
                .project_root(project)
                .map(PathBuf::from)
                .and_then(|root| root.parent().map(Path::to_path_buf)),
            PickFor::NewProject { .. } => dir.parent().map(Path::to_path_buf),
        };
        self.pending_pick = Some(pick);
        self.park_for_picker(at.as_deref(), window, cx);
    }

    /// Shows the picker at `at` and keeps the session out of sight, without
    /// the keyboard, until a directory is chosen: `Esc` or Back returns to
    /// it.
    pub(super) fn park_for_picker(
        &mut self,
        at: Option<&Path>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Content::Session(live) =
            std::mem::replace(&mut self.content, Content::Opening(PathBuf::new()))
        else {
            return;
        };
        live.session
            .update(cx, |session, cx| session.set_shown(false, window, cx));
        self.back = Some(live);
        self.show_picker(at, None, window, cx);
        if let Content::Picker(picker) = &self.content {
            picker.update(cx, |picker, cx| picker.set_can_go_back(true, cx));
        }
    }

    /// A directory chosen in the picker: opens it as a project, or, when the
    /// picker was for a menu row, runs the row.
    pub(super) fn chosen(&mut self, dir: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pick) = self.pending_pick.take() else {
            self.open(dir, window, cx);
            return;
        };
        self.go_back(window, cx);
        let dir = dir.display().to_string();
        let change = match pick {
            PickFor::NewProject { chat } => Organize::Chat {
                id: chat,
                op: ChatOp::Promote(dir),
            },
            PickFor::AddDirectory { project } => Organize::Project {
                id: project,
                op: ProjectOp::AddDirectory(dir),
            },
        };
        self.organize(change, window, cx);
    }
}
