//! Conversation types: sessions, turns, messages and their content parts.
//!
//! These types are the contract shared by the ports and the model adapters.
//! They map 1:1 onto the shape of the Anthropic Messages API without
//! depending on it, so the adapter is a mechanical translation.

use std::fmt;

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
}
