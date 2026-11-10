//! The menus of the sidebar, run in the store: one call per row, through
//! the application's `organize` use cases, so this holds no rules. A
//! refusal comes back as the store's message, which is fit to show.

use application::ports::store::{SessionStorePort, StoreError};
use application::usecases::organize::{
    ChatAction, ChatDone, ProjectAction, ProjectDone, chat_action, project_action, promote_chat,
};
use domain::conversation::{ProjectId, SessionId};
use domain::project::name_from_root;
use ui::chats::{ChatOp, Organize, Organized, ProjectOp};

/// Runs `change` on `store`.
pub(crate) async fn run(
    store: &dyn SessionStorePort,
    change: Organize,
) -> Result<Organized, String> {
    match change {
        Organize::Chat { id, op } => chat(store, &id, op).await,
        Organize::Project { id, op } => project(store, &id, op).await,
    }
}

fn shown(err: StoreError) -> String {
    err.to_string()
}

/// The chat `id`, as the sidebar printed it.
fn session_id(id: &str) -> Result<SessionId, String> {
    SessionId::parse(id).ok_or_else(|| "that chat is not saved yet".to_owned())
}

/// The project `id`, as the sidebar printed it.
fn project_id(id: &str) -> Result<ProjectId, String> {
    ProjectId::parse(id).ok_or_else(|| "that project is gone".to_owned())
}

/// The store's action of a menu row; "New project..." is not one, it is
/// done by [`promote_chat`].
fn action_of(op: ChatOp) -> Result<Option<ChatAction>, String> {
    Ok(Some(match op {
        ChatOp::Pin(pinned) => ChatAction::Pin(pinned),
        ChatOp::Archive(archived) => ChatAction::Archive(archived),
        ChatOp::Branch => ChatAction::Branch,
        ChatOp::Move(project) => ChatAction::Move(project.as_deref().map(project_id).transpose()?),
        ChatOp::Promote(_) => return Ok(None),
    }))
}

async fn chat(store: &dyn SessionStorePort, id: &str, op: ChatOp) -> Result<Organized, String> {
    let id = session_id(id)?;
    if let ChatOp::Promote(directory) = &op {
        let name = name_from_root(directory);
        promote_chat(store, id, &name, directory)
            .await
            .map_err(shown)?;
        return Ok(Organized::Done);
    }
    let action = action_of(op)?.ok_or("nothing to do")?;
    match chat_action(store, id, action).await.map_err(shown)? {
        ChatDone::Changed => Ok(Organized::Done),
        ChatDone::Branched(branch) => {
            let stored = store.load(branch).await.map_err(shown)?;
            Ok(Organized::Branch {
                id: branch.to_string(),
                root: stored.session.workspace_root,
            })
        }
    }
}

async fn project(
    store: &dyn SessionStorePort,
    id: &str,
    op: ProjectOp,
) -> Result<Organized, String> {
    let id = project_id(id)?;
    let projects = store.projects().await.map_err(shown)?;
    let found = projects.iter().find(|project| project.id == id);
    let found = found.ok_or_else(|| "that project is gone".to_owned())?;
    let action = match op {
        ProjectOp::Rename(name) => ProjectAction::Rename(name),
        ProjectOp::Pin(pinned) => ProjectAction::Pin(pinned),
        ProjectOp::AddDirectory(path) => ProjectAction::AddDirectory(path),
        ProjectOp::RemoveDirectory(path) => ProjectAction::RemoveDirectory(path),
        ProjectOp::Archive(archived) => ProjectAction::Archive(archived),
        ProjectOp::Delete => ProjectAction::Delete,
    };
    match project_action(store, found, action).await.map_err(shown)? {
        ProjectDone::Updated(_) => Ok(Organized::Done),
        ProjectDone::Deleted { chats_freed } => Ok(Organized::Freed(chats_freed)),
    }
}

#[cfg(test)]
mod tests {
    use application::ports::store::{ProjectStorePort as _, StoredSession};
    use application::usecases::sidebar::{archived, sidebar};
    use application_testing::system::InMemorySessionStore;
    use domain::conversation::Session;

    use super::*;

    /// `loose chats [projects name[dirs]:chats] archived`.
    async fn state(store: &InMemorySessionStore) -> String {
        let (shown, kept) = (
            sidebar(store).await.unwrap(),
            archived(store).await.unwrap(),
        );
        let projects: Vec<String> = shown
            .projects
            .iter()
            .map(|p| {
                let (name, dirs) = (&p.project.name, p.project.roots.len());
                format!("{name}[{dirs}]:{}", p.chats.len())
            })
            .collect();
        format!("{} {projects:?} {}", shown.chats.len(), kept.count())
    }

    /// A store with one chat in no project, and that chat's id.
    async fn one_chat() -> (InMemorySessionStore, String) {
        let store = InMemorySessionStore::new();
        let mut session = Session::new("", 1);
        session.title = Some("Fix".into());
        let id = session.id.to_string();
        store.save(StoredSession::new(session)).await.unwrap();
        (store, id)
    }

    /// A store with the project `api` holding one chat, and the ids of both.
    async fn one_project() -> (InMemorySessionStore, String, String) {
        let (store, id) = one_chat().await;
        let promoted = chat(&id, ChatOp::Promote("/w/api".into()));
        run(&store, promoted).await.unwrap();
        let project = store.projects().await.unwrap()[0].id.to_string();
        (store, id, project)
    }

    fn chat(id: &str, op: ChatOp) -> Organize {
        Organize::Chat { id: id.into(), op }
    }

    fn project(id: &str, op: ProjectOp) -> Organize {
        Organize::Project { id: id.into(), op }
    }

    #[tokio::test]
    async fn the_rows_of_a_chats_menu_reach_the_store() {
        let (store, id) = one_chat().await;
        let mut log = vec![state(&store).await];
        // New project from the chat: named after its directory.
        let promoted = run(&store, chat(&id, ChatOp::Promote("/w/api".into()))).await;
        log.push(format!("{promoted:?} {}", state(&store).await));
        // Archive and restore; a branch lands in the same project.
        for op in [
            ChatOp::Archive(true),
            ChatOp::Archive(false),
            ChatOp::Pin(true),
        ] {
            run(&store, chat(&id, op)).await.unwrap();
            log.push(state(&store).await);
        }
        let Ok(Organized::Branch { root, .. }) = run(&store, chat(&id, ChatOp::Branch)).await
        else {
            panic!("a branch is reported");
        };
        log.push(format!("branch opens in {root}"));
        log.push(state(&store).await);
        // What cannot be done says why.
        let unsaved = run(&store, chat("not an id", ChatOp::Branch)).await;
        let gone = run(&store, chat(&id, ChatOp::Move(Some("nope".into())))).await;
        log.extend([format!("{unsaved:?}"), format!("{gone:?}")]);

        assert_eq!(log.len(), 9);
        assert_eq!(log[0], r#"1 [] 0"#);
        assert_eq!(log[1], r#"Ok(Done) 0 ["api[1]:1"] 0"#);
        assert_eq!(
            &log[2..5],
            [
                r#"0 ["api[1]:0"] 1"#,
                r#"0 ["api[1]:1"] 0"#,
                r#"0 ["api[1]:1"] 0"#
            ]
        );
        assert_eq!(log[5], "branch opens in /w/api");
        assert_eq!(log[6], r#"0 ["api[1]:2"] 0"#);
        assert_eq!(log[7], r#"Err("that chat is not saved yet")"#);
        assert_eq!(log[8], r#"Err("that project is gone")"#);
    }

    #[tokio::test]
    async fn the_rows_of_a_projects_menu_reach_the_store() {
        let (store, _, project_id) = one_project().await;
        let on = |op| project(&project_id, op);
        let mut log = Vec::new();
        // Directories: add one, remove it; the last one stays.
        let steps = [
            on(ProjectOp::AddDirectory("/w/lib".into())),
            on(ProjectOp::RemoveDirectory("/w/lib".into())),
            on(ProjectOp::RemoveDirectory("/w/api".into())),
            on(ProjectOp::Rename("service".into())),
            on(ProjectOp::Archive(true)),
            on(ProjectOp::Archive(false)),
            on(ProjectOp::Delete),
            on(ProjectOp::Pin(true)),
        ];
        for step in steps {
            let done = run(&store, step).await;
            log.push(format!("{done:?} {}", state(&store).await));
        }
        assert_eq!(
            log,
            [
                r#"Ok(Done) 0 ["api[2]:1"] 0"#,
                r#"Ok(Done) 0 ["api[1]:1"] 0"#,
                r#"Err("a project keeps at least one directory") 0 ["api[1]:1"] 0"#,
                r#"Ok(Done) 0 ["service[1]:1"] 0"#,
                r#"Ok(Done) 0 [] 1"#,
                r#"Ok(Done) 0 ["service[1]:1"] 0"#,
                r#"Ok(Freed(1)) 1 [] 0"#,
                r#"Err("that project is gone") 1 [] 0"#,
            ]
        );
    }
}
