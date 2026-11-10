//! `list_all`: every session with its project, as the sidebar groups them.

use std::sync::Arc;

use application_core::ports::store::{SessionStorePort, SessionSummary};
use application_testing::system::InMemorySessionStore;
use domain::conversation::ProjectId;

use super::SqliteSessionStore;
use super::tests::{port, sample};

/// Every session, newest first, each with its project; the fake agrees.
#[tokio::test]
async fn list_all_carries_the_project_and_matches_the_fake() {
    let sqlite = port(SqliteSessionStore::open_in_memory().unwrap());
    let fake: Arc<dyn SessionStorePort> = Arc::new(InMemorySessionStore::new());
    let mut ids = Vec::new();
    for store in [&sqlite, &fake] {
        let project = store.create_project("p", &["/a".into()]).await.unwrap();
        ids.push(project.id);
        for (root, at, member) in [
            ("/a", 5, true),
            ("", 9, false),
            ("/b", 7, false),
            ("/a", 5, true),
        ] {
            let mut s = sample(root, at, 0);
            s.session.project_id = member.then_some(project.id);
            store.save(s).await.unwrap();
        }
    }
    let all = sqlite.list_all().await.unwrap();
    let shape = |all: &[SessionSummary], project: ProjectId| -> Vec<(u64, bool)> {
        all.iter()
            .map(|s| (s.updated_at, s.project_id == Some(project)))
            .collect()
    };
    assert_eq!(
        shape(&all, ids[0]),
        [(9, false), (7, false), (5, true), (5, true)]
    );
    assert_eq!(
        shape(&fake.list_all().await.unwrap(), ids[1]),
        shape(&all, ids[0])
    );
}
