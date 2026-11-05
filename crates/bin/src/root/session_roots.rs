//! The directories a session opens with.
//!
//! The directory crest was opened in is always the first root. A session in
//! a project adds the project's other directories, read from the store when
//! the session opens; a session in none keeps just that one. The list is
//! fixed for the open session: a directory added to or removed from the
//! project later is seen the next time the session opens.

use std::path::Path;
use std::sync::{Arc, Mutex};

use adapter_fs::guard::canonical_root;
use application::ports::store::SessionStorePort;
use domain::conversation::Session;
use domain::permission::PermissionPolicy;
use domain::roots::WorkspaceRoots;
use domain::workspace::WorkspaceRoot;
use ui::timeline::TimelineState;

use super::Config;
use super::stores::{Resumed, resume_session};

/// What a run opens a session with.
pub(super) struct Opened {
    /// The permission rules, with the ones the session saved restored.
    pub(super) permissions: application::usecases::run_turn::SharedPermissions,
    /// The session, its timeline restored into the caller's, and its history.
    pub(super) resumed: Resumed,
    /// The directories it works in.
    pub(super) roots: WorkspaceRoots,
    /// What went wrong on the way; the session opened anyway.
    pub(super) warnings: Vec<String>,
}

/// Opens the session of this run in `root` and the directories it has.
pub(super) async fn open_session(
    store: Option<&Arc<dyn SessionStorePort>>,
    config: &Config,
    root: &WorkspaceRoot,
    timeline: &Mutex<TimelineState>,
) -> Opened {
    let permissions = Arc::new(Mutex::new(PermissionPolicy::new(config.mode)));
    let mut warnings = Vec::new();
    let resumed = resume_session(store, config, root, timeline, &permissions, &mut warnings).await;
    let roots = session_roots(store, &resumed.session, root, &mut warnings).await;
    Opened {
        permissions,
        resumed,
        roots,
        warnings,
    }
}

/// The roots of `session`, opened in `first`. A project directory that is
/// gone, or a store that cannot be read, becomes a warning and leaves that
/// directory out; the session still opens.
async fn session_roots(
    store: Option<&Arc<dyn SessionStorePort>>,
    session: &Session,
    first: &WorkspaceRoot,
    warnings: &mut Vec<String>,
) -> WorkspaceRoots {
    let mut roots = vec![first.clone()];
    if let (Some(store), Some(project)) = (store, session.project_id) {
        match store.projects().await {
            Ok(all) => {
                let listed = all.iter().find(|p| p.id == project).map(|p| &p.roots);
                for dir in listed.into_iter().flatten() {
                    match canonical_root(Path::new(dir)) {
                        Ok(root) => roots.push(root),
                        Err(err) => warnings.push(format!(
                            "the project directory {dir} is not available: {err}"
                        )),
                    }
                }
            }
            Err(err) => warnings.push(format!("the project's directories were not read: {err}")),
        }
    }
    WorkspaceRoots::new(roots)
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use application::ports::store::ProjectStorePort;
    use application_testing::system::InMemorySessionStore;

    use super::*;

    fn dir(parent: &Path, name: &str) -> String {
        let path = parent.join(name);
        std::fs::create_dir(&path).unwrap();
        std::fs::canonicalize(path).unwrap().display().to_string()
    }

    #[tokio::test]
    async fn a_project_session_gets_its_project_directories_after_the_opened_one() {
        let tmp = tempfile::tempdir().unwrap();
        let (api, web, docs) = (
            dir(tmp.path(), "api"),
            dir(tmp.path(), "web"),
            dir(tmp.path(), "docs"),
        );
        let gone = tmp.path().join("gone").display().to_string();
        let store = Arc::new(InMemorySessionStore::new());
        let project = store
            .create_project("p", &[web.clone(), gone, docs.clone(), api.clone()])
            .await
            .unwrap();
        let store: Arc<dyn SessionStorePort> = store;
        let first = canonical_root(Path::new(&api)).unwrap();
        let mut in_project = Session::new(api.clone(), 1);
        in_project.project_id = Some(project.id);
        let standalone = Session::new(api.clone(), 1);

        let mut warnings = Vec::new();
        let got = session_roots(Some(&store), &in_project, &first, &mut warnings).await;
        let paths: Vec<String> = got
            .iter()
            .map(|r| r.as_path().display().to_string())
            .collect();
        // Opened directory first; the project's others in its order; the one
        // listed twice (the opened one) once; the missing one is a warning.
        assert_eq!(paths, [api, web, docs]);
        assert_eq!(warnings.len(), 1, "{warnings:?}");

        let mut warnings = Vec::new();
        let alone = session_roots(Some(&store), &standalone, &first, &mut warnings).await;
        assert_eq!(alone.len(), 1);
        assert!(warnings.is_empty());
    }
}
