//! Timeline blocks and their payloads.
//!
//! A [`Block`] is a stable [`BlockId`] plus a [`BlockKind`]. The kind is
//! flattened into the block and tagged by `type`, so a block serializes as
//! `{"id": "...", "type": "text", "markdown": "..."}`. Payloads are plain
//! data: the UI decides how to draw each kind, and adapters (diff engines,
//! component tools, Lua plugins) only fill them in.

use std::borrow::Cow;
use std::fmt;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// Stable identifier of a block in the timeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BlockId(pub Uuid);

impl BlockId {
    /// Creates a fresh random id.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for BlockId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for BlockId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl JsonSchema for BlockId {
    fn schema_name() -> Cow<'static, str> {
        "BlockId".into()
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "description": "Stable identifier of a block in the timeline.",
            "type": "string",
            "format": "uuid"
        })
    }
}

/// A block of the timeline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Block {
    /// Stable id, used to update the block in place (streaming, status).
    pub id: BlockId,
    /// What the block shows.
    #[serde(flatten)]
    pub kind: BlockKind,
}

impl Block {
    /// Creates a block with a fresh id.
    #[must_use]
    pub fn new(kind: BlockKind) -> Self {
        Self {
            id: BlockId::new(),
            kind,
        }
    }
}

/// The payload of a [`Block`], tagged by `type`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BlockKind {
    /// Model answer, markdown that may still be streaming.
    Text {
        /// Markdown source.
        markdown: String,
    },
    /// Extended thinking of the model.
    Thinking {
        /// Raw reasoning text.
        text: String,
        /// Whether the UI shows it collapsed (the default).
        collapsed: bool,
    },
    /// A tool invocation: one line with name, key arguments and status.
    ToolCall {
        /// Id of the `tool_use` content block this call belongs to.
        tool_use_id: String,
        /// Tool name.
        name: String,
        /// Short human summary of the key arguments.
        summary: String,
        /// Raw tool input as sent by the model.
        input: Value,
        /// Lifecycle state.
        status: ToolStatus,
        /// Tool output shown when expanded, once available.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<String>,
    },
    /// Failure of the API or a tool.
    Error {
        /// What went wrong.
        message: String,
        /// Suggested action for the user.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hint: Option<String>,
    },
}

/// Lifecycle of a [`BlockKind::ToolCall`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    /// Requested by the model, not started yet.
    Pending,
    /// Waiting for the user to approve it.
    AwaitingApproval,
    /// Executing.
    Running,
    /// Finished successfully.
    Succeeded,
    /// Finished with an error.
    Failed,
    /// Rejected by the user or a permission rule.
    Denied,
    /// Interrupted before finishing.
    Cancelled,
}

impl ToolStatus {
    /// Whether the call has reached a final state.
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::Denied | Self::Cancelled
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn roundtrip(block: &Block) -> serde_json::Value {
        let value = serde_json::to_value(block).unwrap();
        let back: Block = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(&back, block);
        value
    }

    #[test]
    fn block_ids_are_unique() {
        assert_ne!(BlockId::new(), BlockId::new());
    }

    #[test]
    fn block_id_serializes_as_plain_uuid_string() {
        let id = BlockId(Uuid::nil());
        assert_eq!(
            serde_json::to_value(id).unwrap(),
            json!("00000000-0000-0000-0000-000000000000")
        );
        assert_eq!(id.to_string(), "00000000-0000-0000-0000-000000000000");
    }

    #[test]
    fn text_block_flattens_kind_with_type_tag() {
        let block = Block {
            id: BlockId(Uuid::nil()),
            kind: BlockKind::Text {
                markdown: "# hi".into(),
            },
        };
        assert_eq!(
            roundtrip(&block),
            json!({"id": "00000000-0000-0000-0000-000000000000", "type": "text", "markdown": "# hi"})
        );
    }

    #[test]
    fn thinking_and_error_roundtrip() {
        roundtrip(&Block::new(BlockKind::Thinking {
            text: "hmm".into(),
            collapsed: true,
        }));
        let error = Block::new(BlockKind::Error {
            message: "rate limited".into(),
            hint: None,
        });
        let value = roundtrip(&error);
        assert!(value.get("hint").is_none());
        roundtrip(&Block::new(BlockKind::Error {
            message: "rate limited".into(),
            hint: Some("retry later".into()),
        }));
    }

    #[test]
    fn tool_call_roundtrip_with_snake_case_status() {
        let block = Block::new(BlockKind::ToolCall {
            tool_use_id: "toolu_01".into(),
            name: "read_file".into(),
            summary: "src/main.rs".into(),
            input: json!({"path": "src/main.rs"}),
            status: ToolStatus::AwaitingApproval,
            output: None,
        });
        let value = roundtrip(&block);
        assert_eq!(value["type"], "tool_call");
        assert_eq!(value["status"], "awaiting_approval");
        assert!(value.get("output").is_none());
    }

    #[test]
    fn tool_status_terminal_states() {
        assert!(!ToolStatus::Pending.is_terminal());
        assert!(!ToolStatus::AwaitingApproval.is_terminal());
        assert!(!ToolStatus::Running.is_terminal());
        assert!(ToolStatus::Succeeded.is_terminal());
        assert!(ToolStatus::Failed.is_terminal());
        assert!(ToolStatus::Denied.is_terminal());
        assert!(ToolStatus::Cancelled.is_terminal());
    }
}
