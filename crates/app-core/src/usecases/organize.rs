//! What the sidebar's chat and project menus do. One call per menu row: the
//! UI picks the [`ChatAction`] or [`ProjectAction`] of the row and calls
//! [`chat_action`] or [`project_action`], so it holds no rules. A refusal
//! comes back as the store's error, whose message is fit to show.
//!
//! Nothing here deletes a chat except [`ChatAction::Delete`]. Archiving keeps
//! every block; deleting a project sets its chats free as standalone ones.

use domain::conversation::{ProjectId, SessionId};
use domain::project::Project;

use crate::ports::store::{SessionStorePort, StoreError};

/// A row of a chat's menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatAction {
    /// "Rename": the new title; empty clears it.
    Rename(String),
    /// "Pin" (`true`) or "Unpin" (`false`).
    Pin(bool),
    /// "Move to project..." with a project, "Remove from project" with
    /// `None`. The chat keeps its history.
    Move(Option<ProjectId>),
    /// "Archive" (`true`) or restore from the Archived section (`false`).
    Archive(bool),
    /// "Delete", for good, with the history. Confirm before running it.
    Delete,
    /// "Branch chat": a new chat in the same project with the history
    /// copied up to now.
    Branch,
}

/// What a [`ChatAction`] left to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatDone {
    /// Done; refresh the sidebar.
    Changed,
    /// The branch that was created, to open.
    Branched(SessionId),
}

/// Runs a chat menu row on chat `id`.
///
/// # Errors
///
/// [`StoreError::NotFound`], [`StoreError::ProjectNotFound`] for a move, or
/// the store's error.
pub async fn chat_action(
    store: &dyn SessionStorePort,
    id: SessionId,
    action: ChatAction,
) -> Result<ChatDone, StoreError> {
    let changed = |()| ChatDone::Changed;
    match action {
        ChatAction::Rename(title) => store.rename_session(id, &title).await.map(changed),
        ChatAction::Pin(pinned) => store.pin_session(id, pinned).await.map(changed),
        ChatAction::Move(project) => store.move_session(id, project).await.map(changed),
        ChatAction::Archive(on) => store.set_session_archived(id, on).await.map(changed),
        ChatAction::Delete => store.delete(id).await.map(changed),
        ChatAction::Branch => store.branch_session(id).await.map(ChatDone::Branched),
    }
}

/// "New project..." on a chat: creates a project of one directory and moves
/// the chat into it. Nothing is created when the project is refused.
///
/// # Errors
///
/// The refusal of the project (no name or directory), or the store's error.
pub async fn promote_chat(
    store: &dyn SessionStorePort,
    id: SessionId,
    name: &str,
    directory: &str,
) -> Result<Project, StoreError> {
    store.load(id).await?;
    let project = store.create_project(name, &[directory.to_owned()]).await?;
    store.move_session(id, Some(project.id)).await?;
    Ok(project)
}

/// A row of a project's menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectAction {
    /// "Rename": the new name, which cannot be empty.
    Rename(String),
    /// "Pin to top" (`true`) or "Unpin" (`false`).
    Pin(bool),
    /// "Add directory...".
    AddDirectory(String),
    /// The remove button of a directory; the last one cannot go (the menu
    /// disables it when the project has one directory).
    RemoveDirectory(String),
    /// "Archive" (`true`) with its chats, or restore (`false`).
    Archive(bool),
    /// "Delete": the chats are kept as standalone chats. Confirm first.
    Delete,
}

/// What a [`ProjectAction`] left to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectDone {
    /// The project as it is now.
    Updated(Project),
    /// The project is gone; this many chats became standalone.
    Deleted {
        /// Chats set free.
        chats_freed: usize,
    },
}

/// Runs a project menu row on `project`.
///
/// # Errors
///
/// The refusal (empty name or path, last directory),
/// [`StoreError::ProjectNotFound`] or the store's error.
pub async fn project_action(
    store: &dyn SessionStorePort,
    project: &Project,
    action: ProjectAction,
) -> Result<ProjectDone, StoreError> {
    let id = project.id;
    let project = match action {
        ProjectAction::Rename(name) => store.rename_project(id, &name).await?,
        ProjectAction::Pin(pinned) => store.pin_project(id, pinned).await?,
        ProjectAction::AddDirectory(path) => store.add_root(id, &path).await?,
        ProjectAction::RemoveDirectory(path) => store.remove_root(id, &path).await?,
        ProjectAction::Archive(archived) => store.set_project_archived(id, archived).await?,
        ProjectAction::Delete => {
            let chats_freed = store.delete_project(id).await?;
            return Ok(ProjectDone::Deleted { chats_freed });
        }
    };
    Ok(ProjectDone::Updated(project))
}

#[cfg(test)]
mod tests {
    use domain::conversation::Session;

    use super::*;
    use crate::ports::store::{ProjectStorePort as _, StoredSession};
    use crate::testing::system::InMemorySessionStore;
    use crate::usecases::sidebar::{archived, sidebar};

    async fn chat(store: &InMemorySessionStore, at: u64) -> SessionId {
        let mut session = Session::new("", at);
        session.title = Some(format!("chat {at}"));
        let id = session.id;
        store.save(StoredSession::new(session)).await.unwrap();
        id
    }

    /// What the sidebar and the Archived section show, as
    /// `sidebar chats / project names with chats / archived rows`.
    async fn view(store: &InMemorySessionStore) -> String {
        let shown = sidebar(store).await.unwrap();
        let kept = archived(store).await.unwrap();
        let projects: Vec<String> = shown
            .projects
            .iter()
            .map(|p| format!("{}:{}", p.project.name, p.chats.len()))
            .collect();
        format!(
            "{} {projects:?} archived={}",
            shown.chats.len(),
            kept.count()
        )
    }

    #[tokio::test]
    async fn the_menu_rows_move_chats_and_projects_without_losing_any() {
        let store = InMemorySessionStore::new();
        let (a, b) = (chat(&store, 1).await, chat(&store, 2).await);
        let mut log = vec![view(&store).await];

        // New project from a chat.
        let api = promote_chat(&store, a, "api", "/w/api").await.unwrap();
        log.push(view(&store).await);

        // Archive and restore a chat, then a project with its chats.
        for (action, project) in [
            (ChatAction::Archive(true), None),
            (ChatAction::Archive(false), Some(true)),
            (ChatAction::Pin(true), Some(false)),
        ] {
            chat_action(&store, b, action).await.unwrap();
            if let Some(archived) = project {
                project_action(&store, &api, ProjectAction::Archive(archived))
                    .await
                    .unwrap();
            }
            log.push(view(&store).await);
        }

        // Branch lands next to its source; deleting the project frees chats.
        let ChatDone::Branched(branch) = chat_action(&store, a, ChatAction::Branch).await.unwrap()
        else {
            panic!("a branch is reported");
        };
        chat_action(&store, b, ChatAction::Move(Some(api.id)))
            .await
            .unwrap();
        log.push(view(&store).await);
        let deleted = project_action(&store, &api, ProjectAction::Delete).await;
        log.push(format!("{deleted:?}: {}", view(&store).await));
        let loose = store.load(branch).await.unwrap().session;
        log.push(format!("{:?} {:?}", loose.workspace_root, loose.project_id));

        assert_eq!(
            log,
            [
                r#"2 [] archived=0"#,
                r#"1 ["api:1"] archived=0"#,
                r#"0 ["api:1"] archived=1"#,
                r#"1 [] archived=1"#,
                r#"1 ["api:1"] archived=0"#,
                r#"0 ["api:3"] archived=0"#,
                r#"Ok(Deleted { chats_freed: 3 }): 3 [] archived=0"#,
                r#""" None"#,
            ]
        );
    }

    /// A store with the project `api` and one chat.
    async fn api_and_a_chat() -> (InMemorySessionStore, Project, SessionId) {
        let store = InMemorySessionStore::new();
        let api = store
            .create_project("api", &["/w/api".into()])
            .await
            .unwrap();
        let id = chat(&store, 1).await;
        (store, api, id)
    }

    #[tokio::test]
    async fn refused_rows_say_why_and_change_nothing() {
        let (store, api, id) = api_and_a_chat().await;
        let rows = [
            (ProjectAction::Rename("  ".into()), "a project needs a name"),
            (
                ProjectAction::AddDirectory(" ".into()),
                "a directory path cannot be empty",
            ),
            (
                ProjectAction::RemoveDirectory("/w/api/".into()),
                "a project keeps at least one directory",
            ),
        ];
        for (action, message) in rows {
            let refused = project_action(&store, &api, action).await.unwrap_err();
            assert_eq!(refused.to_string(), message);
        }
        // A project that is refused, or a chat that is missing, creates nothing.
        for (target, name) in [(id, " "), (SessionId::new(), "ghost")] {
            assert!(promote_chat(&store, target, name, "/w/x").await.is_err());
        }
        assert_eq!(store.projects().await.unwrap(), [api]);
    }
}
