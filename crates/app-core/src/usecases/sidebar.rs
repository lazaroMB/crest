//! What the chats sidebar lists: the projects, each with its chats, and the
//! chats that are in no project.
//!
//! Projects come pinned first, then by their newest chat (a project with no
//! chat last, by name); the chats of each, and the standalone ones, pinned
//! first and then newest first. Archived projects, their chats and archived
//! chats are left out; [`archived`] lists them for the "Archived" section.
//! [`project_for_directory`] gives a chat opened in a directory its
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

/// What the "Archived" section lists.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Archived {
    /// Archived projects, each with every chat it still has (archived or
    /// not: they are hidden with it).
    pub projects: Vec<SidebarProject>,
    /// Archived chats of a project that is not archived, or of none.
    pub chats: Vec<SessionSummary>,
}

impl Archived {
    /// How many rows the section has, for its "Archived (N)" label.
    #[must_use]
    pub fn count(&self) -> usize {
        self.projects.len() + self.chats.len()
    }
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

/// Reads what is archived, grouped for the "Archived" section.
///
/// # Errors
///
/// The store's error.
pub async fn archived(store: &dyn SessionStorePort) -> Result<Archived, StoreError> {
    let projects = store.projects().await?;
    let sessions = store.list_all().await?;
    Ok(group_archived(projects, sessions))
}

/// Puts every session under its project; the sessions of no existing project
/// come back apart. Projects keep their order, chats theirs.
fn split(
    projects: Vec<Project>,
    sessions: Vec<SessionSummary>,
) -> (Vec<SidebarProject>, Vec<SessionSummary>) {
    let mut entries: Vec<SidebarProject> = projects
        .into_iter()
        .map(|project| SidebarProject {
            project,
            chats: Vec::new(),
        })
        .collect();
    let mut loose = Vec::new();
    for session in sessions {
        match entries
            .iter_mut()
            .find(|entry| Some(entry.project.id) == session.project_id)
        {
            Some(entry) => entry.chats.push(session),
            None => loose.push(session),
        }
    }
    (entries, loose)
}

/// The archived part of `projects` and `sessions` (newest first).
#[must_use]
pub fn group_archived(projects: Vec<Project>, sessions: Vec<SessionSummary>) -> Archived {
    let (mut entries, mut loose) = split(projects, sessions);
    for entry in entries.iter_mut().filter(|entry| !entry.project.archived) {
        loose.extend(entry.chats.extract_if(.., |chat| chat.archived));
    }
    entries.retain(|entry| entry.project.archived);
    loose.retain(|chat| chat.archived);
    loose.sort_by_key(|chat| std::cmp::Reverse(chat.updated_at));
    Archived {
        projects: entries,
        chats: loose,
    }
}

/// Groups the live `sessions` (newest first) by project. A session of a
/// project that does not exist is listed as standalone; archived chats, and
/// the chats of an archived project, are not listed.
#[must_use]
pub fn group(projects: Vec<Project>, sessions: Vec<SessionSummary>) -> Sidebar {
    let (mut grouped, mut standalone) = split(projects, sessions);
    grouped.retain(|entry| !entry.project.archived);
    standalone.retain(|chat| !chat.archived);
    // Pinned first; the sort is stable, so the rest stays newest first.
    standalone.sort_by_key(|chat| !chat.pinned);
    for entry in &mut grouped {
        entry.chats.retain(|chat| !chat.archived);
        entry.chats.sort_by_key(|chat| !chat.pinned);
    }
    let newest = |entry: &SidebarProject| entry.chats.iter().map(|chat| chat.updated_at).max();
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
/// named after the directory. Archived projects are not considered.
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
    let first = |project: &&Project| !project.archived && project.roots.first() == Some(&wanted);
    let any = |project: &&Project| !project.archived && project.roots.contains(&wanted);
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
    use crate::ports::store::{OrganizeStorePort as _, ProjectStorePort as _, StoredSession};
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
            archived: false,
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
        // An archived project is not where a directory lands: a new one is.
        store.set_project_archived(by_first.id, true).await.unwrap();
        let landed = project_for_directory(&store, "/w/api").await.unwrap();
        assert_ne!(landed.id, by_first.id);
        assert_eq!(
            project_for_directory(&store, "/w/api").await.unwrap().id,
            landed.id
        );
    }

    fn flagged(chat: SessionSummary, pinned: bool, archived: bool) -> SessionSummary {
        SessionSummary {
            pinned,
            archived,
            ..chat
        }
    }

    #[test]
    fn pinned_chats_lead_and_archived_ones_move_to_the_archived_section() {
        let live = project("live", false);
        let shelf = Project {
            archived: true,
            ..project("shelf", false)
        };
        let sessions = vec![
            summary("new", 90, Some(live.id)),
            flagged(summary("shelved", 80, Some(live.id)), false, true),
            flagged(summary("pinned-old", 10, Some(live.id)), true, false),
            summary("in-shelf", 70, Some(shelf.id)),
            flagged(
                summary("in-shelf-archived", 60, Some(shelf.id)),
                false,
                true,
            ),
            flagged(summary("loose-archived", 50, None), false, true),
            summary("loose-new", 40, None),
            flagged(summary("loose-pinned", 5, None), true, false),
        ];
        let projects = vec![live, shelf];
        let shown = group(projects.clone(), sessions.clone());
        let kept = group_archived(projects, sessions);
        // (what the sidebar lists, what the Archived section lists)
        let got = (
            shown
                .projects
                .iter()
                .map(|p| p.project.name.as_str())
                .collect::<Vec<_>>(),
            titles(&shown.projects[0].chats),
            titles(&shown.chats),
            kept.projects
                .iter()
                .map(|p| titles(&p.chats))
                .collect::<Vec<_>>(),
            titles(&kept.chats),
            kept.count(),
        );
        assert_eq!(
            got,
            (
                vec!["live"],
                vec!["pinned-old", "new"],
                vec!["loose-pinned", "loose-new"],
                vec![vec!["in-shelf", "in-shelf-archived"]],
                vec!["shelved", "loose-archived"],
                3,
            )
        );
    }
}
