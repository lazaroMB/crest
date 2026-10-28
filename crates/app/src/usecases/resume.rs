//! Resuming the last session of a workspace on start-up.
//!
//! The stored session brings back the conversation history (sent to the
//! model on the next turn), the timeline to redraw and the "allow always"
//! rules granted during it, which go back into the shared permission
//! policy.

use domain::block::{BlockId, BlockKind};
use domain::conversation::SessionId;
use domain::form::FormState;
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
    store.latest_for_workspace(workspace_root).await
}

/// The stored session `id`, when it belongs to `workspace_root`.
///
/// # Errors
///
/// [`StoreError::NotFound`] when the id is missing or belongs to another
/// workspace, or the store's own error.
pub async fn resume_by_id(
    store: &dyn SessionStorePort,
    id: SessionId,
    workspace_root: &str,
) -> Result<StoredSession, StoreError> {
    let stored = store.load(id).await?;
    if !same_workspace(&stored.session.workspace_root, workspace_root) {
        return Err(StoreError::NotFound(id));
    }
    Ok(stored)
}

/// The two roots name the same directory. Slashes are ignored; on Windows
/// the comparison is case-insensitive.
fn same_workspace(left: &str, right: &str) -> bool {
    let normalize = |root: &str| root.replace('\\', "/").trim_end_matches('/').to_owned();
    let (left, right) = (normalize(left), normalize(right));
    if cfg!(windows) {
        left.eq_ignore_ascii_case(&right)
    } else {
        left == right
    }
}

/// Closes the `Form` blocks of `stored` still pending although their
/// `ask_user` call is over (the turn was interrupted before the block was
/// updated, or an old version saved it so). A form whose call is pending
/// (saved while it waited; see
/// [`crate::usecases::run_turn::AgentService::resume_pending_tools`]) is
/// left as it is. Returns how many were closed.
pub fn settle_orphan_forms(stored: &mut StoredSession) -> usize {
    let waiting: Vec<BlockId> = stored
        .session
        .pending_tool_uses()
        .iter()
        .map(|pending| BlockId::for_tool_use(pending.id.as_str(), "form"))
        .collect();
    let mut closed = 0;
    for block in &mut stored.timeline {
        if let BlockKind::Form(spec) = &mut block.kind
            && spec.state == FormState::Pending
            && !waiting.contains(&block.id)
        {
            spec.state = FormState::Cancelled;
            closed += 1;
        }
    }
    closed
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
    async fn a_chosen_session_loads_only_in_its_workspace() {
        let store = InMemorySessionStore::new();
        let session = stored(r"C:\ws", 10);
        let id = session.session.id;
        store.save(&session).await.unwrap();
        assert_eq!(
            resume_by_id(&store, id, "C:/ws").await.unwrap().session.id,
            id
        );
        assert_eq!(
            resume_by_id(&store, id, "/otro").await,
            Err(StoreError::NotFound(id))
        );
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
    fn pending_forms_of_finished_calls_are_closed_on_load() {
        use domain::block::Block;
        use domain::conversation::{ContentPart, Message, Role, ToolUseId, Turn};
        use domain::form::FormSpec;
        use serde_json::json;

        let form = |state: &str| -> FormSpec {
            serde_json::from_value(json!({"title": "t", "fields": [], "state": state})).unwrap()
        };
        let mut stored = stored("/ws", 1);
        // A turn cut while `toolu_1` waited: its form stays pending.
        let mut turn = Turn::new(Message::user_text("hola"));
        turn.messages.push(Message::new(
            Role::Assistant,
            vec![ContentPart::ToolUse {
                id: ToolUseId::new("toolu_1"),
                name: "ask_user".into(),
                input: json!({}),
            }],
        ));
        stored.session.push_turn(turn, 2);
        stored.timeline = vec![
            Block {
                id: BlockId::for_tool_use("toolu_0", "form"),
                kind: BlockKind::Form(form("pending")),
            },
            Block {
                id: BlockId::for_tool_use("toolu_1", "form"),
                kind: BlockKind::Form(form("pending")),
            },
            Block::new(BlockKind::Form(form("submitted"))),
        ];
        assert_eq!(settle_orphan_forms(&mut stored), 1);
        let states: Vec<_> = stored
            .timeline
            .iter()
            .map(|block| match &block.kind {
                BlockKind::Form(spec) => spec.state,
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(
            states,
            [
                FormState::Cancelled,
                FormState::Pending,
                FormState::Submitted
            ]
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
