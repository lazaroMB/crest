//! The chats sidebar's rows from what the store grouped.

use std::collections::HashMap;

use application::ports::store::SessionSummary;
use application::usecases::sidebar::{Archived, Sidebar, SidebarProject};
use domain::conversation::SessionId;
use ui::chats::{Archive, ArchivedProject, Branch, Catalog, ChatEntry, ProjectChats, chat_label};

/// The titles of every chat listed, by id, archived ones too: a branch
/// names the chat it came from by looking it up here.
type Titles = HashMap<SessionId, String>;

fn titles_of(sidebar: &Sidebar, archived: &Archived) -> Titles {
    let in_projects = |projects: &'_ [SidebarProject]| -> Vec<SessionSummary> {
        projects.iter().flat_map(|p| p.chats.clone()).collect()
    };
    let (live, kept) = (
        in_projects(&sidebar.projects),
        in_projects(&archived.projects),
    );
    live.iter()
        .chain(&sidebar.chats)
        .chain(&kept)
        .chain(&archived.chats)
        .map(|chat| (chat.id, chat_label(chat.title.as_deref())))
        .collect()
}

/// One row: its title, pin and, for a branch, where it came from (empty
/// once that chat was deleted).
fn entry_of(chat: SessionSummary, titles: &Titles) -> ChatEntry {
    let from = |id: SessionId| titles.get(&id).cloned();
    ChatEntry {
        id: chat.id.to_string(),
        title: chat_label(chat.title.as_deref()),
        pinned: chat.pinned,
        branch: chat.branched_at.map(|after| Branch {
            from: chat.branched_from.and_then(from).unwrap_or_default(),
            after,
        }),
    }
}

fn project_of(listed: SidebarProject, titles: &Titles) -> ProjectChats {
    let project = listed.project;
    ProjectChats {
        id: project.id.to_string(),
        name: project.name,
        root: project.roots.first().cloned().unwrap_or_default(),
        roots: project.roots,
        pinned: project.pinned,
        chats: listed
            .chats
            .into_iter()
            .map(|c| entry_of(c, titles))
            .collect(),
    }
}

fn archived_project_of(listed: SidebarProject) -> ArchivedProject {
    ArchivedProject {
        id: listed.project.id.to_string(),
        name: listed.project.name,
        chats: listed.chats.len(),
    }
}

/// The sidebar's rows: each project with its directories and chats, then
/// the chats in none, and what is archived.
pub(crate) fn catalog_of(sidebar: Sidebar, archived: Archived) -> Catalog {
    let titles = titles_of(&sidebar, &archived);
    let entries = |chats: Vec<SessionSummary>| -> Vec<ChatEntry> {
        chats
            .into_iter()
            .map(|chat| entry_of(chat, &titles))
            .collect()
    };
    Catalog {
        projects: sidebar
            .projects
            .into_iter()
            .map(|listed| project_of(listed, &titles))
            .collect(),
        chats: entries(sidebar.chats),
        archived: Archive {
            projects: archived
                .projects
                .into_iter()
                .map(archived_project_of)
                .collect(),
            chats: entries(archived.chats),
        },
    }
}

#[cfg(test)]
mod tests {
    use application::ports::store::{
        OrganizeStorePort as _, ProjectStorePort as _, SessionStorePort as _, StoredSession,
    };
    use application::usecases::sidebar::{archived, sidebar};
    use application_testing::system::InMemorySessionStore;
    use domain::conversation::{ProjectId, Session};

    use super::*;

    async fn chat(
        store: &InMemorySessionStore,
        title: Option<&str>,
        project: Option<ProjectId>,
        at: u64,
    ) -> SessionId {
        let mut session = Session::new("/w/api", at);
        session.title = title.map(str::to_owned);
        session.project_id = project;
        session.updated_at = at;
        let id = session.id;
        store.save(StoredSession::new(session)).await.unwrap();
        id
    }

    async fn catalog(store: &InMemorySessionStore) -> Catalog {
        catalog_of(
            sidebar(store).await.unwrap(),
            archived(store).await.unwrap(),
        )
    }

    /// `name:first dir:dirs:pinned[title(pin)(branch)...]`, then `loose[...]`,
    /// then `archived[...]`.
    fn shown(catalog: &Catalog) -> String {
        let chats = |chats: &[ChatEntry]| -> String {
            chats
                .iter()
                .map(|chat| {
                    let pin = if chat.pinned { "*" } else { "" };
                    let branch = chat
                        .branch
                        .as_ref()
                        .map_or(String::new(), |b| format!("<{:?}@{}", b.from, b.after));
                    format!("{}{pin}{branch}", chat.title)
                })
                .collect::<Vec<_>>()
                .join(",")
        };
        let projects: Vec<String> = catalog
            .projects
            .iter()
            .map(|p| {
                format!(
                    "{}:{}:{}:{}[{}]",
                    p.name,
                    p.root,
                    p.dirs(),
                    p.pinned,
                    chats(&p.chats)
                )
            })
            .collect();
        let kept: Vec<String> = catalog
            .archived
            .projects
            .iter()
            .map(|p| format!("{}({})", p.name, p.chats))
            .collect();
        format!(
            "{} loose[{}] archived[{} {}]",
            projects.join(" "),
            chats(&catalog.chats),
            kept.join(" "),
            chats(&catalog.archived.chats)
        )
    }

    #[tokio::test]
    async fn the_store_fills_the_rows_with_projects_chats_in_none_and_the_archive() {
        let store = InMemorySessionStore::new();
        let api = store
            .create_project("api", &["/w/api".into(), "/w/web".into()])
            .await
            .unwrap();
        let old = store
            .create_project("old", &["/w/old".into()])
            .await
            .unwrap();
        let source = chat(&store, Some("in api"), Some(api.id), 5).await;
        let loose = chat(&store, None, None, 9).await;
        let kept = chat(&store, Some("kept"), None, 3).await;
        chat(&store, Some("in old"), Some(old.id), 2).await;
        store.pin_project(api.id, true).await.unwrap();
        store.pin_session(source, true).await.unwrap();
        store.set_session_archived(kept, true).await.unwrap();
        store.set_project_archived(old.id, true).await.unwrap();
        let mut log = vec![shown(&catalog(&store).await)];

        // A branch names its source; once the source is gone it names none.
        let branch = store.branch_session(source).await.unwrap();
        log.push(shown(&catalog(&store).await));
        store.delete(source).await.unwrap();
        log.push(shown(&catalog(&store).await));
        store.set_session_archived(loose, true).await.unwrap();
        store.set_session_archived(branch, false).await.unwrap();
        log.push(shown(&catalog(&store).await));

        assert_eq!(
            log,
            [
                r#"api:/w/api:2:true[in api*] loose[New chat] archived[old(1) kept]"#,
                r#"api:/w/api:2:true[in api*,in api (branch)<"in api"@0] loose[New chat] archived[old(1) kept]"#,
                r#"api:/w/api:2:true[in api (branch)<""@0] loose[New chat] archived[old(1) kept]"#,
                r#"api:/w/api:2:true[in api (branch)<""@0] loose[] archived[old(1) New chat,kept]"#,
            ]
        );
    }
}
