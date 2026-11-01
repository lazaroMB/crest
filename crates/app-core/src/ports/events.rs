//! Domain events emitted by the use cases towards the UI.
//!
//! The UI only consumes these events and never polls the use cases, so every
//! visible change of a turn (new block, streamed text, tool status, usage,
//! warnings) has a [`DomainEvent`].

use domain::block::{Block, BlockId, CustomNode, ToolStatus};
use domain::conversation::{SessionId, StopReason, TurnId, Usage};
use domain::modal::{ModalId, ModalSpec};
use domain::model::{ModelId, ModelRole};
use domain::permission::PermissionMode;
use domain::sysmodel::Board;
use serde::{Deserialize, Serialize};

use super::approval::ApprovalRequestId;

/// Severity of a [`DomainEvent::Notice`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoticeLevel {
    /// Informative message.
    Info,
    /// Something degraded but the app keeps working (e.g. unknown model).
    Warning,
    /// Something failed.
    Error,
}

/// Something that happened in a session, in emission order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DomainEvent {
    /// A turn started with the resolved model.
    TurnStarted {
        /// Session of the turn.
        session_id: SessionId,
        /// The new turn.
        turn_id: TurnId,
        /// Model answering the turn.
        model: ModelId,
        /// Role the model was resolved from.
        role: ModelRole,
    },
    /// A block was appended to the timeline.
    BlockAdded {
        /// The new block.
        block: Block,
    },
    /// A block changed; `block` replaces the previous one with the same id.
    BlockUpdated {
        /// The full new version of the block.
        block: Block,
    },
    /// Markdown streamed into a `Text` block; append `text` to it.
    TextDelta {
        /// Block receiving the text.
        block_id: BlockId,
        /// Fragment to append.
        text: String,
    },
    /// Reasoning streamed into a `Thinking` block; append `text` to it.
    ThinkingDelta {
        /// Block receiving the text.
        block_id: BlockId,
        /// Fragment to append.
        text: String,
    },
    /// The status of a `ToolCall` block changed.
    ToolStatusChanged {
        /// The tool call block.
        block_id: BlockId,
        /// New status.
        status: ToolStatus,
        /// Short output summary, when the call finished.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<String>,
    },
    /// A tool call waits for the user's approval.
    ApprovalRequested {
        /// Request to answer through the approval port.
        request_id: ApprovalRequestId,
        /// Block showing the call (its diff or command).
        block_id: BlockId,
    },
    /// Token usage and cost were updated.
    UsageUpdated {
        /// Usage of the current turn so far.
        turn: Usage,
        /// Usage of the whole session.
        session: Usage,
        /// Cost of the session in USD, when pricing is known.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cost_usd: Option<f64>,
    },
    /// The model failed and the turn continues with a fallback.
    ModelFallback {
        /// Model that failed.
        from: ModelId,
        /// Model used instead.
        to: ModelId,
    },
    /// A model call failed transiently and will be retried.
    Retrying {
        /// Retry number, starting at 1.
        attempt: u32,
        /// Wait before the retry, in milliseconds.
        delay_ms: u64,
    },
    /// The permission mode changed.
    ModeChanged {
        /// The new mode.
        mode: PermissionMode,
    },
    /// The turn finished normally.
    TurnFinished {
        /// The finished turn.
        turn_id: TurnId,
        /// Why the model stopped.
        stop_reason: StopReason,
    },
    /// The turn failed and was abandoned.
    TurnFailed {
        /// The failed turn.
        turn_id: TurnId,
        /// Human-readable error.
        error: String,
    },
    /// What a long piece of work (a flow stage, a sub-agent, a check) is
    /// doing now, for the activity line. Transient: it is neither a block
    /// nor saved, and the next one replaces it.
    Activity {
        /// Short present-tense description.
        message: String,
    },
    /// A message for the status bar.
    Notice {
        /// Severity.
        level: NoticeLevel,
        /// Text to show.
        message: String,
    },
    /// A plugin opened a modal, or replaced the one with the same id.
    ModalOpened {
        /// What to show and where.
        spec: ModalSpec,
    },
    /// A plugin replaced the content of an open modal.
    ModalUpdated {
        /// The modal.
        id: ModalId,
        /// Its new content.
        content: CustomNode,
    },
    /// A plugin closed a modal.
    ModalClosed {
        /// The modal.
        id: ModalId,
    },
    /// A solution was simulated (`/simulate`, `simulate_solution`): the
    /// window opens its board in a full-size modal.
    SimulationReady {
        /// The model and the run of every scenario.
        board: Box<Board>,
    },
}

impl DomainEvent {
    /// Whether this event is about a modal (a plugin's or a simulation
    /// board); those are not part of the saved timeline.
    #[must_use]
    pub fn is_modal(&self) -> bool {
        matches!(
            self,
            Self::ModalOpened { .. }
                | Self::ModalUpdated { .. }
                | Self::ModalClosed { .. }
                | Self::SimulationReady { .. }
        )
    }
}

/// Receives the domain events of the use cases.
///
/// `emit` is synchronous and must not block: the real adapter pushes into an
/// unbounded channel drained by the UI thread, and a slow consumer must never
/// stall the agent loop. Events that cannot be delivered (UI closed) are
/// dropped silently.
pub trait EventSink: Send + Sync {
    /// Delivers `event` without blocking.
    fn emit(&self, event: DomainEvent);
}

#[cfg(test)]
mod tests {
    use domain::block::BlockKind;

    use super::*;

    fn round_trip(event: &DomainEvent) {
        let json = serde_json::to_string(event).unwrap();
        let back: DomainEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(&back, event, "{json}");
    }

    #[test]
    fn events_round_trip_through_json() {
        let block = Block::new(BlockKind::Text {
            markdown: "hola".into(),
        });
        let events = [
            DomainEvent::TurnStarted {
                session_id: SessionId::new(),
                turn_id: TurnId::new(),
                model: ModelId::new("claude-sonnet-5-5"),
                role: ModelRole::Main,
            },
            DomainEvent::BlockAdded {
                block: block.clone(),
            },
            DomainEvent::BlockUpdated {
                block: block.clone(),
            },
            DomainEvent::TextDelta {
                block_id: block.id,
                text: " mundo".into(),
            },
            DomainEvent::ThinkingDelta {
                block_id: block.id,
                text: "hmm".into(),
            },
            DomainEvent::ToolStatusChanged {
                block_id: block.id,
                status: ToolStatus::Succeeded,
                output: Some("3 matches".into()),
            },
            DomainEvent::ApprovalRequested {
                request_id: ApprovalRequestId(7),
                block_id: block.id,
            },
            DomainEvent::UsageUpdated {
                turn: Usage::default(),
                session: Usage::default(),
                cost_usd: Some(0.25),
            },
            DomainEvent::ModelFallback {
                from: ModelId::new("a"),
                to: ModelId::new("b"),
            },
            DomainEvent::Retrying {
                attempt: 1,
                delay_ms: 500,
            },
            DomainEvent::ModeChanged {
                mode: PermissionMode::Plan,
            },
            DomainEvent::TurnFinished {
                turn_id: TurnId::new(),
                stop_reason: StopReason::EndTurn,
            },
            DomainEvent::TurnFailed {
                turn_id: TurnId::new(),
                error: "boom".into(),
            },
            DomainEvent::Activity {
                message: "data: asking the model".into(),
            },
            DomainEvent::Notice {
                level: NoticeLevel::Warning,
                message: "unknown model".into(),
            },
            DomainEvent::ModalOpened {
                spec: ModalSpec {
                    id: ModalId::new("p/coverage"),
                    plugin: "p".into(),
                    title: "Coverage".into(),
                    width: "60%".parse().unwrap(),
                    height: domain::modal::Percent::FULL,
                    position: "top-right".parse().unwrap(),
                    content: CustomNode::Text { text: "x".into() },
                },
            },
            DomainEvent::ModalUpdated {
                id: ModalId::new("p/coverage"),
                content: CustomNode::Markdown {
                    source: "# hi".into(),
                },
            },
            DomainEvent::ModalClosed {
                id: ModalId::new("p/coverage"),
            },
        ];
        for event in &events {
            round_trip(event);
        }
        let modal = events.iter().filter(|e| e.is_modal()).count();
        assert_eq!(modal, 3);
    }

    #[test]
    fn events_are_tagged_by_type() {
        let event = DomainEvent::Retrying {
            attempt: 2,
            delay_ms: 1000,
        };
        insta::assert_json_snapshot!(event, @r#"
        {
          "type": "retrying",
          "attempt": 2,
          "delay_ms": 1000
        }
        "#);
    }

    #[test]
    fn sink_is_object_safe() {
        fn assert_dyn(_: Option<std::sync::Arc<dyn EventSink>>) {}
        assert_dyn(None);
    }
}
