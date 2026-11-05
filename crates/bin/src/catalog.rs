//! The chats sidebar's rows from what the store grouped.

use application::ports::store::SessionSummary;
use application::usecases::sidebar::Sidebar;
use ui::chats::{Catalog, ChatEntry, ProjectChats, chat_label};

/// The sidebar's rows: each project with its first directory, how many it
/// has and its chats, then the chats in none.
pub(crate) fn catalog_of(sidebar: Sidebar) -> Catalog {
    let entry = |chat: SessionSummary| ChatEntry {
        id: chat.id.to_string(),
        title: chat_label(chat.title.as_deref()),
    };
    Catalog {
        projects: sidebar
            .projects
            .into_iter()
            .map(|listed| ProjectChats {
                id: listed.project.id.to_string(),
                name: listed.project.name,
                root: listed.project.roots.first().cloned().unwrap_or_default(),
                dirs: listed.project.roots.len(),
                chats: listed.chats.into_iter().map(entry).collect(),
            })
            .collect(),
        chats: sidebar.chats.into_iter().map(entry).collect(),
    }
}

#[cfg(test)]
mod tests {
    use application::ports::store::{ProjectStorePort as _, SessionStorePort as _, StoredSession};
    use application::usecases::sidebar::sidebar;
    use application_testing::system::InMemorySessionStore;
    use domain::conversation::Session;

    use super::*;

    #[tokio::test]
    async fn the_store_fills_the_rows_with_projects_then_chats_with_none() {
        let store = InMemorySessionStore::new();
        let project = store
            .create_project("api", &["/w/api".into(), "/w/web".into()])
            .await
            .unwrap();
        let save = |title: Option<&str>, project: Option<_>, at: u64| {
            let mut session = Session::new("/w/api", at);
            session.title = title.map(str::to_owned);
            session.project_id = project;
            session.updated_at = at;
            StoredSession::new(session)
        };
        store
            .save(save(Some("in api"), Some(project.id), 5))
            .await
            .unwrap();
        store.save(save(None, None, 9)).await.unwrap();
        let catalog = catalog_of(sidebar(&store).await.unwrap());
        let projects: Vec<_> = catalog
            .projects
            .iter()
            .map(|p| {
                (
                    p.name.as_str(),
                    p.root.as_str(),
                    p.dirs,
                    p.chats[0].title.as_str(),
                )
            })
            .collect();
        assert_eq!(projects, [("api", "/w/api", 2, "in api")]);
        assert_eq!(catalog.chats[0].title, "New chat");
    }
}
