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

/// Extended thinking configuration for a request.
///
/// Serialises with a `type` tag, mirroring the Messages API shape
/// (`{"type": "enabled", "budget_tokens": 8000}`).
///
/// `Adaptive` lets the model decide how much to think (supported by recent
/// models); it carries no budget. Adapters for models that do not support it
/// should fall back to `Disabled`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ThinkingConfig {
    /// No extended thinking.
    #[default]
    Disabled,
    /// Extended thinking with a fixed token budget.
    Enabled {
        /// Maximum tokens the model may spend thinking.
        budget_tokens: u32,
    },
    /// The model chooses its own thinking effort.
    Adaptive,
}

impl ThinkingConfig {
    /// Token budget, if this configuration has one.
    #[must_use]
    pub const fn budget_tokens(self) -> Option<u32> {
        match self {
            Self::Enabled { budget_tokens } => Some(budget_tokens),
            Self::Disabled | Self::Adaptive => None,
        }
    }
}

/// Fully specified model choice for one role.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ModelSpec {
    /// Primary model id.
    pub id: ModelId,
    /// `max_tokens` sent with each request.
    pub max_tokens: u32,
    /// Extended thinking configuration.
    #[serde(default)]
    pub thinking: ThinkingConfig,
    /// Models to try, in order, after 529/5xx errors on `id`.
    #[serde(default)]
    pub fallbacks: Vec<ModelId>,
}

impl ModelSpec {
    /// A spec with no thinking and no fallbacks.
    #[must_use]
    pub fn new(id: impl Into<ModelId>, max_tokens: u32) -> Self {
        Self {
            id: id.into(),
            max_tokens,
            thinking: ThinkingConfig::Disabled,
            fallbacks: Vec::new(),
        }
    }

    /// Builder: sets the thinking configuration.
    #[must_use]
    pub fn with_thinking(mut self, thinking: ThinkingConfig) -> Self {
        self.thinking = thinking;
        self
    }

    /// Builder: sets the fallback list.
    #[must_use]
    pub fn with_fallbacks<I, T>(mut self, fallbacks: I) -> Self
    where
        I: IntoIterator<Item = T>,
        T: Into<ModelId>,
    {
        self.fallbacks = fallbacks.into_iter().map(Into::into).collect();
        self
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

    #[test]
    fn thinking_serialises_like_the_api() {
        let cases = [
            (
                ThinkingConfig::Disabled,
                serde_json::json!({"type": "disabled"}),
            ),
            (
                ThinkingConfig::Enabled {
                    budget_tokens: 8000,
                },
                serde_json::json!({"type": "enabled", "budget_tokens": 8000}),
            ),
            (
                ThinkingConfig::Adaptive,
                serde_json::json!({"type": "adaptive"}),
            ),
        ];
        for (cfg, json) in cases {
            assert_eq!(serde_json::to_value(cfg).unwrap(), json);
            assert_eq!(serde_json::from_value::<ThinkingConfig>(json).unwrap(), cfg);
        }
    }

    #[test]
    fn thinking_budget() {
        assert_eq!(ThinkingConfig::Disabled.budget_tokens(), None);
        assert_eq!(ThinkingConfig::Adaptive.budget_tokens(), None);
        assert_eq!(
            ThinkingConfig::Enabled { budget_tokens: 5 }.budget_tokens(),
            Some(5)
        );
    }

    #[test]
    fn model_spec_defaults_optional_fields() {
        let spec: ModelSpec =
            serde_json::from_value(serde_json::json!({"id": "m", "max_tokens": 10})).unwrap();
        assert_eq!(spec, ModelSpec::new("m", 10));
        let full = ModelSpec::new("m", 10)
            .with_thinking(ThinkingConfig::Enabled { budget_tokens: 4 })
            .with_fallbacks(["a", "b"]);
        let back: ModelSpec = serde_json::from_value(serde_json::to_value(&full).unwrap()).unwrap();
        assert_eq!(back, full);
        assert_eq!(full.fallbacks, vec![ModelId::from("a"), ModelId::from("b")]);
    }
}
