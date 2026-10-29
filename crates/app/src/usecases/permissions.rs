//! Seeing and changing the permission rules: the session's, in the shared
//! [`PermissionPolicy`](domain::permission::PermissionPolicy), and the
//! project's, which are also in the policy (loaded when the workspace
//! opens) and in its `permissions.local.toml`.
//!
//! Behind `/permissions` and the "always in project" answer of the approval
//! dialog.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use domain::permission::{PermissionRule, RuleDecision, RuleScope};

use super::approval::{SharedPermissions, lock_permissions};
use crate::ports::permissions::{
    LoadedRules, PermissionStoreError, PermissionStorePort, permissions_file,
};

/// The project rules of one workspace: its store and its root.
#[derive(Clone)]
pub struct ProjectRules {
    store: Arc<dyn PermissionStorePort>,
    workspace: PathBuf,
}

impl fmt::Debug for ProjectRules {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProjectRules")
            .field("workspace", &self.workspace)
            .finish_non_exhaustive()
    }
}

impl ProjectRules {
    /// The rules of `workspace`, kept by `store`.
    #[must_use]
    pub fn new(store: Arc<dyn PermissionStorePort>, workspace: impl Into<PathBuf>) -> Self {
        Self {
            store,
            workspace: workspace.into(),
        }
    }

    /// The workspace root.
    #[must_use]
    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    /// The file the rules live in.
    #[must_use]
    pub fn file(&self) -> PathBuf {
        permissions_file(&self.workspace)
    }

    /// The rules in the file, and its problems.
    ///
    /// # Errors
    ///
    /// When the file exists but cannot be read.
    pub async fn load(&self) -> Result<LoadedRules, PermissionStoreError> {
        self.store.load(&self.workspace).await
    }

    /// Adds `rule` to the file, as a project rule, unless one with the same
    /// pattern and decision is there. Returns whether the file changed.
    ///
    /// # Errors
    ///
    /// When the file cannot be read or written, or has problems: rewriting
    /// it would drop the entries crest does not understand.
    pub async fn add(&self, rule: &PermissionRule) -> Result<bool, PermissionStoreError> {
        let mut rules = self.load_clean().await?;
        if rules.iter().any(|kept| same_rule(kept, rule)) {
            return Ok(false);
        }
        rules.push(rule.clone().with_scope(RuleScope::Project));
        self.store.save(&self.workspace, &rules).await?;
        Ok(true)
    }

    /// Removes the rules with the pattern and decision of `rule` from the
    /// file. Returns whether the file changed.
    ///
    /// # Errors
    ///
    /// As [`add`](Self::add).
    pub async fn remove(&self, rule: &PermissionRule) -> Result<bool, PermissionStoreError> {
        let mut rules = self.load_clean().await?;
        let before = rules.len();
        rules.retain(|kept| !same_rule(kept, rule));
        if rules.len() == before {
            return Ok(false);
        }
        self.store.save(&self.workspace, &rules).await?;
        Ok(true)
    }

    /// The rules of a file without problems.
    async fn load_clean(&self) -> Result<Vec<PermissionRule>, PermissionStoreError> {
        let loaded = self.load().await?;
        match loaded.warnings.first() {
            None => Ok(loaded.rules),
            Some(warning) => Err(PermissionStoreError(format!(
                "{} was not rewritten because it has problems: {warning}",
                self.file().display()
            ))),
        }
    }
}

/// Same tool, matcher and decision, whatever the scope.
fn same_rule(a: &PermissionRule, b: &PermissionRule) -> bool {
    a.same_pattern(b) && a.decision == b.decision
}

/// The rules in force, grouped as `/permissions` shows them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuleListing {
    /// "Allow always" rules of this session.
    pub session: Vec<PermissionRule>,
    /// "Allow always" rules of the project.
    pub project: Vec<PermissionRule>,
    /// Denials, of the session and of the project.
    pub deny: Vec<PermissionRule>,
    /// The project file, when the workspace has project rules.
    pub project_file: Option<PathBuf>,
}

/// The rules in force, from the shared policy, in the order they were
/// added.
#[must_use]
pub fn list_rules(permissions: &SharedPermissions, project: Option<&ProjectRules>) -> RuleListing {
    let mut listing = RuleListing {
        project_file: project.map(ProjectRules::file),
        ..RuleListing::default()
    };
    for rule in &lock_permissions(permissions).rules {
        let group = match (rule.decision, rule.scope) {
            (RuleDecision::Deny, _) => &mut listing.deny,
            (RuleDecision::AllowAlways, RuleScope::Session) => &mut listing.session,
            (RuleDecision::AllowAlways, RuleScope::Project) => &mut listing.project,
        };
        group.push(rule.clone());
    }
    listing
}

/// Removes `rule` from the policy and, for a project rule, from the
/// project file.
///
/// # Errors
///
/// When a project rule cannot be removed from the file (or there is none
/// to write to); it then stays in the policy too.
pub async fn remove_rule(
    permissions: &SharedPermissions,
    project: Option<&ProjectRules>,
    rule: &PermissionRule,
) -> Result<(), PermissionStoreError> {
    if rule.scope == RuleScope::Project {
        project.ok_or_else(no_project)?.remove(rule).await?;
    }
    lock_permissions(permissions)
        .rules
        .retain(|kept| kept != rule);
    Ok(())
}

/// Turns the session rule `rule` into a project rule: writes it to the
/// project file and changes its scope in the policy, where it keeps its
/// place.
///
/// # Errors
///
/// When the file cannot be written (or there is none); the rule then stays
/// a session rule.
pub async fn promote_rule(
    permissions: &SharedPermissions,
    project: Option<&ProjectRules>,
    rule: &PermissionRule,
) -> Result<(), PermissionStoreError> {
    let promoted = rule.clone().with_scope(RuleScope::Project);
    project.ok_or_else(no_project)?.add(&promoted).await?;
    let mut policy = lock_permissions(permissions);
    let already = policy.rules.contains(&promoted);
    if already {
        policy.rules.retain(|kept| kept != rule);
    } else if let Some(kept) = policy.rules.iter_mut().find(|kept| *kept == rule) {
        *kept = promoted;
    } else {
        policy.remember(promoted);
    }
    Ok(())
}

fn no_project() -> PermissionStoreError {
    PermissionStoreError("this workspace has no project permissions file".to_owned())
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use domain::permission::{PermissionMode, PermissionPolicy};

    use super::*;
    use crate::testing::permissions::MemoryPermissionStore;

    fn rule(text: &str) -> PermissionRule {
        text.parse().unwrap()
    }

    fn deny(text: &str, scope: RuleScope) -> PermissionRule {
        PermissionRule::parse(text, RuleDecision::Deny, scope).unwrap()
    }

    fn setup(
        rules: Vec<PermissionRule>,
    ) -> (SharedPermissions, Arc<MemoryPermissionStore>, ProjectRules) {
        let mut policy = PermissionPolicy::new(PermissionMode::Ask);
        for rule in rules {
            policy.remember(rule);
        }
        let store = Arc::new(MemoryPermissionStore::new());
        let project = ProjectRules::new(store.clone(), "/w");
        (Arc::new(Mutex::new(policy)), store, project)
    }

    #[test]
    fn list_groups_rules_by_scope_and_decision() {
        let session = rule("run_command(cargo test *)");
        let in_project = rule("edit_file(src/*)").with_scope(RuleScope::Project);
        let denied = deny("run_command(git push *)", RuleScope::Project);
        let (permissions, _, project) =
            setup(vec![session.clone(), denied.clone(), in_project.clone()]);
        assert_eq!(
            list_rules(&permissions, Some(&project)),
            RuleListing {
                session: vec![session],
                project: vec![in_project],
                deny: vec![denied],
                project_file: Some(PathBuf::from("/w/.crest/permissions.local.toml")),
            }
        );
        assert_eq!(list_rules(&permissions, None).project_file, None);
    }

    #[tokio::test]
    async fn promote_moves_a_session_rule_to_the_project_file() {
        let session = rule("run_command(cargo test *)");
        let (permissions, store, project) = setup(vec![rule("ls"), session.clone()]);
        promote_rule(&permissions, Some(&project), &session)
            .await
            .unwrap();
        let promoted = session.clone().with_scope(RuleScope::Project);
        assert_eq!(
            lock_permissions(&permissions).rules,
            [rule("ls"), promoted.clone()]
        );
        assert_eq!(store.rules(Path::new("/w")), [promoted.clone()]);
        // Promoting again does not duplicate the entry.
        lock_permissions(&permissions).remember(session.clone());
        promote_rule(&permissions, Some(&project), &session)
            .await
            .unwrap();
        assert_eq!(
            lock_permissions(&permissions).rules,
            [rule("ls"), promoted.clone()]
        );
        assert_eq!(store.rules(Path::new("/w")), [promoted]);
    }

    #[tokio::test]
    async fn promote_failure_keeps_the_session_rule() {
        let session = rule("run_command(cargo test *)");
        let (permissions, store, project) = setup(vec![session.clone()]);
        store.fail_saves(true);
        let err = promote_rule(&permissions, Some(&project), &session)
            .await
            .unwrap_err();
        assert!(err.0.contains("disk full"), "{err}");
        assert_eq!(lock_permissions(&permissions).rules, [session.clone()]);
        assert!(
            promote_rule(&permissions, None, &session).await.is_err(),
            "no project file"
        );
    }

    #[tokio::test]
    async fn remove_takes_rules_out_of_the_policy_and_the_file() {
        let session = rule("run_command(cargo test *)");
        let denied = deny("run_command(git push *)", RuleScope::Project);
        let (permissions, store, project) = setup(vec![session.clone(), denied.clone()]);
        store.set_text(
            Path::new("/w"),
            "allow = [\"edit_file(src/*)\"]\ndeny = [\"run_command(git push *)\"]\n",
        );
        remove_rule(&permissions, Some(&project), &denied)
            .await
            .unwrap();
        assert_eq!(lock_permissions(&permissions).rules, [session.clone()]);
        assert_eq!(
            store.rules(Path::new("/w")),
            [rule("edit_file(src/*)").with_scope(RuleScope::Project)]
        );
        // A session rule never touches the file.
        store.fail_saves(true);
        remove_rule(&permissions, None, &session).await.unwrap();
        assert!(lock_permissions(&permissions).rules.is_empty());
    }

    #[tokio::test]
    async fn a_file_with_problems_is_not_rewritten() {
        let (_, store, project) = setup(Vec::new());
        store.set_text(Path::new("/w"), "allow = [\"bad(\"]\n");
        let err = project.add(&rule("ls")).await.unwrap_err();
        assert!(err.0.contains("has problems"), "{err}");
        assert_eq!(
            store.text(Path::new("/w")).as_deref(),
            Some("allow = [\"bad(\"]\n")
        );
    }

    #[tokio::test]
    async fn add_skips_duplicates_whatever_the_scope() {
        let (_, store, project) = setup(Vec::new());
        assert!(project.add(&rule("ls")).await.unwrap());
        assert!(
            !project
                .add(&rule("ls").with_scope(RuleScope::Project))
                .await
                .unwrap()
        );
        assert!(project.add(&deny("ls", RuleScope::Session)).await.unwrap());
        assert_eq!(store.rules(Path::new("/w")).len(), 2);
    }
}
