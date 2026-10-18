//! Model selection by role.
//!
//! Every LLM call asks for a [`ModelRole`], never for a concrete model. A pure
//! policy ([`ModelPolicy`]) resolves the role to a concrete [`ModelSpec`]
//! according to layered configuration, so which model does what can change
//! without touching code.

use std::collections::BTreeMap;
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

/// Prices of one model, in USD per million tokens.
///
/// Prices change, so they come from configuration; nothing here hardcodes
/// them.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
pub struct ModelPricing {
    /// Uncached input tokens.
    pub input_per_mtok: f64,
    /// Output tokens (including thinking).
    pub output_per_mtok: f64,
    /// Input tokens read from the prompt cache.
    pub cache_read_per_mtok: f64,
    /// Input tokens written to the prompt cache.
    pub cache_write_per_mtok: f64,
}

impl ModelPricing {
    /// Cost in USD of one response's usage counts.
    #[must_use]
    pub fn cost(
        &self,
        input_tokens: u64,
        output_tokens: u64,
        cache_read_tokens: u64,
        cache_write_tokens: u64,
    ) -> f64 {
        const PER: f64 = 1_000_000.0;
        (input_tokens as f64 * self.input_per_mtok
            + output_tokens as f64 * self.output_per_mtok
            + cache_read_tokens as f64 * self.cache_read_per_mtok
            + cache_write_tokens as f64 * self.cache_write_per_mtok)
            / PER
    }
}

/// Partial [`ModelSpec`] as written in one configuration layer.
///
/// Layers merge field by field: a `Some` field in a higher layer replaces the
/// same field of the lower one, a `None` field keeps it. `fallbacks` is
/// replaced as a whole list, never concatenated.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct ModelSpecPatch {
    /// Primary model id or alias.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<ModelId>,
    /// `max_tokens` override.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    /// Thinking override.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ThinkingConfig>,
    /// Fallback list override.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fallbacks: Option<Vec<ModelId>>,
}

impl ModelSpecPatch {
    /// Returns a patch where every `Some` field of `higher` wins over `self`.
    #[must_use]
    pub fn overlay(&self, higher: &Self) -> Self {
        Self {
            id: higher.id.clone().or_else(|| self.id.clone()),
            max_tokens: higher.max_tokens.or(self.max_tokens),
            thinking: higher.thinking.or(self.thinking),
            fallbacks: higher.fallbacks.clone().or_else(|| self.fallbacks.clone()),
        }
    }

    /// Applies the `Some` fields of this patch onto `spec`.
    pub fn apply_to(&self, spec: &mut ModelSpec) {
        if let Some(id) = &self.id {
            spec.id = id.clone();
        }
        if let Some(max_tokens) = self.max_tokens {
            spec.max_tokens = max_tokens;
        }
        if let Some(thinking) = self.thinking {
            spec.thinking = thinking;
        }
        if let Some(fallbacks) = &self.fallbacks {
            spec.fallbacks.clone_from(fallbacks);
        }
    }

    /// `true` if the patch sets no field.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }
}

impl From<ModelSpec> for ModelSpecPatch {
    fn from(spec: ModelSpec) -> Self {
        Self {
            id: Some(spec.id),
            max_tokens: Some(spec.max_tokens),
            thinking: Some(spec.thinking),
            fallbacks: Some(spec.fallbacks),
        }
    }
}

/// One configuration layer (`app.models` of a single `init.lua`).
///
/// Ids anywhere in the layer (role ids and fallbacks) may be aliases; they
/// are expanded when the policy resolves a role.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct RoleConfig {
    /// Per-role partial specs.
    pub roles: BTreeMap<ModelRole, ModelSpecPatch>,
    /// Short names (`opus`) to model ids.
    pub aliases: BTreeMap<String, ModelId>,
}

impl RoleConfig {
    /// Merges `higher` over `self`: roles field by field, aliases key by key.
    #[must_use]
    pub fn overlay(&self, higher: &Self) -> Self {
        let mut out = self.clone();
        for (role, patch) in &higher.roles {
            let merged = out
                .roles
                .get(role)
                .map_or_else(|| patch.clone(), |p| p.overlay(patch));
            out.roles.insert(*role, merged);
        }
        out.aliases
            .extend(higher.aliases.iter().map(|(k, v)| (k.clone(), v.clone())));
        out
    }

    /// The patch for `role`, if this layer has one.
    #[must_use]
    pub fn role(&self, role: ModelRole) -> Option<&ModelSpecPatch> {
        self.roles.get(&role)
    }

    /// Expands `name` if it is an alias of this layer.
    #[must_use]
    pub fn alias(&self, name: &str) -> Option<&ModelId> {
        self.aliases.get(name)
    }
}

const SONNET: &str = "claude-sonnet-5-5";
const OPUS: &str = "claude-opus-5-5";
const HAIKU: &str = "claude-haiku-5-5";
const FABLE: &str = "claude-fable-5-1";

/// Built-in spec for `role` (level 5 of the resolution order).
///
/// These are configuration defaults shipped in the binary; any field can be
/// overridden from `init.lua`.
#[must_use]
pub fn default_spec(role: ModelRole) -> ModelSpec {
    let thinking = |budget_tokens| ThinkingConfig::Enabled { budget_tokens };
    match role {
        ModelRole::Main => ModelSpec::new(SONNET, 16_000)
            .with_thinking(thinking(8_000))
            .with_fallbacks([HAIKU]),
        ModelRole::Plan => ModelSpec::new(OPUS, 32_000)
            .with_thinking(thinking(16_000))
            .with_fallbacks([SONNET]),
        ModelRole::Explore => ModelSpec::new(HAIKU, 8_000),
        ModelRole::Compact => ModelSpec::new(HAIKU, 4_000),
        ModelRole::Title => ModelSpec::new(HAIKU, 1_024),
        ModelRole::Component => ModelSpec::new(SONNET, 16_000).with_fallbacks([HAIKU]),
        ModelRole::Deep => ModelSpec::new(FABLE, 32_000)
            .with_thinking(thinking(16_000))
            .with_fallbacks([OPUS, SONNET]),
    }
}

/// Built-in aliases (`opus`, `sonnet`, `haiku`, `fable`).
#[must_use]
pub fn default_aliases() -> BTreeMap<String, ModelId> {
    [
        ("opus", OPUS),
        ("sonnet", SONNET),
        ("haiku", HAIKU),
        ("fable", FABLE),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), ModelId::from(v)))
    .collect()
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

    #[test]
    fn pricing_cost_table() {
        let p = ModelPricing {
            input_per_mtok: 3.0,
            output_per_mtok: 15.0,
            cache_read_per_mtok: 0.3,
            cache_write_per_mtok: 3.75,
        };
        let cases: [((u64, u64, u64, u64), f64); 6] = [
            ((0, 0, 0, 0), 0.0),
            ((1_000_000, 0, 0, 0), 3.0),
            ((0, 1_000_000, 0, 0), 15.0),
            ((0, 0, 1_000_000, 0), 0.3),
            ((0, 0, 0, 1_000_000), 3.75),
            ((1000, 500, 2000, 4000), 0.003 + 0.0075 + 0.0006 + 0.015),
        ];
        for ((i, o, cr, cw), want) in cases {
            let got = p.cost(i, o, cr, cw);
            assert!(
                (got - want).abs() < 1e-12,
                "{i},{o},{cr},{cw}: {got} != {want}"
            );
        }
        assert!(ModelPricing::default().cost(9, 9, 9, 9).abs() < f64::EPSILON);
    }

    fn patch(
        id: Option<&str>,
        max_tokens: Option<u32>,
        thinking: Option<ThinkingConfig>,
        fallbacks: Option<&[&str]>,
    ) -> ModelSpecPatch {
        ModelSpecPatch {
            id: id.map(ModelId::from),
            max_tokens,
            thinking,
            fallbacks: fallbacks.map(|f| f.iter().copied().map(ModelId::from).collect()),
        }
    }

    #[test]
    fn patch_overlay_is_field_by_field() {
        let on = Some(ThinkingConfig::Enabled { budget_tokens: 1 });
        let cases = [
            // (lower, higher, expected)
            (
                patch(None, None, None, None),
                patch(None, None, None, None),
                patch(None, None, None, None),
            ),
            (
                patch(Some("a"), Some(1), None, None),
                patch(None, None, None, None),
                patch(Some("a"), Some(1), None, None),
            ),
            (
                patch(Some("a"), Some(1), None, None),
                patch(Some("b"), None, None, None),
                patch(Some("b"), Some(1), None, None),
            ),
            (
                patch(Some("a"), None, on, Some(&["x"])),
                patch(None, Some(2), None, None),
                patch(Some("a"), Some(2), on, Some(&["x"])),
            ),
            (
                patch(None, None, None, Some(&["x", "y"])),
                patch(None, None, None, Some(&[])),
                patch(None, None, None, Some(&[])),
            ),
            (
                patch(None, None, on, None),
                patch(None, None, Some(ThinkingConfig::Disabled), None),
                patch(None, None, Some(ThinkingConfig::Disabled), None),
            ),
        ];
        for (i, (lower, higher, want)) in cases.into_iter().enumerate() {
            assert_eq!(lower.overlay(&higher), want, "case {i}");
        }
    }

    #[test]
    fn patch_apply_and_empty() {
        let mut spec = ModelSpec::new("a", 10).with_fallbacks(["x"]);
        patch(None, Some(20), None, None).apply_to(&mut spec);
        assert_eq!(spec, ModelSpec::new("a", 20).with_fallbacks(["x"]));
        patch(Some("b"), None, Some(ThinkingConfig::Adaptive), Some(&[])).apply_to(&mut spec);
        assert_eq!(
            spec,
            ModelSpec::new("b", 20).with_thinking(ThinkingConfig::Adaptive)
        );
        assert!(ModelSpecPatch::default().is_empty());
        assert!(!ModelSpecPatch::from(spec).is_empty());
    }

    #[test]
    fn role_config_overlay_and_json_shape() {
        let lower: RoleConfig = serde_json::from_value(serde_json::json!({
            "roles": {
                "main": {"id": "sonnet", "max_tokens": 16000, "fallbacks": ["haiku"]},
                "plan": {"id": "opus"}
            },
            "aliases": {"opus": "claude-opus-5-5", "sonnet": "claude-sonnet-5-5"}
        }))
        .unwrap();
        let higher: RoleConfig = serde_json::from_value(serde_json::json!({
            "roles": {"main": {"max_tokens": 4000}, "explore": {"id": "haiku"}},
            "aliases": {"opus": "claude-opus-6"}
        }))
        .unwrap();
        let merged = lower.overlay(&higher);
        assert_eq!(
            merged.role(ModelRole::Main),
            Some(&patch(Some("sonnet"), Some(4000), None, Some(&["haiku"])))
        );
        assert_eq!(
            merged.role(ModelRole::Plan),
            Some(&patch(Some("opus"), None, None, None))
        );
        assert_eq!(
            merged.role(ModelRole::Explore),
            Some(&patch(Some("haiku"), None, None, None))
        );
        assert_eq!(merged.role(ModelRole::Deep), None);
        assert_eq!(merged.alias("opus"), Some(&ModelId::from("claude-opus-6")));
        assert_eq!(
            merged.alias("sonnet"),
            Some(&ModelId::from("claude-sonnet-5-5"))
        );
        assert_eq!(merged.alias("haiku"), None);
        assert_eq!(
            serde_json::from_value::<RoleConfig>(serde_json::json!({})).unwrap(),
            RoleConfig::default()
        );
    }

    #[test]
    fn builtin_defaults_table() {
        let on = |b| ThinkingConfig::Enabled { budget_tokens: b };
        let off = ThinkingConfig::Disabled;
        let cases: [(ModelRole, &str, u32, ThinkingConfig, &[&str]); 7] = [
            (
                ModelRole::Main,
                "claude-sonnet-5-5",
                16_000,
                on(8_000),
                &["claude-haiku-5-5"],
            ),
            (
                ModelRole::Plan,
                "claude-opus-5-5",
                32_000,
                on(16_000),
                &["claude-sonnet-5-5"],
            ),
            (ModelRole::Explore, "claude-haiku-5-5", 8_000, off, &[]),
            (ModelRole::Compact, "claude-haiku-5-5", 4_000, off, &[]),
            (ModelRole::Title, "claude-haiku-5-5", 1_024, off, &[]),
            (
                ModelRole::Component,
                "claude-sonnet-5-5",
                16_000,
                off,
                &["claude-haiku-5-5"],
            ),
            (
                ModelRole::Deep,
                "claude-fable-5-1",
                32_000,
                on(16_000),
                &["claude-opus-5-5", "claude-sonnet-5-5"],
            ),
        ];
        for (role, id, max, thinking, fallbacks) in cases {
            let want = ModelSpec::new(id, max)
                .with_thinking(thinking)
                .with_fallbacks(fallbacks.iter().copied());
            assert_eq!(default_spec(role), want, "{role}");
        }
        let aliases = default_aliases();
        assert_eq!(aliases.get("opus"), Some(&ModelId::from("claude-opus-5-5")));
        assert_eq!(aliases.len(), 4);
    }
}
