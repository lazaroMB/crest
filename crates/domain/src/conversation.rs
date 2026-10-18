//! Conversation types: sessions, turns, messages and their content parts.
//!
//! These types are the contract shared by the ports and the model adapters.
//! They map 1:1 onto the shape of the Anthropic Messages API without
//! depending on it, so the adapter is a mechanical translation.

use std::fmt;
use std::iter::Sum;
use std::ops::{Add, AddAssign};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

macro_rules! uuid_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
        )]
        #[serde(transparent)]
        pub struct $name(#[schemars(with = "String")] Uuid);

        impl $name {
            /// Creates a new random id.
            #[must_use]
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }

            /// Returns the underlying UUID.
            #[must_use]
            pub fn as_uuid(&self) -> Uuid {
                self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl From<Uuid> for $name {
            fn from(value: Uuid) -> Self {
                Self(value)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(&self.0, f)
            }
        }
    };
}

uuid_id!(
    /// Identifies a [`Session`].
    SessionId
);
uuid_id!(
    /// Identifies a [`Turn`] within a session.
    TurnId
);
uuid_id!(
    /// Identifies a [`Message`] within a turn.
    MessageId
);

/// Identifies a tool call. Assigned by the model provider (e.g. `toolu_...`),
/// so it is an opaque string rather than a UUID.
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(transparent)]
pub struct ToolUseId(String);

impl ToolUseId {
    /// Wraps a provider-assigned tool use id.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Returns the id as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for ToolUseId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for ToolUseId {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl fmt::Display for ToolUseId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Author of a [`Message`]. Not to be confused with the model role used to
/// pick which model answers (see `crate::model`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// The human (or tool results sent on their behalf).
    User,
    /// The model.
    Assistant,
}

/// One piece of a message's content.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentPart {
    /// Plain text.
    Text {
        /// The text.
        text: String,
    },
    /// Extended thinking produced by the model.
    Thinking {
        /// The reasoning text.
        thinking: String,
        /// Opaque signature that must be sent back unchanged.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    /// Thinking redacted by the provider; only the opaque payload is kept.
    RedactedThinking {
        /// Opaque encrypted payload.
        data: String,
    },
    /// A tool call requested by the model.
    ToolUse {
        /// Provider-assigned id, echoed back in the matching result.
        id: ToolUseId,
        /// Tool name.
        name: String,
        /// Tool arguments as JSON.
        input: serde_json::Value,
    },
    /// The result of executing a tool call.
    ToolResult {
        /// Id of the [`ContentPart::ToolUse`] this answers.
        tool_use_id: ToolUseId,
        /// Result content.
        content: Vec<ToolResultContent>,
        /// Whether the tool failed.
        #[serde(default)]
        is_error: bool,
    },
    /// An inline image.
    Image {
        /// MIME type, e.g. `image/png`.
        media_type: String,
        /// Base64-encoded image bytes.
        data: String,
    },
}

impl ContentPart {
    /// Builds a [`ContentPart::Text`].
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }

    /// Builds a successful [`ContentPart::ToolResult`] with a single text item.
    #[must_use]
    pub fn tool_result_text(tool_use_id: ToolUseId, text: impl Into<String>) -> Self {
        Self::ToolResult {
            tool_use_id,
            content: vec![ToolResultContent::text(text)],
            is_error: false,
        }
    }
}

/// One piece of a tool result's content.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolResultContent {
    /// Plain text.
    Text {
        /// The text.
        text: String,
    },
    /// An inline image.
    Image {
        /// MIME type, e.g. `image/png`.
        media_type: String,
        /// Base64-encoded image bytes.
        data: String,
    },
}

impl ToolResultContent {
    /// Builds a [`ToolResultContent::Text`].
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }
}

/// A single message exchanged with the model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Message {
    /// Local id of the message.
    pub id: MessageId,
    /// Who wrote it.
    pub role: Role,
    /// Ordered content parts.
    pub content: Vec<ContentPart>,
}

impl Message {
    /// Builds a message with a fresh id.
    #[must_use]
    pub fn new(role: Role, content: Vec<ContentPart>) -> Self {
        Self {
            id: MessageId::new(),
            role,
            content,
        }
    }

    /// Builds a user message with a single text part.
    #[must_use]
    pub fn user_text(text: impl Into<String>) -> Self {
        Self::new(Role::User, vec![ContentPart::text(text)])
    }

    /// Builds an assistant message from its content parts.
    #[must_use]
    pub fn assistant(content: Vec<ContentPart>) -> Self {
        Self::new(Role::Assistant, content)
    }
}

/// Token accounting for one or more model calls.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct Usage {
    /// Uncached input tokens.
    pub input_tokens: u64,
    /// Output tokens.
    pub output_tokens: u64,
    /// Input tokens served from the prompt cache.
    pub cache_read_input_tokens: u64,
    /// Input tokens written to the prompt cache.
    pub cache_creation_input_tokens: u64,
}

impl Add for Usage {
    type Output = Self;

    fn add(mut self, rhs: Self) -> Self {
        self += rhs;
        self
    }
}

impl AddAssign for Usage {
    fn add_assign(&mut self, rhs: Self) {
        self.input_tokens = self.input_tokens.saturating_add(rhs.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(rhs.output_tokens);
        self.cache_read_input_tokens = self
            .cache_read_input_tokens
            .saturating_add(rhs.cache_read_input_tokens);
        self.cache_creation_input_tokens = self
            .cache_creation_input_tokens
            .saturating_add(rhs.cache_creation_input_tokens);
    }
}

impl Sum for Usage {
    fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
        iter.fold(Self::default(), Add::add)
    }
}

/// Why the model stopped generating.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// The model finished its answer.
    EndTurn,
    /// The model is waiting for tool results.
    ToolUse,
    /// The `max_tokens` limit was reached.
    MaxTokens,
    /// A custom stop sequence was produced.
    StopSequence,
    /// A long-running server-side turn was paused and can be resumed.
    PauseTurn,
    /// The model declined to answer.
    Refusal,
    /// The user interrupted the turn (local, not sent by the provider).
    Interrupted,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::de::DeserializeOwned;
    use std::fmt::Debug;

    fn round_trip<T: Serialize + DeserializeOwned + PartialEq + Debug>(value: &T) {
        let json = serde_json::to_string(value).unwrap();
        let back: T = serde_json::from_str(&json).unwrap();
        assert_eq!(&back, value, "round trip through {json}");
    }

    #[test]
    fn uuid_ids_serialize_as_plain_strings() {
        let id = SessionId::new();
        let json = serde_json::to_value(id).unwrap();
        assert_eq!(json, serde_json::Value::String(id.to_string()));
        round_trip(&id);
        round_trip(&TurnId::new());
        round_trip(&MessageId::new());
    }

    #[test]
    fn new_ids_are_distinct() {
        assert_ne!(MessageId::new(), MessageId::new());
    }

    #[test]
    fn tool_use_id_is_transparent() {
        let id = ToolUseId::new("toolu_01ABC");
        assert_eq!(serde_json::to_string(&id).unwrap(), "\"toolu_01ABC\"");
        assert_eq!(id.to_string(), "toolu_01ABC");
        round_trip(&id);
    }

    #[test]
    fn role_serializes_snake_case() {
        assert_eq!(serde_json::to_string(&Role::User).unwrap(), "\"user\"");
        assert_eq!(
            serde_json::to_string(&Role::Assistant).unwrap(),
            "\"assistant\""
        );
        round_trip(&Role::Assistant);
    }

    fn all_parts() -> Vec<ContentPart> {
        vec![
            ContentPart::text("hola"),
            ContentPart::Thinking {
                thinking: "pienso".into(),
                signature: Some("sig".into()),
            },
            ContentPart::Thinking {
                thinking: "sin firma".into(),
                signature: None,
            },
            ContentPart::RedactedThinking {
                data: "opaque".into(),
            },
            ContentPart::ToolUse {
                id: ToolUseId::new("toolu_1"),
                name: "grep".into(),
                input: serde_json::json!({ "pattern": "fn main" }),
            },
            ContentPart::tool_result_text(ToolUseId::new("toolu_1"), "src/main.rs:1"),
            ContentPart::ToolResult {
                tool_use_id: ToolUseId::new("toolu_2"),
                content: vec![ToolResultContent::Image {
                    media_type: "image/png".into(),
                    data: "iVBORw0KGgo=".into(),
                }],
                is_error: true,
            },
            ContentPart::Image {
                media_type: "image/jpeg".into(),
                data: "/9j/4AAQ".into(),
            },
        ]
    }

    #[test]
    fn content_parts_round_trip() {
        for part in all_parts() {
            round_trip(&part);
        }
    }

    #[test]
    fn content_parts_wire_shape() {
        insta::assert_json_snapshot!(all_parts());
    }

    #[test]
    fn tool_result_is_error_defaults_to_false() {
        let part: ContentPart = serde_json::from_value(serde_json::json!({
            "type": "tool_result",
            "tool_use_id": "toolu_9",
            "content": [{ "type": "text", "text": "ok" }]
        }))
        .unwrap();
        assert_eq!(
            part,
            ContentPart::tool_result_text(ToolUseId::new("toolu_9"), "ok")
        );
    }

    #[test]
    fn message_constructors_set_role_and_content() {
        let user = Message::user_text("hola");
        assert_eq!(user.role, Role::User);
        assert_eq!(user.content, vec![ContentPart::text("hola")]);

        let assistant = Message::assistant(all_parts());
        assert_eq!(assistant.role, Role::Assistant);
        assert_ne!(assistant.id, user.id);
    }

    #[test]
    fn message_round_trip() {
        round_trip(&Message::user_text("hola"));
        round_trip(&Message::assistant(all_parts()));
    }

    fn usage(input: u64, output: u64, read: u64, creation: u64) -> Usage {
        Usage {
            input_tokens: input,
            output_tokens: output,
            cache_read_input_tokens: read,
            cache_creation_input_tokens: creation,
        }
    }

    #[test]
    fn usage_adds_field_by_field() {
        let mut total = usage(1, 2, 3, 4) + usage(10, 20, 30, 40);
        assert_eq!(total, usage(11, 22, 33, 44));
        total += usage(1, 1, 1, 1);
        assert_eq!(total, usage(12, 23, 34, 45));
        let summed: Usage = [usage(1, 0, 0, 0), usage(0, 1, 0, 0)].into_iter().sum();
        assert_eq!(summed, usage(1, 1, 0, 0));
    }

    #[test]
    fn usage_saturates_instead_of_overflowing() {
        assert_eq!(
            usage(u64::MAX, 0, 0, 0) + usage(1, 0, 0, 0),
            usage(u64::MAX, 0, 0, 0)
        );
    }

    #[test]
    fn usage_round_trip_and_missing_fields_default_to_zero() {
        round_trip(&usage(1, 2, 3, 4));
        let partial: Usage =
            serde_json::from_value(serde_json::json!({ "input_tokens": 5, "output_tokens": 6 }))
                .unwrap();
        assert_eq!(partial, usage(5, 6, 0, 0));
    }

    #[test]
    fn stop_reason_uses_api_names() {
        let cases = [
            (StopReason::EndTurn, "end_turn"),
            (StopReason::ToolUse, "tool_use"),
            (StopReason::MaxTokens, "max_tokens"),
            (StopReason::StopSequence, "stop_sequence"),
            (StopReason::PauseTurn, "pause_turn"),
            (StopReason::Refusal, "refusal"),
            (StopReason::Interrupted, "interrupted"),
        ];
        for (reason, name) in cases {
            assert_eq!(serde_json::to_value(reason).unwrap(), name);
            round_trip(&reason);
        }
    }
}
