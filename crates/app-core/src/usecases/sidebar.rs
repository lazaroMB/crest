//! What the chats sidebar lists: the projects, each with its chats, and the
//! chats that are in no project.
//!
//! Projects come pinned first, then by their newest chat (a project with no
//! chat last, by name); the chats of each, and the standalone ones, newest
//! first. [`project_for_directory`] gives a chat opened in a directory its
//! project, so no chat with a directory is listed as standalone.

use domain::conversation::SessionId;
use domain::project::{Project, name_from_root, normalize_root};

use crate::ports::store::{SessionStorePort, SessionSummary, StoreError};

/// A project and its chats, newest first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidebarProject {
    /// The project.
    pub project: Project,
    /// Its chats, newest first.
    pub chats: Vec<SessionSummary>,
}

/// The sidebar's content.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sidebar {
    /// Projects in the order the sidebar lists them.
    pub projects: Vec<SidebarProject>,
    /// Chats in no project, newest first.
    pub chats: Vec<SessionSummary>,
}

/// Reads the projects and the sessions of `store` and groups them.
///
/// # Errors
///
/// The store's error.
pub async fn sidebar(store: &dyn SessionStorePort) -> Result<Sidebar, StoreError> {
    let projects = store.projects().await?;
    let sessions = store.list_all().await?;
    Ok(group(projects, sessions))
}

/// Groups `sessions` (newest first) by project. A session of a project that
/// does not exist is listed as standalone.
#[must_use]
pub fn group(projects: Vec<Project>, sessions: Vec<SessionSummary>) -> Sidebar {
    let mut grouped: Vec<SidebarProject> = projects
        .into_iter()
        .map(|project| SidebarProject {
            project,
            chats: Vec::new(),
        })
        .collect();
    let mut standalone = Vec::new();
    for session in sessions {
        let owner = grouped
            .iter_mut()
            .find(|entry| Some(entry.project.id) == session.project_id);
        match owner {
            Some(entry) => entry.chats.push(session),
            None => standalone.push(session),
        }
    }
    let newest = |entry: &SidebarProject| entry.chats.first().map(|chat| chat.updated_at);
    grouped.sort_by(|a, b| {
        b.project
            .pinned
            .cmp(&a.project.pinned)
            .then_with(|| newest(b).cmp(&newest(a)))
            .then_with(|| a.project.name.cmp(&b.project.name))
    });
    Sidebar {
        projects: grouped,
        chats: standalone,
    }
}

/// The project a chat opened in `directory` belongs to: the first project
/// whose first directory it is, else the first that has it, else a new one
/// named after the directory.
///
/// # Errors
///
/// The store's error, or a directory that is empty.
pub async fn project_for_directory(
    store: &dyn SessionStorePort,
    directory: &str,
) -> Result<Project, StoreError> {
    let wanted = normalize_root(directory).ok_or(domain::project::ProjectError::NoDirectory)?;
    let projects = store.projects().await?;
    let first = |project: &&Project| project.roots.first() == Some(&wanted);
    let any = |project: &&Project| project.roots.contains(&wanted);
    match projects
        .iter()
        .find(first)
        .or_else(|| projects.iter().find(any))
    {
        Some(project) => Ok(project.clone()),
        None => {
            store
                .create_project(&name_from_root(&wanted), &[wanted])
                .await
        }
    }
}

/// Puts the saved chat `id` in the project of `directory` when it has none
/// (a chat saved by a version without projects).
///
/// # Errors
///
/// The store's error.
pub async fn adopt(
    store: &dyn SessionStorePort,
    id: SessionId,
    directory: &str,
) -> Result<(), StoreError> {
    let project = project_for_directory(store, directory).await?;
    store.move_session(id, Some(project.id)).await
}

#[cfg(test)]
mod tests {
    use domain::conversation::{ProjectId, Session};

    use super::*;
    use crate::ports::store::{ProjectStorePort as _, StoredSession};
    use crate::testing::system::InMemorySessionStore;

    fn summary(title: &str, at: u64, project: Option<ProjectId>) -> SessionSummary {
        let mut session = Session::new("/w", at);
        session.title = Some(title.into());
        session.project_id = project;
        session.updated_at = at;
        StoredSession::new(session).summary()
    }

    fn project(name: &str, pinned: bool) -> Project {
        Project {
            id: ProjectId::new(),
            name: name.into(),
            pinned,
            roots: vec![format!("/w/{name}")],
        }
    }

    fn titles(chats: &[SessionSummary]) -> Vec<&str> {
        chats
            .iter()
            .map(|chat| chat.title.as_deref().unwrap_or(""))
            .collect()
    }

    #[test]
    fn projects_come_pinned_then_by_newest_chat_and_chats_that_belong_to_none_stay_apart() {
        let (old, fresh, pinned, empty) = (
            project("old", false),
            project("fresh", false),
            project("pinned", true),
            project("empty", false),
        );
        let sessions = vec![
            summary("loose-new", 90, None),
            summary("fresh-2", 80, Some(fresh.id)),
            summary("gone-project", 70, Some(ProjectId::new())),
            summary("fresh-1", 60, Some(fresh.id)),
            summary("old-1", 10, Some(old.id)),
            summary("pinned-1", 5, Some(pinned.id)),
        ];
        let got = group(vec![empty, old, fresh, pinned], sessions);
        let names: Vec<_> = got
            .projects
            .iter()
            .map(|p| p.project.name.as_str())
            .collect();
        assert_eq!(names, ["pinned", "fresh", "old", "empty"]);
        assert_eq!(titles(&got.projects[1].chats), ["fresh-2", "fresh-1"]);
        assert!(got.projects[3].chats.is_empty());
        assert_eq!(
            titles(&got.chats),
            ["loose-new", "gone-project"],
            "a missing project's chat is listed as standalone"
        );
    }

    #[tokio::test]
    async fn a_directory_finds_its_project_or_gets_one() {
        let store = InMemorySessionStore::new();
        store
            .create_project("api", &["/w/api".into(), "/w/web".into()])
            .await
            .unwrap();
        let by_first = project_for_directory(&store, "/w/api/").await.unwrap();
        assert_eq!(
            by_first.name, "api",
            "the project whose first directory it is"
        );
        let by_any = project_for_directory(&store, "/w/web").await.unwrap();
        assert_eq!(by_any.name, "api", "else one that has it");
        let created = project_for_directory(&store, "/w/new/").await.unwrap();
        assert_eq!((created.name.as_str(), created.roots.len()), ("new", 1));
        assert_eq!(store.projects().await.unwrap().len(), 2);
        assert!(project_for_directory(&store, "  ").await.is_err());
    }
}
