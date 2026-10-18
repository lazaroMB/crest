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
    /// Failure of the API or a tool.
    Error {
        /// What went wrong.
        message: String,
        /// Suggested action for the user.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hint: Option<String>,
    },
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
}
