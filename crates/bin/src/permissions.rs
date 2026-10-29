//! The project permission rules on disk: `<workspace>/.crest/
//! permissions.local.toml`, read and written for
//! [`PermissionStorePort`].
//!
//! Parsing and writing the text live in
//! [`application::ports::permissions`]; this module only moves it between
//! memory and the file, off the async threads. A write goes to a temporary
//! file next to the real one and is renamed over it, so a crash never
//! leaves half a file.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use application::ports::permissions::{
    LoadedRules, PermissionStoreError, PermissionStorePort, parse_permissions_toml,
    permissions_file, permissions_toml,
};
use application::usecases::approval::SharedPermissions;
use application::usecases::permissions::ProjectRules;
use application::usecases::resume::restore_permission_rules;
use async_trait::async_trait;
use domain::permission::PermissionRule;

/// [`PermissionStorePort`] over the workspace's `.crest` directory.
#[derive(Debug, Clone, Copy, Default)]
pub struct FilePermissionStore;

#[async_trait]
impl PermissionStorePort for FilePermissionStore {
    async fn load(&self, workspace: &Path) -> Result<LoadedRules, PermissionStoreError> {
        let path = permissions_file(workspace);
        tokio::task::spawn_blocking(move || read_rules(&path))
            .await
            .map_err(|err| PermissionStoreError(format!("cannot read the rules: {err}")))?
    }

    async fn save(
        &self,
        workspace: &Path,
        rules: &[PermissionRule],
    ) -> Result<(), PermissionStoreError> {
        let path = permissions_file(workspace);
        let text = permissions_toml(rules);
        tokio::task::spawn_blocking(move || write_atomically(&path, &text))
            .await
            .map_err(|err| PermissionStoreError(format!("cannot write the rules: {err}")))?
    }
}

/// The rules in `path`; none when it does not exist.
fn read_rules(path: &Path) -> Result<LoadedRules, PermissionStoreError> {
    let file = path.display().to_string();
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(parse_permissions_toml(&text, &file)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(LoadedRules::default()),
        Err(err) => Err(PermissionStoreError(format!(
            "{file}: cannot read the file: {err}"
        ))),
    }
}

/// Writes `text` to `path` through a temporary file in the same directory,
/// creating the directory when missing.
fn write_atomically(path: &Path, text: &str) -> Result<(), PermissionStoreError> {
    let fail = |what: &str, err: std::io::Error| {
        PermissionStoreError(format!("{}: cannot {what}: {err}", path.display()))
    };
    let dir = path
        .parent()
        .ok_or_else(|| PermissionStoreError(format!("{}: no parent folder", path.display())))?;
    std::fs::create_dir_all(dir).map_err(|err| fail("create the folder", err))?;
    let temp = temp_path(path);
    let written = std::fs::File::create(&temp).and_then(|mut file| {
        file.write_all(text.as_bytes())?;
        file.sync_all()
    });
    if let Err(err) = written.and_then(|()| std::fs::rename(&temp, path)) {
        // Best effort: the temporary file is only litter now.
        let _ = std::fs::remove_file(&temp);
        return Err(fail("write the file", err));
    }
    Ok(())
}

/// `.<name>.<pid>.tmp` next to `path`.
fn temp_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    path.with_file_name(format!(".{name}.{}.tmp", std::process::id()))
}

/// The project rules of `workspace`, through the file store, and the
/// warnings to show at startup. The loaded rules are added to
/// `permissions` (next to the restored session rules).
pub async fn open_project_rules(
    workspace: &Path,
    permissions: &SharedPermissions,
) -> (ProjectRules, Vec<String>) {
    let project = ProjectRules::new(Arc::new(FilePermissionStore), workspace);
    let warnings = match project.load().await {
        Ok(loaded) => {
            restore_permission_rules(permissions, &loaded.rules);
            loaded.warnings
        }
        Err(err) => vec![format!("project permission rules not loaded: {err}")],
    };
    (project, warnings)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use domain::permission::{
        Decision, PermissionMode, PermissionPolicy, RuleDecision, RuleScope, ToolEffect,
    };
    use domain::permission::ActionRequest;

    use super::*;

    fn rule(text: &str, decision: RuleDecision) -> PermissionRule {
        PermissionRule::parse(text, decision, RuleScope::Project).unwrap()
    }

    #[tokio::test]
    async fn a_missing_file_is_no_rules() {
        let dir = tempfile::tempdir().unwrap();
        let loaded = FilePermissionStore.load(dir.path()).await.unwrap();
        assert_eq!(loaded, LoadedRules::default());
        assert!(!dir.path().join(".crest").exists(), "loading creates nothing");
    }

    #[tokio::test]
    async fn saved_rules_load_back_and_crest_is_created() {
        let dir = tempfile::tempdir().unwrap();
        let rules = vec![
            rule("run_command(cargo test *)", RuleDecision::AllowAlways),
            rule("run_command(git push *)", RuleDecision::Deny),
        ];
        FilePermissionStore.save(dir.path(), &rules).await.unwrap();
        let file = permissions_file(dir.path());
        assert!(file.is_file());
        let loaded = FilePermissionStore.load(dir.path()).await.unwrap();
        assert_eq!(loaded.rules, rules);
        assert!(loaded.warnings.is_empty());
        // Saving again replaces the file and leaves no temporary file.
        FilePermissionStore
            .save(dir.path(), &rules[..1])
            .await
            .unwrap();
        let entries: Vec<_> = std::fs::read_dir(dir.path().join(".crest"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(entries, ["permissions.local.toml"]);
        let loaded = FilePermissionStore.load(dir.path()).await.unwrap();
        assert_eq!(loaded.rules, rules[..1]);
    }

    #[tokio::test]
    async fn a_file_with_problems_is_loaded_and_not_rewritten() {
        let dir = tempfile::tempdir().unwrap();
        let file = permissions_file(dir.path());
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        let text = "allow = [\"run_command(cargo test *)\", \"bad(\"]\n";
        std::fs::write(&file, text).unwrap();
        let permissions = Arc::new(Mutex::new(PermissionPolicy::new(PermissionMode::Ask)));
        let (project, warnings) = open_project_rules(dir.path(), &permissions).await;
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("allow #2"), "{warnings:?}");
        let cargo = ActionRequest::new("run_command", ToolEffect::Execute, "cargo test -p ui")
            .with_argv(vec!["cargo".into(), "test".into(), "-p".into(), "ui".into()]);
        assert_eq!(
            permissions.lock().unwrap().decide(&cargo),
            Decision::Allow,
            "the good entry is in force"
        );
        let added = project
            .add(&rule("edit_file(src/*)", RuleDecision::AllowAlways))
            .await;
        assert!(added.is_err(), "{added:?}");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), text);
    }
}
