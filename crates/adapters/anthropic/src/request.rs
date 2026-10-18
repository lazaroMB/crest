//! Builds the Messages API body of a request for one concrete model:
//! [`convert::messages_request`] plus the per-model rules of the API and
//! the prompt-cache plan.
//!
//! # Per-model rules
//!
//! [`MODEL_RULES`] records what each model accepts, from the official
//! model and migration docs (checked 2026-10):
//!
//! | Model | Forced `tool_choice` | `budget_tokens` | Adaptive | Programmatic tool calling |
//! | --- | --- | --- | --- | --- |
//! | Fable 5.1, Mythos 5.1 | no | no | yes | yes |
//! | Fable 5, Mythos 5 | yes | no | yes | yes |
//! | Opus 5.5 | no | no | yes | yes |
//! | Opus 5, 4.8, 4.7 | yes | no | yes | yes |
//! | Opus 4.6, Sonnet 4.6 | yes | yes (deprecated) | yes | yes |
//! | Sonnet 5.5 | no | no | yes | yes |
//! | Sonnet 5, Haiku 5.5 | yes | no | yes | yes |
//! | Haiku 4.5 | yes | yes | no | no |
//!
//! Ids match by longest prefix, so dated snapshots
//! (`claude-haiku-4-5-20251001`) follow their family. Unknown models get
//! [`ModelRules::PERMISSIVE`]: the request is sent as configured.
//!
//! What the rules change, each with a [`RequestWarning`] logged at `warn`:
//!
//! - A forced `tool_choice` (`any`, a named tool) on a model that rejects
//!   it becomes `auto`; the prompt has to steer the model instead.
//! - `budget_tokens` thinking on a model without it becomes adaptive.
//!   Adaptive thinking on a model without it is dropped. A budget is kept
//!   within the API limits (at least 1024 and below `max_tokens`), or
//!   dropped when `max_tokens` leaves no room.
//! - Web search and fetch get `allowed_callers: ["direct"]` on models
//!   without programmatic tool calling: the current tool versions default
//!   to calling them from code execution (dynamic filtering), and those
//!   models answer that default with a 400.
//!
//! # Prompt caching
//!
//! Explicit breakpoints come from [`SystemBlock::cache`] and
//! [`ToolDefinition::cache`] (see [`convert::messages_request`]). On top,
//! the request enables automatic caching (top-level `cache_control`) so
//! the growing conversation is cached turn after turn. The API allows four
//! breakpoints and the automatic one takes a slot, so at most three
//! explicit ones are kept: the last three in render order (tools, then
//! system), since a later breakpoint already covers everything before it.
//!
//! No `anthropic-beta` header is needed for any of this.
//!
//! [`SystemBlock::cache`]: application::ports::llm::SystemBlock::cache
//! [`ToolDefinition::cache`]: application::ports::llm::ToolDefinition::cache

use application::ports::llm::LlmRequest;
use domain::model::{ModelId, ThinkingConfig};

use crate::convert;
use crate::wire::{CALLER_DIRECT, CacheControl, MessagesRequest, WireTool, WireToolChoice};

/// Breakpoints the API accepts per request.
pub const MAX_CACHE_BREAKPOINTS: usize = 4;
/// Smallest thinking budget the API accepts.
pub const MIN_THINKING_BUDGET: u32 = 1024;

/// What a model accepts. See the module docs for the table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelRules {
    /// Accepts `tool_choice` `any` and `tool`.
    pub forced_tool_choice: bool,
    /// Accepts `thinking: {type: "enabled", budget_tokens}`.
    pub budget_thinking: bool,
    /// Accepts `thinking: {type: "adaptive"}`.
    pub adaptive_thinking: bool,
    /// Supports programmatic tool calling (server tools may be called from
    /// code execution).
    pub programmatic_tool_calling: bool,
}

impl ModelRules {
    /// Rules of a model this crate does not know: everything allowed.
    pub const PERMISSIVE: Self = Self {
        forced_tool_choice: true,
        budget_thinking: true,
        adaptive_thinking: true,
        programmatic_tool_calling: true,
    };

    const fn new(forced: bool, budget: bool, adaptive: bool, programmatic: bool) -> Self {
        Self {
            forced_tool_choice: forced,
            budget_thinking: budget,
            adaptive_thinking: adaptive,
            programmatic_tool_calling: programmatic,
        }
    }
}

const NO_FORCED: ModelRules = ModelRules::new(false, false, true, true);
const ADAPTIVE_ONLY: ModelRules = ModelRules::new(true, false, true, true);
const BOTH_THINKING: ModelRules = ModelRules::new(true, true, true, true);

/// Model id prefixes and their rules. See the module docs.
pub const MODEL_RULES: &[(&str, ModelRules)] = &[
    ("claude-fable-5-1", NO_FORCED),
    ("claude-mythos-5-1", NO_FORCED),
    ("claude-fable-5", ADAPTIVE_ONLY),
    ("claude-mythos-5", ADAPTIVE_ONLY),
    ("claude-opus-5-5", NO_FORCED),
    ("claude-opus-5", ADAPTIVE_ONLY),
    ("claude-opus-4-8", ADAPTIVE_ONLY),
    ("claude-opus-4-7", ADAPTIVE_ONLY),
    ("claude-opus-4-6", BOTH_THINKING),
    ("claude-sonnet-5-5", NO_FORCED),
    ("claude-sonnet-5", ADAPTIVE_ONLY),
    ("claude-sonnet-4-6", BOTH_THINKING),
    ("claude-haiku-5-5", ADAPTIVE_ONLY),
    (
        "claude-haiku-4-5",
        ModelRules::new(true, true, false, false),
    ),
];

/// The rules of `model`: the entry of [`MODEL_RULES`] with the longest
/// prefix matching the id exactly or followed by `-`.
#[must_use]
pub fn rules_for(model: &str) -> ModelRules {
    MODEL_RULES
        .iter()
        .filter(|(prefix, _)| {
            model == *prefix
                || model
                    .strip_prefix(prefix)
                    .is_some_and(|rest| rest.starts_with('-'))
        })
        .max_by_key(|(prefix, _)| prefix.len())
        .map_or(ModelRules::PERMISSIVE, |(_, rules)| *rules)
}

/// An adjustment made to fit the request to the model.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RequestWarning {
    /// A forced `tool_choice` was sent as `auto`.
    #[error("{model} rejects a forced tool_choice; sending auto")]
    ForcedToolChoice {
        /// Target model.
        model: String,
    },
    /// `budget_tokens` thinking was sent as adaptive.
    #[error("{model} rejects budget_tokens thinking; using adaptive thinking")]
    BudgetToAdaptive {
        /// Target model.
        model: String,
    },
    /// Adaptive thinking was dropped.
    #[error("{model} does not support adaptive thinking; sending without thinking")]
    AdaptiveDropped {
        /// Target model.
        model: String,
    },
    /// The thinking budget was moved inside the API limits.
    #[error("thinking budget {from} adjusted to {to} (must be >= 1024 and < max_tokens)")]
    BudgetAdjusted {
        /// Configured budget.
        from: u32,
        /// Budget sent.
        to: u32,
    },
    /// `max_tokens` leaves no room for a thinking budget.
    #[error(
        "max_tokens {max_tokens} leaves no room for a thinking budget; sending without thinking"
    )]
    BudgetDropped {
        /// Request `max_tokens`.
        max_tokens: u32,
    },
    /// Explicit cache breakpoints beyond the API limit were removed.
    #[error("removed {count} cache breakpoint(s) beyond the limit of 4")]
    CacheBreakpointsDropped {
        /// Breakpoints removed.
        count: usize,
    },
}

/// Builds the streaming body of `req` for `model`, logging every
/// [`RequestWarning`].
#[must_use]
pub fn build(req: &LlmRequest, model: &ModelId) -> MessagesRequest {
    let (body, warnings) = build_with_warnings(req, model);
    for warning in &warnings {
        tracing::warn!(model = %model, "{warning}");
    }
    body
}

/// Builds the streaming body of `req` for `model` and returns the
/// adjustments made. See the module docs.
#[must_use]
pub fn build_with_warnings(
    req: &LlmRequest,
    model: &ModelId,
) -> (MessagesRequest, Vec<RequestWarning>) {
    let rules = rules_for(model.as_str());
    let mut warnings = Vec::new();
    let mut out = convert::messages_request(req, model, true);
    let name = || model.as_str().to_owned();

    let thinking = fit_thinking(
        req.model.spec.thinking,
        rules,
        out.max_tokens,
        &mut warnings,
        name,
    );
    out.thinking = convert::thinking(thinking);
    let has_tools = !out.tools.is_empty();
    out.tool_choice = if has_tools {
        convert::tool_choice(&req.tool_choice, out.thinking.is_some())
    } else {
        None
    };
    if !rules.forced_tool_choice
        && matches!(
            out.tool_choice,
            Some(WireToolChoice::Any | WireToolChoice::Tool { .. })
        )
    {
        out.tool_choice = None;
        warnings.push(RequestWarning::ForcedToolChoice { model: name() });
    }

    if !rules.programmatic_tool_calling {
        for tool in &mut out.tools {
            if let WireTool::Server(server) = tool {
                server.allowed_callers = vec![CALLER_DIRECT];
            }
        }
    }

    plan_cache(&mut out, &mut warnings);
    (out, warnings)
}

fn fit_thinking(
    config: ThinkingConfig,
    rules: ModelRules,
    max_tokens: u32,
    warnings: &mut Vec<RequestWarning>,
    model: impl Fn() -> String,
) -> ThinkingConfig {
    match config {
        ThinkingConfig::Disabled => ThinkingConfig::Disabled,
        ThinkingConfig::Adaptive if rules.adaptive_thinking => ThinkingConfig::Adaptive,
        ThinkingConfig::Adaptive => {
            warnings.push(RequestWarning::AdaptiveDropped { model: model() });
            ThinkingConfig::Disabled
        }
        ThinkingConfig::Enabled { .. } if !rules.budget_thinking => {
            if rules.adaptive_thinking {
                warnings.push(RequestWarning::BudgetToAdaptive { model: model() });
                ThinkingConfig::Adaptive
            } else {
                ThinkingConfig::Disabled
            }
        }
        ThinkingConfig::Enabled { budget_tokens } => {
            if max_tokens <= MIN_THINKING_BUDGET {
                warnings.push(RequestWarning::BudgetDropped { max_tokens });
                return ThinkingConfig::Disabled;
            }
            let fitted = budget_tokens.clamp(MIN_THINKING_BUDGET, max_tokens - 1);
            if fitted != budget_tokens {
                warnings.push(RequestWarning::BudgetAdjusted {
                    from: budget_tokens,
                    to: fitted,
                });
            }
            ThinkingConfig::Enabled {
                budget_tokens: fitted,
            }
        }
    }
}

/// Keeps at most three explicit breakpoints (the last ones in render
/// order) and turns on automatic caching for the conversation.
fn plan_cache(out: &mut MessagesRequest, warnings: &mut Vec<RequestWarning>) {
    let slots = MAX_CACHE_BREAKPOINTS - 1;
    let mut markers: Vec<&mut Option<CacheControl>> = out
        .tools
        .iter_mut()
        .map(|tool| match tool {
            WireTool::Custom(tool) => &mut tool.cache_control,
            WireTool::Server(tool) => &mut tool.cache_control,
        })
        .chain(out.system.iter_mut().map(|block| &mut block.cache_control))
        .filter(|marker| marker.is_some())
        .collect();
    let excess = markers.len().saturating_sub(slots);
    for marker in markers.iter_mut().take(excess) {
        **marker = None;
    }
    if excess > 0 {
        warnings.push(RequestWarning::CacheBreakpointsDropped { count: excess });
    }
    if !out.messages.is_empty() {
        out.cache_control = Some(CacheControl::EPHEMERAL);
    }
}

#[cfg(test)]
mod tests;
