//! Resuming the last session of a workspace on start-up.
//!
//! The stored session brings back the conversation history (sent to the
//! model on the next turn), the timeline to redraw and the "allow always"
//! rules granted during it, which go back into the shared permission
//! policy. A form still pending in it was never answered (nobody waits
//! for it any more): it comes back cancelled, "not answered".

use domain::block::{Block, BlockKind};
use domain::permission::PermissionRule;

use crate::ports::store::{SessionStorePort, StoreError, StoredSession};
use crate::usecases::approval::{SharedPermissions, lock_permissions};

/// The most recently updated session of `workspace_root`, if any.
///
/// # Errors
///
/// The store's error (a corrupt session, an unreadable database).
pub async fn resume_latest(
    store: &dyn SessionStorePort,
    workspace_root: &str,
) -> Result<Option<StoredSession>, StoreError> {
    let mut stored = store.latest_for_workspace(workspace_root).await?;
    if let Some(stored) = &mut stored {
        withdraw_pending_forms(&mut stored.timeline);
    }
    Ok(stored)
}

/// Marks every pending `Form` of `timeline` cancelled, not answered;
/// returns how many there were.
pub fn withdraw_pending_forms(timeline: &mut [Block]) -> usize {
    let mut withdrawn = 0;
    for block in timeline {
        if let BlockKind::Form(spec) = &mut block.kind
            && spec.withdraw()
        {
            withdrawn += 1;
        }
    }
    withdrawn
}

/// Adds the `rules` of a resumed session to `permissions`, after the rules
/// already there and without repeating any.
pub fn restore_permission_rules(permissions: &SharedPermissions, rules: &[PermissionRule]) {
    let mut policy = lock_permissions(permissions);
    for rule in rules {
        if !policy.rules.contains(rule) {
            policy.rules.push(rule.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use domain::conversation::Session;
    use domain::permission::{PermissionMode, PermissionPolicy, RuleDecision, RuleMatcher};

    use super::*;
    use crate::testing::system::InMemorySessionStore;

    fn stored(root: &str, updated_at: u64) -> StoredSession {
        StoredSession {
            session: Session::new(root, updated_at),
            timeline: Vec::new(),
            permission_rules: Vec::new(),
            prompt_history: Vec::new(),
        }
    }

    #[tokio::test]
    async fn pending_forms_come_back_not_answered() {
        use domain::form::{FormSpec, FormState, NOT_ANSWERED};
        let form = |state| {
            Block::new(BlockKind::Form(FormSpec {
                title: "t".into(),
                description: None,
                fields: Vec::new(),
                submit_label: None,
                state,
                answer: None,
                reason: None,
                body: None,
                actions: Vec::new(),
                action: None,
            }))
        };
        let store = InMemorySessionStore::new();
        let mut session = stored("/ws", 10);
        session.timeline = vec![form(FormState::Pending), form(FormState::Submitted)];
        store.save(&session).await.unwrap();
        let resumed = resume_latest(&store, "/ws").await.unwrap().unwrap();
        let states: Vec<_> = resumed
            .timeline
            .iter()
            .map(|b| match &b.kind {
                BlockKind::Form(spec) => (spec.state, spec.reason.clone()),
                _ => panic!("a form"),
            })
            .collect();
        assert_eq!(
            states,
            [
                (FormState::Cancelled, Some(NOT_ANSWERED.to_owned())),
                (FormState::Submitted, None)
            ]
        );
    }

    #[tokio::test]
    async fn the_latest_session_of_the_workspace_is_resumed() {
        let store = InMemorySessionStore::new();
        assert_eq!(resume_latest(&store, "/ws").await, Ok(None));
        let old = stored("/ws", 10);
        let new = stored("/ws", 20);
        for session in [&old, &new, &stored("/otro", 30)] {
            store.save(session).await.unwrap();
        }
        let resumed = resume_latest(&store, "/ws").await.unwrap();
        assert_eq!(resumed, Some(new));
    }

    #[tokio::test]
    async fn store_errors_are_returned() {
        let store = InMemorySessionStore::new();
        store.fail_next(StoreError::Backend("locked".into()));
        assert_eq!(
            resume_latest(&store, "/ws").await,
            Err(StoreError::Backend("locked".into()))
        );
    }

    #[test]
    fn rules_are_restored_once() {
        let rule = |path: &str| {
            PermissionRule::new(
                "edit_file",
                RuleMatcher::Exact(path.into()),
                RuleDecision::AllowAlways,
            )
        };
        let permissions: SharedPermissions =
            Arc::new(Mutex::new(PermissionPolicy::new(PermissionMode::Ask)));
        lock_permissions(&permissions).rules.push(rule("a.rs"));
        restore_permission_rules(&permissions, &[rule("a.rs"), rule("b.rs")]);
        let policy = lock_permissions(&permissions);
        assert_eq!(policy.rules, [rule("a.rs"), rule("b.rs")]);
        assert_eq!(policy.mode, PermissionMode::Ask);
    }
}
