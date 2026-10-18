//! Pure timeline model: the ordered blocks of a session, rebuilt from
//! [`DomainEvent`]s. It knows nothing about GPUI, so it is tested without a
//! window; [`super::TimelineView`] mirrors its changes into the list.

use std::collections::HashMap;

use application::ports::events::DomainEvent;
use domain::block::{Block, BlockId, BlockKind};

/// One block of the timeline plus its view state.
#[derive(Debug, Clone, PartialEq)]
pub struct TimelineEntry {
    /// The block as last reported by the domain.
    pub block: Block,
    /// Whether the block shows its details (tool input and output, thinking).
    pub expanded: bool,
}

impl TimelineEntry {
    fn new(block: Block) -> Self {
        let expanded = expanded_by_default(&block.kind);
        Self { block, expanded }
    }
}

/// Initial expansion of a block: tool calls start as one line and thinking
/// follows its own `collapsed` flag; everything else is shown in full.
fn expanded_by_default(kind: &BlockKind) -> bool {
    match kind {
        BlockKind::ToolCall { .. } => false,
        BlockKind::Thinking { collapsed, .. } => !collapsed,
        _ => true,
    }
}

/// What an event changed, so the view can update only what it must.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimelineChange {
    /// The event does not touch the timeline (usage, notices, unknown ids).
    None,
    /// A block was inserted at this index.
    Inserted(usize),
    /// The block at this index changed in place.
    Updated(usize),
}

/// Ordered blocks of the timeline with lookup by id.
#[derive(Debug, Clone, Default)]
pub struct TimelineState {
    entries: Vec<TimelineEntry>,
    index: HashMap<BlockId, usize>,
}

impl TimelineState {
    /// An empty timeline.
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of blocks.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether there are no blocks.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Blocks in timeline order.
    pub fn entries(&self) -> &[TimelineEntry] {
        &self.entries
    }

    /// The block at `ix`.
    pub fn get(&self, ix: usize) -> Option<&TimelineEntry> {
        self.entries.get(ix)
    }

    /// Index of the block with `id`.
    pub fn position(&self, id: BlockId) -> Option<usize> {
        self.index.get(&id).copied()
    }

    /// Applies one domain event.
    ///
    /// `BlockAdded` with a known id and `BlockUpdated` replace the block in
    /// place (keeping its expansion); `BlockUpdated` with an unknown id
    /// appends it, so a lost `BlockAdded` never hides a block. Deltas and
    /// status changes for unknown ids, or for blocks of another kind, are
    /// ignored.
    pub fn apply(&mut self, event: &DomainEvent) -> TimelineChange {
        match event {
            DomainEvent::BlockAdded { block } | DomainEvent::BlockUpdated { block } => {
                self.upsert(block.clone())
            }
            DomainEvent::TextDelta { block_id, text } => self.edit(*block_id, |kind| match kind {
                BlockKind::Text { markdown } => {
                    markdown.push_str(text);
                    true
                }
                _ => false,
            }),
            DomainEvent::ThinkingDelta { block_id, text } => {
                self.edit(*block_id, |kind| match kind {
                    BlockKind::Thinking { text: thinking, .. } => {
                        thinking.push_str(text);
                        true
                    }
                    _ => false,
                })
            }
            DomainEvent::ToolStatusChanged {
                block_id,
                status,
                output,
            } => self.edit(*block_id, |kind| match kind {
                BlockKind::ToolCall {
                    status: current,
                    output: current_output,
                    ..
                } => {
                    *current = *status;
                    if output.is_some() {
                        current_output.clone_from(output);
                    }
                    true
                }
                _ => false,
            }),
            _ => TimelineChange::None,
        }
    }

    fn upsert(&mut self, block: Block) -> TimelineChange {
        if let Some(ix) = self.position(block.id) {
            self.entries[ix].block = block;
            return TimelineChange::Updated(ix);
        }
        let ix = self.entries.len();
        self.index.insert(block.id, ix);
        self.entries.push(TimelineEntry::new(block));
        TimelineChange::Inserted(ix)
    }

    fn edit(&mut self, id: BlockId, f: impl FnOnce(&mut BlockKind) -> bool) -> TimelineChange {
        let Some(ix) = self.position(id) else {
            return TimelineChange::None;
        };
        if f(&mut self.entries[ix].block.kind) {
            TimelineChange::Updated(ix)
        } else {
            TimelineChange::None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::block::ToolStatus;
    use serde_json::json;

    fn text(markdown: &str) -> Block {
        Block::new(BlockKind::Text {
            markdown: markdown.into(),
        })
    }

    fn tool(name: &str) -> Block {
        Block::new(BlockKind::ToolCall {
            tool_use_id: "toolu_1".into(),
            name: name.into(),
            summary: "src/main.rs".into(),
            input: json!({"path": "src/main.rs"}),
            status: ToolStatus::Pending,
            output: None,
        })
    }

    fn added(block: &Block) -> DomainEvent {
        DomainEvent::BlockAdded {
            block: block.clone(),
        }
    }

    fn markdown(state: &TimelineState, ix: usize) -> &str {
        match &state.entries()[ix].block.kind {
            BlockKind::Text { markdown } => markdown,
            other => panic!("not text: {other:?}"),
        }
    }

    #[test]
    fn blocks_keep_emission_order() {
        let mut state = TimelineState::new();
        let blocks = [text("a"), tool("read_file"), text("b")];
        for (ix, block) in blocks.iter().enumerate() {
            assert_eq!(state.apply(&added(block)), TimelineChange::Inserted(ix));
        }
        let ids: Vec<_> = state.entries().iter().map(|e| e.block.id).collect();
        assert_eq!(ids, blocks.iter().map(|b| b.id).collect::<Vec<_>>());
        assert_eq!(state.position(blocks[2].id), Some(2));
    }

    #[test]
    fn text_delta_appends_to_its_block() {
        let mut state = TimelineState::new();
        let first = text("Hola");
        let second = text("otro");
        state.apply(&added(&first));
        state.apply(&added(&second));
        let change = state.apply(&DomainEvent::TextDelta {
            block_id: first.id,
            text: ", mundo".into(),
        });
        assert_eq!(change, TimelineChange::Updated(0));
        assert_eq!(markdown(&state, 0), "Hola, mundo");
        assert_eq!(markdown(&state, 1), "otro");
    }

    #[test]
    fn thinking_delta_appends_and_starts_collapsed() {
        let mut state = TimelineState::new();
        let block = Block::new(BlockKind::Thinking {
            text: "hmm".into(),
            collapsed: true,
        });
        state.apply(&added(&block));
        state.apply(&DomainEvent::ThinkingDelta {
            block_id: block.id,
            text: "...".into(),
        });
        let entry = &state.entries()[0];
        assert!(!entry.expanded);
        assert!(matches!(&entry.block.kind, BlockKind::Thinking { text, .. } if text == "hmm..."));
    }

    #[test]
    fn delta_for_unknown_or_other_kind_is_ignored() {
        let mut state = TimelineState::new();
        let call = tool("bash");
        state.apply(&added(&call));
        let unknown = DomainEvent::TextDelta {
            block_id: BlockId::new(),
            text: "x".into(),
        };
        let wrong_kind = DomainEvent::TextDelta {
            block_id: call.id,
            text: "x".into(),
        };
        assert_eq!(state.apply(&unknown), TimelineChange::None);
        assert_eq!(state.apply(&wrong_kind), TimelineChange::None);
        assert_eq!(state.entries()[0].block, call);
    }

    #[test]
    fn update_replaces_in_place_and_keeps_expansion() {
        let mut state = TimelineState::new();
        let mut block = text("v1");
        state.apply(&added(&block));
        state.apply(&added(&text("after")));
        block.kind = BlockKind::Text {
            markdown: "v2".into(),
        };
        let change = state.apply(&DomainEvent::BlockUpdated {
            block: block.clone(),
        });
        assert_eq!(change, TimelineChange::Updated(0));
        assert_eq!(markdown(&state, 0), "v2");
        assert_eq!(state.len(), 2);
    }

    #[test]
    fn update_of_unknown_block_appends_it() {
        let mut state = TimelineState::new();
        let block = text("late");
        let change = state.apply(&DomainEvent::BlockUpdated {
            block: block.clone(),
        });
        assert_eq!(change, TimelineChange::Inserted(0));
        assert_eq!(state.entries()[0].block, block);
    }

    #[test]
    fn tool_status_updates_status_and_output() {
        let mut state = TimelineState::new();
        let call = tool("grep");
        state.apply(&added(&call));
        state.apply(&DomainEvent::ToolStatusChanged {
            block_id: call.id,
            status: ToolStatus::Running,
            output: None,
        });
        state.apply(&DomainEvent::ToolStatusChanged {
            block_id: call.id,
            status: ToolStatus::Succeeded,
            output: Some("3 matches".into()),
        });
        let BlockKind::ToolCall { status, output, .. } = &state.entries()[0].block.kind else {
            panic!("not a tool call");
        };
        assert_eq!(*status, ToolStatus::Succeeded);
        assert_eq!(output.as_deref(), Some("3 matches"));
        assert!(!state.entries()[0].expanded);
    }

    #[test]
    fn unrelated_events_change_nothing() {
        let mut state = TimelineState::new();
        let change = state.apply(&DomainEvent::Retrying {
            attempt: 1,
            delay_ms: 10,
        });
        assert_eq!(change, TimelineChange::None);
        assert!(state.is_empty());
    }
}
