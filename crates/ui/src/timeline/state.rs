//! Pure timeline model: the ordered blocks of a session, rebuilt from
//! [`DomainEvent`]s. It knows nothing about GPUI, so it is tested without a
//! window; [`super::TimelineView`] mirrors its changes into the list.

use std::collections::HashMap;

use application::ports::events::DomainEvent;
use domain::block::{Block, BlockId, BlockKind};
use domain::computed::Computed;
use domain::computed::lint::lint_numbers;
use domain::computed::slots::replace_slots;

/// What the timeline worked out about the numbers of a `Text` block.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TextNumbers {
    /// The markdown with its number slots (`⟦id.key⟧`) replaced by the
    /// values of the computed blocks in the timeline; a slot no envelope
    /// has shows as `⟦?id.key⟧`. `None` when the text has no slots.
    pub resolved: Option<String>,
    /// Numbers of a finished assistant reply that are neither slots nor in
    /// the user's prompts (the "not from an engine" chip).
    pub unverified: Vec<String>,
}

use super::copy_text;

/// One block of the timeline plus its view state.
#[derive(Debug, Clone, PartialEq)]
pub struct TimelineEntry {
    /// The block as last reported by the domain.
    pub block: Block,
    /// Whether the block shows its details (tool input and output, thinking).
    pub expanded: bool,
    /// Whether the block shows its alternate view (`t`): a chart's table, a
    /// document's or diagram's source.
    pub alt_view: bool,
    /// Slots and unverified numbers of a `Text` block.
    pub numbers: TextNumbers,
}

impl TimelineEntry {
    /// An entry for `block` in its default view state.
    pub fn new(block: Block) -> Self {
        let expanded = expanded_by_default(&block.kind);
        Self {
            block,
            expanded,
            alt_view: false,
            numbers: TextNumbers::default(),
        }
    }
}

/// Whether `markdown` is a prompt the user sent (every line quoted, see
/// `prompt_block`).
fn is_prompt(markdown: &str) -> bool {
    !markdown.trim().is_empty() && markdown.lines().all(|line| line.starts_with('>'))
}

/// Whether `markdown` may hold number slots.
fn has_slots(markdown: &str) -> bool {
    markdown.contains('⟦') || markdown.contains("[[calc:")
}

/// Whether blocks of this kind have an alternate view for `t`.
pub fn has_alt_view(kind: &BlockKind) -> bool {
    matches!(
        kind,
        BlockKind::Chart(_) | BlockKind::Markdown { .. } | BlockKind::Mermaid(_)
    )
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
    selected: Option<usize>,
    /// Between `TurnStarted` and `TurnFinished`/`TurnFailed`.
    turn_open: bool,
    /// Assistant text blocks not linted yet: still streaming, or the last
    /// block when nothing followed it.
    unlinted: Vec<usize>,
    /// Entries whose [`TextNumbers`] changed outside the event's own
    /// change, for the view to measure again.
    touched: Vec<usize>,
}

/// A user prompt, as `prompt_block` writes it: every line quoted with `> `.
fn is_prompt_entry(entry: &TimelineEntry) -> bool {
    matches!(&entry.block.kind, BlockKind::Text { markdown } if is_prompt(markdown))
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

    /// Whether block `ix` ends a complete reply: it is not a prompt, and a
    /// prompt follows it (or nothing does and the turn is over). Cheap, for
    /// every frame; [`Self::reply_to_copy`] builds the text on demand.
    pub fn is_reply_end(&self, ix: usize) -> bool {
        let Some(entry) = self.entries.get(ix) else {
            return false;
        };
        if is_prompt_entry(entry) {
            return false;
        }
        match self.entries.get(ix + 1) {
            Some(next) => is_prompt_entry(next),
            None => !self.turn_open,
        }
    }

    /// The text to copy for the reply that ends at block `ix`: the text
    /// blocks since the last prompt. `None` unless `ix` is the last block of
    /// the reply, the reply is complete (the next block is a prompt, or no
    /// turn is running) and it has text.
    pub fn reply_to_copy(&self, ix: usize) -> Option<String> {
        if !self.is_reply_end(ix) {
            return None;
        }
        let start = self.entries[..ix]
            .iter()
            .rposition(is_prompt_entry)
            .map_or(0, |prompt| prompt + 1);
        let texts: Vec<String> = self.entries[start..=ix]
            .iter()
            .filter(|e| {
                matches!(
                    e.block.kind,
                    BlockKind::Text { .. } | BlockKind::Markdown { .. }
                )
            })
            .map(|e| copy_text(&e.block.kind))
            .filter(|text| !text.trim().is_empty())
            .collect();
        (!texts.is_empty()).then(|| texts.join("\n\n"))
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
                let change = self.upsert(block.clone());
                self.after_block(change);
                change
            }
            DomainEvent::TextDelta { block_id, text } => {
                let change = self.edit(*block_id, |kind| match kind {
                    BlockKind::Text { markdown } => {
                        markdown.push_str(text);
                        true
                    }
                    _ => false,
                });
                if let TimelineChange::Updated(ix) = change {
                    self.resolve(ix);
                }
                change
            }
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
            DomainEvent::TurnStarted { .. } => self.set_turn_open(true),
            DomainEvent::TurnFinished { .. } | DomainEvent::TurnFailed { .. } => {
                self.lint_pending(None);
                self.set_turn_open(false)
            }
            _ => TimelineChange::None,
        }
    }

    /// Index of the selected block, the target of normal-mode actions.
    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    /// The selected block.
    pub fn selected_entry(&self) -> Option<&TimelineEntry> {
        self.selected.and_then(|ix| self.entries.get(ix))
    }

    /// Selects the next block (`j`); with nothing selected, the last one.
    /// Returns the new selection.
    pub fn select_next(&mut self) -> Option<usize> {
        let next = match self.selected {
            Some(ix) => ix + 1,
            None => self.len().saturating_sub(1),
        };
        self.select(next)
    }

    /// Selects the previous block (`k`); with nothing selected, the last one.
    pub fn select_prev(&mut self) -> Option<usize> {
        let prev = match self.selected {
            Some(ix) => ix.saturating_sub(1),
            None => self.len().saturating_sub(1),
        };
        self.select(prev)
    }

    /// Selects the first block (`gg`).
    pub fn select_first(&mut self) -> Option<usize> {
        self.select(0)
    }

    /// Selects the last block (`G`).
    pub fn select_last(&mut self) -> Option<usize> {
        self.select(self.len().saturating_sub(1))
    }

    /// Selects block `ix`, clamped to the last one; nothing on an empty
    /// timeline.
    pub fn select(&mut self, ix: usize) -> Option<usize> {
        self.selected = (!self.is_empty()).then(|| ix.min(self.len() - 1));
        self.selected
    }

    /// Drops the selection.
    pub fn clear_selection(&mut self) {
        self.selected = None;
    }

    /// Expands or collapses the selected block (`Enter`). Returns its index
    /// when there was one.
    pub fn toggle_selected(&mut self) -> Option<usize> {
        let ix = self.selected?;
        let entry = self.entries.get_mut(ix)?;
        entry.expanded = !entry.expanded;
        Some(ix)
    }

    /// Switches the selected block between its main and alternate view
    /// (`t`). Returns its index when it has an alternate view.
    pub fn toggle_view_selected(&mut self) -> Option<usize> {
        let ix = self.selected?;
        let entry = self.entries.get_mut(ix)?;
        if !has_alt_view(&entry.block.kind) {
            return None;
        }
        entry.alt_view = !entry.alt_view;
        Some(ix)
    }

    /// The entries whose numbers changed since the last call.
    pub fn take_touched(&mut self) -> Vec<usize> {
        std::mem::take(&mut self.touched)
    }

    /// Keeps slots and lints in step after block `change`: a new or
    /// changed text resolves its slots, a computed block resolves every
    /// text again, and a block after an assistant text finishes it.
    fn after_block(&mut self, change: TimelineChange) {
        let (TimelineChange::Inserted(ix) | TimelineChange::Updated(ix)) = change else {
            return;
        };
        match &self.entries[ix].block.kind {
            BlockKind::Text { markdown } => {
                if !is_prompt(markdown) && !self.unlinted.contains(&ix) {
                    self.unlinted.push(ix);
                }
                self.resolve(ix);
            }
            BlockKind::Computed { .. } => {
                for text in 0..self.entries.len() {
                    if self.resolve(text) {
                        self.touched.push(text);
                    }
                }
            }
            _ => {}
        }
        if matches!(change, TimelineChange::Inserted(_)) {
            self.lint_pending(Some(ix));
        }
    }

    /// The envelopes of the computed blocks, newest first (a recomputed
    /// card replaces its block, so ids stay unique in practice).
    fn envelopes(&self) -> Vec<&Computed> {
        self.entries
            .iter()
            .rev()
            .filter_map(|entry| match &entry.block.kind {
                BlockKind::Computed { envelope, .. } => Some(envelope.as_ref()),
                _ => None,
            })
            .collect()
    }

    /// Resolves the slots of text entry `ix`; returns whether that changed
    /// what it shows.
    fn resolve(&mut self, ix: usize) -> bool {
        let BlockKind::Text { markdown } = &self.entries[ix].block.kind else {
            return false;
        };
        let resolved = has_slots(markdown).then(|| {
            let envelopes = self.envelopes();
            replace_slots(markdown, |slot| {
                Some(
                    envelopes
                        .iter()
                        .find(|e| e.id == slot.id)
                        .and_then(|e| e.value(slot.key))
                        .map_or_else(
                            || format!("⟦?{}.{}⟧", slot.id, slot.key),
                            domain::computed::ComputedValue::display_text,
                        ),
                )
            })
        });
        let numbers = &mut self.entries[ix].numbers;
        if numbers.resolved == resolved {
            return false;
        }
        numbers.resolved = resolved;
        true
    }

    /// Lints the assistant texts that are finished: all of them at the end
    /// of a turn (`before` is `None`), or those before entry `before`.
    fn lint_pending(&mut self, before: Option<usize>) {
        let pending = std::mem::take(&mut self.unlinted);
        let (done, waiting): (Vec<usize>, Vec<usize>) = pending
            .into_iter()
            .partition(|ix| before.is_none_or(|b| *ix < b));
        self.unlinted = waiting;
        if done.is_empty() {
            return;
        }
        let prompts: Vec<&str> = self
            .entries
            .iter()
            .filter_map(|entry| match &entry.block.kind {
                BlockKind::Text { markdown } if is_prompt(markdown) => Some(markdown.as_str()),
                _ => None,
            })
            .collect();
        let found: Vec<(usize, Vec<String>)> = done
            .into_iter()
            .filter_map(|ix| match &self.entries.get(ix)?.block.kind {
                BlockKind::Text { markdown } => Some((
                    ix,
                    lint_numbers(markdown, &prompts)
                        .into_iter()
                        .map(|n| n.text)
                        .collect(),
                )),
                _ => None,
            })
            .collect();
        for (ix, unverified) in found {
            if self.entries[ix].numbers.unverified != unverified {
                self.entries[ix].numbers.unverified = unverified;
                self.touched.push(ix);
            }
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

    /// Records whether a turn is running. The last block may change its
    /// reply's state (its copy button), so it is reported as updated.
    fn set_turn_open(&mut self, open: bool) -> TimelineChange {
        if self.turn_open == open {
            return TimelineChange::None;
        }
        self.turn_open = open;
        match self.entries.len().checked_sub(1) {
            Some(ix) => TimelineChange::Updated(ix),
            None => TimelineChange::None,
        }
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
    use domain::conversation::{SessionId, StopReason, TurnId};
    use domain::model::{ModelId, ModelRole};
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

    fn with_blocks(n: usize) -> TimelineState {
        let mut state = TimelineState::new();
        for i in 0..n {
            state.apply(&added(&text(&i.to_string())));
        }
        state
    }

    #[test]
    fn selection_moves_and_clamps() {
        let mut state = with_blocks(3);
        assert_eq!(state.selected(), None);
        assert_eq!(
            state.select_next(),
            Some(2),
            "j without selection picks the last"
        );
        assert_eq!(state.select_next(), Some(2), "clamped at the end");
        assert_eq!(state.select_prev(), Some(1));
        assert_eq!(state.select_first(), Some(0));
        assert_eq!(state.select_prev(), Some(0), "clamped at the start");
        assert_eq!(state.select_last(), Some(2));
        assert_eq!(
            state.selected_entry().map(|e| e.block.id),
            state.get(2).map(|e| e.block.id)
        );
    }

    #[test]
    fn selection_on_empty_timeline_is_none() {
        let mut state = TimelineState::new();
        assert_eq!(state.select_next(), None);
        assert_eq!(state.select_last(), None);
        assert_eq!(state.select_first(), None);
        assert_eq!(state.toggle_selected(), None);
    }

    #[test]
    fn selection_survives_new_blocks() {
        let mut state = with_blocks(2);
        state.select_first();
        state.apply(&added(&text("new")));
        assert_eq!(state.selected(), Some(0));
        assert_eq!(state.select_last(), Some(2));
    }

    #[test]
    fn toggle_flips_expansion_of_selected_block() {
        let mut state = TimelineState::new();
        state.apply(&added(&text("a")));
        state.apply(&added(&tool("read_file")));
        state.select_last();
        assert_eq!(state.toggle_selected(), Some(1));
        assert!(state.entries()[1].expanded);
        assert!(state.entries()[0].expanded, "others untouched");
        state.toggle_selected();
        assert!(!state.entries()[1].expanded);
    }

    #[test]
    fn toggle_view_only_flips_blocks_with_an_alternate_view() {
        let mut state = TimelineState::new();
        state.apply(&added(&text("a")));
        state.apply(&added(&Block::new(BlockKind::Markdown {
            title: None,
            source: "# doc".into(),
        })));
        state.select_first();
        assert_eq!(state.toggle_view_selected(), None, "text has no alt view");
        assert!(!state.entries()[0].alt_view);
        state.select_last();
        assert_eq!(state.toggle_view_selected(), Some(1));
        assert!(state.entries()[1].alt_view);
        state.apply(&DomainEvent::BlockUpdated {
            block: state.entries()[1].block.clone(),
        });
        assert!(state.entries()[1].alt_view, "kept across updates");
        state.toggle_view_selected();
        assert!(!state.entries()[1].alt_view);
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

    fn prompt(line: &str) -> Block {
        Block::new(BlockKind::Text {
            markdown: format!("> {line}\n"),
        })
    }

    fn turn_started() -> DomainEvent {
        DomainEvent::TurnStarted {
            session_id: SessionId::new(),
            turn_id: TurnId::new(),
            model: ModelId::new("m"),
            role: ModelRole::Main,
        }
    }

    fn turn_finished() -> DomainEvent {
        DomainEvent::TurnFinished {
            turn_id: TurnId::new(),
            stop_reason: StopReason::EndTurn,
        }
    }

    #[test]
    fn reply_is_copyable_only_once_complete() {
        let mut state = TimelineState::new();
        state.apply(&turn_started());
        state.apply(&added(&prompt("hola")));
        let first = text("Hola");
        let tool_call = tool("read_file");
        state.apply(&added(&first));
        state.apply(&added(&tool_call));
        let last = text("¿en qué te ayudo?");
        state.apply(&added(&last));
        // The turn is still writing the reply: no copy button yet.
        assert_eq!(state.reply_to_copy(3), None);
        assert_eq!(state.reply_to_copy(1), None, "not the last block");
        assert_eq!(state.reply_to_copy(2), None, "not the last block");
        state.apply(&turn_finished());
        assert_eq!(
            state.reply_to_copy(3).as_deref(),
            Some("Hola\n\n¿en qué te ayudo?"),
            "only text blocks, from the prompt on"
        );
        // A prompt after the reply completes it even during a new turn.
        state.apply(&turn_started());
        state.apply(&added(&prompt("otra")));
        assert!(state.reply_to_copy(3).is_some());
        assert_eq!(state.reply_to_copy(4), None, "a prompt has no copy button");
    }

    #[test]
    fn reply_without_text_has_no_copy_button() {
        let mut state = TimelineState::new();
        state.apply(&added(&prompt("ls")));
        state.apply(&added(&tool("bash")));
        assert_eq!(state.reply_to_copy(1), None);
    }

    fn computed(id: &str, key: &str, display: &str) -> Block {
        use domain::computed::{ComputedValue, DescriptiveBody, Engine};
        let engine = Engine {
            name: "crest-stats".into(),
            version: "0".into(),
            backend: None,
            platform: None,
        };
        let mut envelope = Computed::new(id, "stats.describe", "Descriptives", engine);
        envelope.values.insert(
            key.into(),
            ComputedValue {
                display: Some(display.into()),
                ..ComputedValue::new(0.0)
            },
        );
        Block::new(BlockKind::Computed {
            envelope: Box::new(envelope),
            body: Box::new(domain::computed::ComputedBody::Descriptive(
                DescriptiveBody {
                    by: None,
                    variables: Vec::new(),
                },
            )),
        })
    }

    fn numbers(state: &TimelineState, ix: usize) -> &TextNumbers {
        &state.entries()[ix].numbers
    }

    #[test]
    fn slots_show_engine_values_and_unknown_ones_a_marker() {
        let mut state = TimelineState::new();
        state.apply(&added(&computed("st_1", "p_value", ".030")));
        let reply = text("p = ⟦st_1.p_value⟧ and ⟦st_2.r⟧; plain");
        state.apply(&added(&reply));
        assert_eq!(
            numbers(&state, 1).resolved.as_deref(),
            Some("p = .030 and ⟦?st_2.r⟧; plain")
        );
        assert_eq!(state.take_touched(), Vec::<usize>::new());

        // A card that arrives later fills the open slot.
        state.apply(&added(&computed("st_2", "r", ".45")));
        assert_eq!(
            numbers(&state, 1).resolved.as_deref(),
            Some("p = .030 and .45; plain")
        );
        assert_eq!(state.take_touched(), [1]);

        let plain = text("no slots here");
        state.apply(&added(&plain));
        assert_eq!(numbers(&state, 3).resolved, None);
    }

    #[test]
    fn finished_replies_are_linted_against_the_prompts() {
        let mut state = TimelineState::new();
        state.apply(&added(&text("> we have 1500 users\n")));
        let reply = text("");
        state.apply(&added(&reply));
        state.apply(&DomainEvent::TextDelta {
            block_id: reply.id,
            text: "Of 1500 users, 3 groups had a mean of 12.7 and ⟦st_1.p⟧.".into(),
        });
        // Still streaming: nothing flagged yet.
        assert!(numbers(&state, 1).unverified.is_empty());
        state.apply(&added(&tool("read_file")));
        assert_eq!(numbers(&state, 1).unverified, ["12.7"]);
        assert_eq!(state.take_touched(), [1]);
        // The prompt itself is never linted.
        assert!(numbers(&state, 0).unverified.is_empty());

        let last = text("Then 45.5 more.");
        state.apply(&added(&last));
        assert!(numbers(&state, 3).unverified.is_empty());
        state.apply(&DomainEvent::TurnFinished {
            turn_id: domain::conversation::TurnId::new(),
            stop_reason: domain::conversation::StopReason::EndTurn,
        });
        assert_eq!(numbers(&state, 3).unverified, ["45.5"]);
    }
}
