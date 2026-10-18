//! Model selection by role.
//!
//! Every LLM call asks for a [`ModelRole`], never for a concrete model. A pure
//! policy ([`ModelPolicy`]) resolves the role to a concrete [`ModelSpec`]
//! according to layered configuration, so which model does what can change
//! without touching code.

use std::fmt;
use std::str::FromStr;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// What an LLM call is for. The policy maps each role to a concrete model.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ModelRole {
    /// Conversation and main agent loop (read, edit, use tools).
    Main,
    /// Plans, architecture design, large refactors.
    Plan,
    /// Search subagents and broad reading of the project.
    Explore,
    /// Summarising the history when it approaches the context limit.
    Compact,
    /// Session title and short names.
    Title,
    /// Structured data for blocks (charts, Mermaid, tables).
    Component,
    /// Optional, very hard tasks on demand.
    Deep,
}

impl ModelRole {
    /// All roles, in declaration order.
    pub const ALL: [Self; 7] = [
        Self::Main,
        Self::Plan,
        Self::Explore,
        Self::Compact,
        Self::Title,
        Self::Component,
        Self::Deep,
    ];

    /// The snake_case name used in config and serialisation.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::Plan => "plan",
            Self::Explore => "explore",
            Self::Compact => "compact",
            Self::Title => "title",
            Self::Component => "component",
            Self::Deep => "deep",
        }
    }
}

impl fmt::Display for ModelRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Error returned when parsing an unknown role name.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown model role `{0}`")]
pub struct UnknownRole(pub String);

impl FromStr for ModelRole {
    type Err = UnknownRole;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|r| r.as_str() == s)
            .ok_or_else(|| UnknownRole(s.to_owned()))
    }
}

/// A concrete model id as the provider knows it (e.g. `claude-sonnet-5-5`).
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(transparent)]
pub struct ModelId(pub String);

impl ModelId {
    /// Creates an id from anything string-like.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// The id as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ModelId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for ModelId {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

impl From<String> for ModelId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_names_round_trip() {
        let cases = [
            (ModelRole::Main, "main"),
            (ModelRole::Plan, "plan"),
            (ModelRole::Explore, "explore"),
            (ModelRole::Compact, "compact"),
            (ModelRole::Title, "title"),
            (ModelRole::Component, "component"),
            (ModelRole::Deep, "deep"),
        ];
        for (role, name) in cases {
            assert_eq!(role.to_string(), name);
            assert_eq!(name.parse::<ModelRole>(), Ok(role));
            assert_eq!(serde_json::to_value(role).unwrap(), serde_json::json!(name));
            assert_eq!(
                serde_json::from_value::<ModelRole>(serde_json::json!(name)).unwrap(),
                role
            );
        }
    }

    #[test]
    fn unknown_role_is_rejected() {
        for bad in ["", "Main", "opus", "planner"] {
            assert_eq!(bad.parse::<ModelRole>(), Err(UnknownRole(bad.to_owned())));
        }
    }

    #[test]
    fn model_id_is_transparent_string() {
        let id = ModelId::from("claude-haiku-5-5");
        assert_eq!(id.as_str(), "claude-haiku-5-5");
        assert_eq!(id.to_string(), "claude-haiku-5-5");
        assert_eq!(
            serde_json::to_value(&id).unwrap(),
            serde_json::json!("claude-haiku-5-5")
        );
    }
}
