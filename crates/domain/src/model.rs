//! Model selection by role.
//!
//! Every LLM call asks for a [`ModelRole`], never for a concrete model. A pure
//! policy ([`ModelPolicy`]) resolves the role to a concrete [`ModelSpec`]
//! according to layered configuration, so which model does what can change
//! without touching code.

use std::collections::{BTreeMap, BTreeSet};
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

/// Who serves a model, written as the prefix of its id
/// (`opencode-go:glm-5.3`). An id without a known prefix is served by the
/// default provider (Anthropic unless configured otherwise).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderId {
    /// The Anthropic Messages API, with an Anthropic API key.
    Anthropic,
    /// OpenCode Go: the flat-rate subscription, with an OpenCode key.
    OpencodeGo,
    /// OpenCode Zen: pay as you go, with the same OpenCode key.
    OpencodeZen,
}

impl ProviderId {
    /// Every provider, in declaration order.
    pub const ALL: [Self; 3] = [Self::Anthropic, Self::OpencodeGo, Self::OpencodeZen];

    /// The prefix used in model ids and config.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::OpencodeGo => "opencode-go",
            Self::OpencodeZen => "opencode-zen",
        }
    }

    /// The account whose key this provider uses: Go and Zen share one.
    #[must_use]
    pub const fn account(self) -> Account {
        match self {
            Self::Anthropic => Account::Anthropic,
            Self::OpencodeGo | Self::OpencodeZen => Account::OpenCode,
        }
    }
}

impl fmt::Display for ProviderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ProviderId {
    type Err = UnknownProvider;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|p| p.as_str() == s)
            .ok_or_else(|| UnknownProvider(s.to_owned()))
    }
}

/// Error returned when parsing an unknown provider name.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown provider `{0}`")]
pub struct UnknownProvider(pub String);

/// An account with its own API key, shared by one or more providers.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum Account {
    /// Anthropic (`ANTHROPIC_API_KEY`).
    Anthropic,
    /// OpenCode, for Go and Zen (`OPENCODE_API_KEY`).
    OpenCode,
}

impl Account {
    /// Every account, in declaration order.
    pub const ALL: [Self; 2] = [Self::Anthropic, Self::OpenCode];

    /// The name used in `/login` and `/logout`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::OpenCode => "opencode",
        }
    }

    /// The name shown to people.
    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Anthropic => "Anthropic",
            Self::OpenCode => "OpenCode",
        }
    }

    /// The environment variable that holds the key; it is also the name of
    /// the keyring entry (service `crest`).
    #[must_use]
    pub const fn key_name(self) -> &'static str {
        match self {
            Self::Anthropic => "ANTHROPIC_API_KEY",
            Self::OpenCode => "OPENCODE_API_KEY",
        }
    }
}

impl fmt::Display for Account {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Account {
    type Err = UnknownProvider;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|a| a.as_str() == s)
            .ok_or_else(|| UnknownProvider(s.to_owned()))
    }
}

/// A concrete model id as the provider knows it (e.g. `claude-sonnet-5-5`),
/// optionally prefixed with its provider (`opencode-go:glm-5.3`).
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

    /// The provider prefix and the id the provider knows, or `None` when
    /// the id has no known prefix (the default provider serves it).
    #[must_use]
    pub fn provider(&self) -> Option<(ProviderId, &str)> {
        let (prefix, rest) = self.0.split_once(':')?;
        let provider = prefix.parse().ok()?;
        (!rest.is_empty()).then_some((provider, rest))
    }

    /// The id with `provider` as prefix.
    #[must_use]
    pub fn with_provider(provider: ProviderId, id: &str) -> Self {
        Self(format!("{provider}:{id}"))
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

/// Which level of the resolution order chose the model id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SpecSource {
    /// Built-in defaults (level 5).
    Default,
    /// `~/.config/crest/init.lua` (level 4).
    Global,
    /// `<workspace>/.crest/init.lua` (level 3).
    Workspace,
    /// `/model …` or a shortcut for this turn (level 1).
    TurnOverride,
}

/// What a caller asks the policy for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ResolveRequest {
    /// Role the caller would use by default (usually `main`).
    pub role: ModelRole,
    /// Turn override (level 1): an alias, a role name or a literal model id.
    #[serde(default)]
    pub turn_override: Option<String>,
    /// Role pinned by a tool or plugin (level 2), e.g. a subagent asking for
    /// `explore`. Replaces `role`.
    #[serde(default)]
    pub pinned_role: Option<ModelRole>,
}

impl ResolveRequest {
    /// A request for `role` with no override and no pinned role.
    #[must_use]
    pub const fn new(role: ModelRole) -> Self {
        Self {
            role,
            turn_override: None,
            pinned_role: None,
        }
    }
}

/// Result of resolving a role.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ResolvedModel {
    /// Role whose spec was used (after pinning and role-name overrides).
    pub role: ModelRole,
    /// Concrete spec, with aliases expanded.
    pub spec: ModelSpec,
    /// Level that chose `spec.id`.
    pub source: SpecSource,
    /// Problems found against the available models, if they are known.
    #[serde(default)]
    pub warnings: Vec<ModelWarning>,
}

impl ResolvedModel {
    /// Models to try in order on 529/5xx errors: `spec.id`, then fallbacks.
    pub fn fallback_chain(&self) -> impl Iterator<Item = &ModelId> {
        std::iter::once(&self.spec.id).chain(self.spec.fallbacks.iter())
    }
}

/// A role points to a model the provider does not offer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, thiserror::Error)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ModelWarning {
    /// The primary model is unavailable; `replacement` is the first
    /// available fallback, or `None` if there is none (the id is kept).
    #[error("role `{role}`: model `{model}` is not available{}", replacement_note(.replacement.as_ref()))]
    UnavailableModel {
        /// Role being resolved.
        role: ModelRole,
        /// Missing model.
        model: ModelId,
        /// Model used instead.
        replacement: Option<ModelId>,
    },
    /// A fallback is unavailable and was dropped from the chain.
    #[error("role `{role}`: fallback `{model}` is not available")]
    UnavailableFallback {
        /// Role being resolved.
        role: ModelRole,
        /// Missing model.
        model: ModelId,
    },
}

fn replacement_note(replacement: Option<&ModelId>) -> String {
    replacement.map_or_else(
        || "; no fallback available".to_owned(),
        |id| format!("; using `{id}`"),
    )
}

/// Pure policy that resolves a role to a model.
///
/// Resolution order, highest first:
/// 1. turn override ([`ResolveRequest::turn_override`]);
/// 2. role pinned by a tool or plugin ([`ResolveRequest::pinned_role`]);
/// 3. `workspace` layer;
/// 4. `global` layer;
/// 5. built-in defaults ([`default_spec`], [`default_aliases`]).
///
/// Levels 3–5 merge field by field. Aliases from all layers are merged too
/// (workspace wins) and expanded once, without recursion, in ids and
/// fallbacks.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct ModelPolicy {
    /// Global configuration layer.
    pub global: RoleConfig,
    /// Workspace configuration layer.
    pub workspace: RoleConfig,
    /// Models the provider offers (`GET /v1/models`); `None` if unknown,
    /// in which case no validation happens.
    pub available: Option<BTreeSet<ModelId>>,
}

impl ModelPolicy {
    /// Policy with the given layers.
    #[must_use]
    pub const fn new(global: RoleConfig, workspace: RoleConfig) -> Self {
        Self {
            global,
            workspace,
            available: None,
        }
    }

    /// Builder: records the models the provider offers, so that
    /// [`Self::resolve`] replaces unavailable ids by their fallbacks.
    #[must_use]
    pub fn with_available_models(mut self, available: &[ModelId]) -> Self {
        self.available = Some(available.iter().cloned().collect());
        self
    }

    /// Warnings for every role whose models are not in `available`
    /// (for the status bar at startup).
    #[must_use]
    pub fn validate_against(&self, available: &[ModelId]) -> Vec<ModelWarning> {
        let checked = self.clone().with_available_models(available);
        ModelRole::ALL
            .into_iter()
            .flat_map(|role| checked.resolve(&ResolveRequest::new(role)).warnings)
            .collect()
    }

    fn check(resolved: &mut ResolvedModel, available: &BTreeSet<ModelId>) {
        let role = resolved.role;
        let spec = &mut resolved.spec;
        let mut fallbacks = Vec::new();
        for model in std::mem::take(&mut spec.fallbacks) {
            if available.contains(&model) {
                fallbacks.push(model);
            } else {
                resolved
                    .warnings
                    .push(ModelWarning::UnavailableFallback { role, model });
            }
        }
        if !available.contains(&spec.id) {
            let replacement = (!fallbacks.is_empty()).then(|| fallbacks.remove(0));
            resolved.warnings.insert(
                0,
                ModelWarning::UnavailableModel {
                    role,
                    model: spec.id.clone(),
                    replacement: replacement.clone(),
                },
            );
            if let Some(id) = replacement {
                spec.id = id;
            }
        }
        spec.fallbacks = fallbacks;
    }

    /// Merged aliases of every layer.
    #[must_use]
    pub fn aliases(&self) -> BTreeMap<String, ModelId> {
        let mut aliases = default_aliases();
        for layer in [&self.global, &self.workspace] {
            aliases.extend(layer.aliases.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
        aliases
    }

    /// Expands `name` if it is an alias, otherwise takes it as a literal id.
    #[must_use]
    pub fn expand(&self, name: &str) -> ModelId {
        Self::expand_with(&self.aliases(), name)
    }

    fn expand_with(aliases: &BTreeMap<String, ModelId>, name: &str) -> ModelId {
        aliases
            .get(name)
            .cloned()
            .unwrap_or_else(|| ModelId::new(name))
    }

    /// Spec for `role` from levels 3–5, with aliases expanded.
    fn layered(&self, role: ModelRole, aliases: &BTreeMap<String, ModelId>) -> ResolvedModel {
        let mut spec = default_spec(role);
        let mut source = SpecSource::Default;
        for (layer, layer_source) in [
            (&self.global, SpecSource::Global),
            (&self.workspace, SpecSource::Workspace),
        ] {
            if let Some(patch) = layer.role(role) {
                patch.apply_to(&mut spec);
                if patch.id.is_some() {
                    source = layer_source;
                }
            }
        }
        spec.id = Self::expand_with(aliases, spec.id.as_str());
        let fallbacks = std::mem::take(&mut spec.fallbacks);
        for fallback in fallbacks {
            let fallback = Self::expand_with(aliases, fallback.as_str());
            if fallback != spec.id && !spec.fallbacks.contains(&fallback) {
                spec.fallbacks.push(fallback);
            }
        }
        ResolvedModel {
            role,
            spec,
            source,
            warnings: Vec::new(),
        }
    }

    /// Resolves a request following the five-level order.
    ///
    /// A turn override that names a role (`/model plan`) uses that role's
    /// whole spec. Otherwise it is an alias or a literal id that replaces
    /// only `spec.id`, keeping the role's `max_tokens`, thinking and
    /// fallbacks. Aliases are checked before role names.
    ///
    /// When the available models are known ([`Self::with_available_models`]),
    /// unavailable fallbacks are dropped and an unavailable primary id is
    /// replaced by the first available fallback, with a warning for each.
    #[must_use]
    pub fn resolve(&self, req: &ResolveRequest) -> ResolvedModel {
        let mut resolved = self.resolve_unchecked(req);
        if let Some(available) = &self.available {
            Self::check(&mut resolved, available);
        }
        resolved
    }

    fn resolve_unchecked(&self, req: &ResolveRequest) -> ResolvedModel {
        let aliases = self.aliases();
        let role = req.pinned_role.unwrap_or(req.role);
        let over = req
            .turn_override
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let Some(over) = over else {
            return self.layered(role, &aliases);
        };
        if let (false, Ok(over_role)) = (aliases.contains_key(over), over.parse::<ModelRole>()) {
            let mut resolved = self.layered(over_role, &aliases);
            resolved.source = SpecSource::TurnOverride;
            return resolved;
        }
        let mut resolved = self.layered(role, &aliases);
        let id = Self::expand_with(&aliases, over);
        resolved.spec.fallbacks.retain(|f| *f != id);
        resolved.spec.id = id;
        resolved.source = SpecSource::TurnOverride;
        resolved
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
    fn model_ids_carry_an_optional_provider() {
        let id = ModelId::from("opencode-go:glm-5.3");
        assert_eq!(id.provider(), Some((ProviderId::OpencodeGo, "glm-5.3")));
        assert_eq!(
            ModelId::with_provider(ProviderId::OpencodeZen, "claude-sonnet-5-5").as_str(),
            "opencode-zen:claude-sonnet-5-5"
        );
        for plain in [
            "claude-sonnet-5-5",
            "ollama:qwen3",
            "opencode-go:",
            "anthropic",
        ] {
            assert_eq!(ModelId::from(plain).provider(), None, "{plain}");
        }
        assert_eq!(ProviderId::OpencodeZen.account(), Account::OpenCode);
        assert_eq!("opencode".parse::<Account>(), Ok(Account::OpenCode));
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

    fn layer(roles: serde_json::Value) -> RoleConfig {
        serde_json::from_value(roles).unwrap()
    }

    fn sample_policy() -> ModelPolicy {
        ModelPolicy::new(
            layer(serde_json::json!({
                "roles": {
                    "main": {"id": "g-main", "max_tokens": 1000, "fallbacks": ["g-fb"]},
                    "explore": {"id": "g-explore"},
                    "title": {"max_tokens": 50}
                },
                "aliases": {"fast": "g-fast", "big": "g-big"}
            })),
            layer(serde_json::json!({
                "roles": {
                    "main": {"id": "w-main"},
                    "plan": {"thinking": {"type": "disabled"}}
                },
                "aliases": {"fast": "w-fast"}
            })),
        )
    }

    fn req(role: ModelRole, over: Option<&str>, pinned: Option<ModelRole>) -> ResolveRequest {
        ResolveRequest {
            role,
            turn_override: over.map(str::to_owned),
            pinned_role: pinned,
        }
    }

    /// (request, role, id, max_tokens, thinking, fallbacks, source)
    type Case = (
        ResolveRequest,
        ModelRole,
        &'static str,
        u32,
        ThinkingConfig,
        Vec<&'static str>,
        SpecSource,
    );

    #[test]
    fn resolve_five_levels() {
        use ModelRole::{Compact, Explore, Main, Plan, Title};
        use SpecSource::{Default, Global, TurnOverride, Workspace};
        let p = sample_policy();
        let on = |b| ThinkingConfig::Enabled { budget_tokens: b };
        // (request, role, id, max_tokens, thinking, fallbacks, source)
        let cases: Vec<Case> = vec![
            // 5. defaults
            (
                req(Compact, None, None),
                Compact,
                "claude-haiku-5-5",
                4_000,
                ThinkingConfig::Disabled,
                vec![],
                Default,
            ),
            // 4. global: id from global, thinking from defaults
            (
                req(Explore, None, None),
                Explore,
                "g-explore",
                8_000,
                ThinkingConfig::Disabled,
                vec![],
                Global,
            ),
            // 4. global patch without id keeps the default id
            (
                req(Title, None, None),
                Title,
                "claude-haiku-5-5",
                50,
                ThinkingConfig::Disabled,
                vec![],
                Default,
            ),
            // 3. workspace id over global, global max_tokens and fallbacks kept
            (
                req(Main, None, None),
                Main,
                "w-main",
                1000,
                on(8_000),
                vec!["g-fb"],
                Workspace,
            ),
            // 3. workspace patch without id over defaults
            (
                req(Plan, None, None),
                Plan,
                "claude-opus-5-5",
                32_000,
                ThinkingConfig::Disabled,
                vec!["claude-sonnet-5-5"],
                Default,
            ),
            // 2. pinned role replaces the requested one
            (
                req(Main, None, Some(Explore)),
                Explore,
                "g-explore",
                8_000,
                ThinkingConfig::Disabled,
                vec![],
                Global,
            ),
            // 1. override with alias (workspace alias wins) keeps the role's other fields
            (
                req(Main, Some("fast"), None),
                Main,
                "w-fast",
                1000,
                on(8_000),
                vec!["g-fb"],
                TurnOverride,
            ),
            // 1. override with built-in alias
            (
                req(Main, Some("opus"), None),
                Main,
                "claude-opus-5-5",
                1000,
                on(8_000),
                vec!["g-fb"],
                TurnOverride,
            ),
            // 1. override beats pinned role, applied on the pinned role's spec
            (
                req(Main, Some("big"), Some(Compact)),
                Compact,
                "g-big",
                4_000,
                ThinkingConfig::Disabled,
                vec![],
                TurnOverride,
            ),
            // 1. override with a role name uses that role's spec
            (
                req(Main, Some("plan"), None),
                Plan,
                "claude-opus-5-5",
                32_000,
                ThinkingConfig::Disabled,
                vec!["claude-sonnet-5-5"],
                TurnOverride,
            ),
            // 1. override with a literal id
            (
                req(Explore, Some("my-model"), None),
                Explore,
                "my-model",
                8_000,
                ThinkingConfig::Disabled,
                vec![],
                TurnOverride,
            ),
            // 1. override equal to a fallback removes it from fallbacks
            (
                req(Main, Some("g-fb"), None),
                Main,
                "g-fb",
                1000,
                on(8_000),
                vec![],
                TurnOverride,
            ),
            // blank override is ignored
            (
                req(Main, Some("  "), None),
                Main,
                "w-main",
                1000,
                on(8_000),
                vec!["g-fb"],
                Workspace,
            ),
        ];
        for (i, (r, role, id, max, thinking, fallbacks, source)) in cases.into_iter().enumerate() {
            let want = ResolvedModel {
                role,
                spec: ModelSpec::new(id, max)
                    .with_thinking(thinking)
                    .with_fallbacks(fallbacks),
                source,
                warnings: vec![],
            };
            assert_eq!(p.resolve(&r), want, "case {i}: {r:?}");
        }
    }

    #[test]
    fn empty_policy_resolves_to_defaults() {
        let p = ModelPolicy::default();
        for role in ModelRole::ALL {
            let got = p.resolve(&ResolveRequest::new(role));
            assert_eq!(got.spec, default_spec(role));
            assert_eq!((got.role, got.source), (role, SpecSource::Default));
        }
    }

    #[test]
    fn aliases_expand_in_ids_and_fallbacks() {
        let p = ModelPolicy::new(
            layer(serde_json::json!({
                "roles": {"main": {"id": "opus", "fallbacks": ["sonnet", "opus", "haiku", "sonnet"]}}
            })),
            RoleConfig::default(),
        );
        let got = p.resolve(&ResolveRequest::new(ModelRole::Main));
        assert_eq!(got.spec.id, ModelId::from("claude-opus-5-5"));
        assert_eq!(
            got.spec.fallbacks,
            vec![
                ModelId::from("claude-sonnet-5-5"),
                ModelId::from("claude-haiku-5-5")
            ]
        );
        assert_eq!(p.expand("haiku"), ModelId::from("claude-haiku-5-5"));
        assert_eq!(p.expand("x"), ModelId::from("x"));
    }

    fn ids(names: &[&str]) -> Vec<ModelId> {
        names.iter().copied().map(ModelId::from).collect()
    }

    #[test]
    fn unavailable_models_use_first_valid_fallback() {
        let p = ModelPolicy::new(
            layer(serde_json::json!({
                "roles": {"main": {"id": "a", "fallbacks": ["b", "c", "d"]}}
            })),
            RoleConfig::default(),
        );
        let unavailable =
            |role, model: &str, replacement: Option<&str>| ModelWarning::UnavailableModel {
                role,
                model: model.into(),
                replacement: replacement.map(ModelId::from),
            };
        let dropped = |role, model: &str| ModelWarning::UnavailableFallback {
            role,
            model: model.into(),
        };
        let main = ModelRole::Main;
        // (available, expected chain, expected warnings)
        let cases: Vec<(Vec<&str>, Vec<&str>, Vec<ModelWarning>)> = vec![
            (vec!["a", "b", "c", "d"], vec!["a", "b", "c", "d"], vec![]),
            (
                vec!["b", "c", "d"],
                vec!["b", "c", "d"],
                vec![unavailable(main, "a", Some("b"))],
            ),
            (
                vec!["c", "d"],
                vec!["c", "d"],
                vec![unavailable(main, "a", Some("c")), dropped(main, "b")],
            ),
            (
                vec!["a", "d"],
                vec!["a", "d"],
                vec![dropped(main, "b"), dropped(main, "c")],
            ),
            (
                vec![],
                vec!["a"],
                vec![
                    unavailable(main, "a", None),
                    dropped(main, "b"),
                    dropped(main, "c"),
                    dropped(main, "d"),
                ],
            ),
        ];
        for (available, chain, warnings) in cases {
            let got = p
                .clone()
                .with_available_models(&ids(&available))
                .resolve(&ResolveRequest::new(main));
            assert_eq!(
                got.fallback_chain().cloned().collect::<Vec<_>>(),
                ids(&chain),
                "{available:?}"
            );
            assert_eq!(got.warnings, warnings, "{available:?}");
        }
    }

    #[test]
    fn turn_override_is_validated_too() {
        let p = ModelPolicy::default().with_available_models(&ids(&["claude-haiku-5-5"]));
        let got = p.resolve(&req(ModelRole::Main, Some("nope"), None));
        assert_eq!(got.spec.id, ModelId::from("claude-haiku-5-5"));
        assert_eq!(
            got.warnings,
            vec![ModelWarning::UnavailableModel {
                role: ModelRole::Main,
                model: "nope".into(),
                replacement: Some("claude-haiku-5-5".into()),
            }]
        );
    }

    #[test]
    fn validate_against_reports_every_role() {
        let p = ModelPolicy::default();
        let all = ids(&[
            "claude-sonnet-5-5",
            "claude-opus-5-5",
            "claude-haiku-5-5",
            "claude-fable-5-1",
        ]);
        assert_eq!(p.validate_against(&all), vec![]);
        let warnings = p.validate_against(&ids(&["claude-sonnet-5-5", "claude-haiku-5-5"]));
        let text: Vec<String> = warnings.iter().map(ToString::to_string).collect();
        assert_eq!(
            text,
            [
                "role `plan`: model `claude-opus-5-5` is not available; using `claude-sonnet-5-5`",
                "role `deep`: model `claude-fable-5-1` is not available; using `claude-sonnet-5-5`",
                "role `deep`: fallback `claude-opus-5-5` is not available",
            ]
        );
        // validate_against does not change the policy itself.
        assert_eq!(p.available, None);
        assert_eq!(
            p.validate_against(&[])[0].to_string(),
            "role `main`: model `claude-sonnet-5-5` is not available; no fallback available"
        );
    }

    #[test]
    fn fallback_chain_without_validation() {
        let got = ModelPolicy::default().resolve(&ResolveRequest::new(ModelRole::Deep));
        assert_eq!(
            got.fallback_chain()
                .map(ModelId::as_str)
                .collect::<Vec<_>>(),
            ["claude-fable-5-1", "claude-opus-5-5", "claude-sonnet-5-5"]
        );
        assert!(got.warnings.is_empty());
    }
}
