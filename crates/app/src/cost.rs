//! Cost of a session and its optional spending cap.
//!
//! Prices change, so they come from configuration and nothing here
//! hardcodes them. A [`PricingTable`] maps model ids (or prefixes of them)
//! to a [`ModelPricing`]; it is empty by default, and without a price the
//! cost is unknown (`cost_usd: None` in `UsageUpdated`).
//!
//! In `init.lua` both live in `app.opt` (prices in USD per million
//! tokens; the numbers below are only an example, take the current ones
//! from the provider):
//!
//! ```lua
//! app.opt.pricing = {
//!   -- Matches `claude-sonnet-5-5` and any id starting with this prefix;
//!   -- the longest matching key wins.
//!   ["claude-sonnet-5"] = { input = 3, output = 15, cache_read = 0.3, cache_write = 3.75 },
//! }
//! -- Pause and ask before the next model call once the session spent $5.
//! app.opt.max_session_cost = 5
//! ```
//!
//! [`CostTracker::from_options`] reads both from the loaded options. The
//! tracker adds up the cost of every answer of a session (including the
//! summaries of compaction) and, with a cap, the agent loop asks through
//! the `ApprovalPort` before a model call once the session reached it:
//! "approve" raises the cap by its original amount (the next pause comes
//! after spending as much again), "approve always" turns the cap off for
//! the session, and a denial ends the turn.
//!
//! Costs are kept in memory per [`SessionId`]: a session resumed from the
//! store starts counting from zero.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

use domain::conversation::{SessionId, ToolUseId, Usage};
use domain::model::{ModelId, ModelPricing, ProviderId};
use domain::permission::ToolEffect;
use serde_json::Value;

use crate::ports::approval::{ApprovalPreview, ApprovalRequest, ApprovalRequestId};
use crate::ports::plugin::AppOptions;

/// Key of the prices in `app.opt`.
pub const PRICING_OPTION: &str = "pricing";

/// Key of the spending cap in `app.opt`.
pub const CAP_OPTION: &str = "max_session_cost";

/// Name of the tool in the approval request of the spending cap.
pub const SPEND_CAP_TOOL: &str = "spend_cap";

/// Fields of one model's prices in the config, in USD per million tokens.
const PRICE_FIELDS: [&str; 4] = ["input", "output", "cache_read", "cache_write"];

/// A malformed `pricing` or `max_session_cost` option.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CostConfigError {
    /// `pricing` is not a table of models.
    #[error("app.opt.{PRICING_OPTION} must be a table of model prices")]
    NotATable,
    /// A model's prices are wrong.
    #[error("app.opt.{PRICING_OPTION}[\"{model}\"]: {problem}")]
    BadPrice {
        /// Key of the model in the table.
        model: String,
        /// What is wrong with it.
        problem: String,
    },
    /// The cap is not a positive number.
    #[error("app.opt.{CAP_OPTION} must be a positive number of USD")]
    BadCap,
}

/// Price of the models of a flat-rate plan (`opencode-go:…`): nothing
/// per token, unless the config says otherwise.
const PLAN: ModelPricing = ModelPricing {
    input_per_mtok: 0.0,
    output_per_mtok: 0.0,
    cache_read_per_mtok: 0.0,
    cache_write_per_mtok: 0.0,
};

/// Prices per model id or prefix. Empty by default, except that the models
/// of OpenCode Go (a flat-rate subscription) cost nothing per token.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PricingTable {
    prices: BTreeMap<String, ModelPricing>,
}

impl PricingTable {
    /// A table with no prices: every cost is unknown.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the table with `pricing` for ids starting with `prefix`.
    #[must_use]
    pub fn with(mut self, prefix: impl Into<String>, pricing: ModelPricing) -> Self {
        self.prices.insert(prefix.into(), pricing);
        self
    }

    /// Whether no price is configured.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.prices.is_empty()
    }

    /// The price of `model`: its exact id, or else the longest key that is
    /// a prefix of it, or else nothing per token for a flat-rate plan.
    #[must_use]
    pub fn get(&self, model: &ModelId) -> Option<&ModelPricing> {
        let id = model.as_str();
        self.prices
            .get(id)
            .or_else(|| {
                self.prices
                    .iter()
                    .filter(|(prefix, _)| id.starts_with(prefix.as_str()))
                    .max_by_key(|(prefix, _)| prefix.len())
                    .map(|(_, pricing)| pricing)
            })
            .or_else(|| {
                (model.provider().map(|(p, _)| p) == Some(ProviderId::OpencodeGo)).then_some(&PLAN)
            })
    }

    /// Cost in USD of `usage` answered by `model`, when it has a price.
    #[must_use]
    pub fn cost(&self, model: &ModelId, usage: &Usage) -> Option<f64> {
        self.get(model).map(|p| {
            p.cost(
                usage.input_tokens,
                usage.output_tokens,
                usage.cache_read_input_tokens,
                usage.cache_creation_input_tokens,
            )
        })
    }

    /// Reads the `pricing` option: `{ [model] = { input, output,
    /// cache_read, cache_write } }`, every field a non-negative number.
    ///
    /// # Errors
    ///
    /// [`CostConfigError`] naming the first malformed entry.
    pub fn from_json(value: &Value) -> Result<Self, CostConfigError> {
        let map = match value {
            Value::Object(map) => map,
            // Lua has one table type: `{}` may come out as an empty list.
            Value::Array(items) if items.is_empty() => return Ok(Self::new()),
            _ => return Err(CostConfigError::NotATable),
        };
        let mut table = Self::new();
        for (model, prices) in map {
            let bad = |problem: String| CostConfigError::BadPrice {
                model: model.clone(),
                problem,
            };
            let Value::Object(prices) = prices else {
                return Err(bad(format!(
                    "must be a table with {}",
                    PRICE_FIELDS.join(", ")
                )));
            };
            if let Some(unknown) = prices.keys().find(|k| !PRICE_FIELDS.contains(&k.as_str())) {
                return Err(bad(format!("unknown field `{unknown}`")));
            }
            let field = |name: &str| match prices.get(name).and_then(Value::as_f64) {
                Some(n) if n >= 0.0 && n.is_finite() => Ok(n),
                Some(_) => Err(bad(format!("`{name}` must not be negative"))),
                None => Err(bad(format!(
                    "`{name}` must be a number (USD per million tokens)"
                ))),
            };
            let pricing = ModelPricing {
                input_per_mtok: field("input")?,
                output_per_mtok: field("output")?,
                cache_read_per_mtok: field("cache_read")?,
                cache_write_per_mtok: field("cache_write")?,
            };
            table.prices.insert(model.clone(), pricing);
        }
        Ok(table)
    }
}

/// Reads the `max_session_cost` option: a positive number of USD.
///
/// # Errors
///
/// [`CostConfigError::BadCap`] for anything else.
pub fn cap_from_json(value: &Value) -> Result<f64, CostConfigError> {
    match value.as_f64() {
        Some(cap) if cap > 0.0 && cap.is_finite() => Ok(cap),
        _ => Err(CostConfigError::BadCap),
    }
}

/// The session reached its spending cap.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CapReached {
    /// What the session spent, in USD.
    pub spent: f64,
    /// The cap in force, in USD.
    pub cap: f64,
}

impl CapReached {
    /// The question shown to the user.
    #[must_use]
    pub fn question(&self) -> String {
        format!(
            "The session spent ${:.2} of ${:.2}; continue?",
            self.spent, self.cap
        )
    }
}

/// Spending of one session.
#[derive(Debug, Clone, Copy, Default)]
struct Spend {
    /// USD of the answers with a price.
    cost: f64,
    /// Whether at least one answer had a price.
    priced: bool,
    /// Cap in force once raised; `None` uses the configured one.
    cap: Option<f64>,
    /// The user turned the cap off for this session.
    uncapped: bool,
}

/// Request ids of the cap, apart from the tool approvals' counter.
const REQUEST_ID_BASE: u64 = 1 << 63;

/// Adds up the cost of each session and enforces the optional cap.
#[derive(Debug, Default)]
pub struct CostTracker {
    pricing: PricingTable,
    cap: Option<f64>,
    sessions: Mutex<HashMap<SessionId, Spend>>,
    next_request: AtomicU64,
}

impl CostTracker {
    /// A tracker pricing answers with `pricing`, without a cap.
    #[must_use]
    pub fn new(pricing: PricingTable) -> Self {
        Self {
            pricing,
            ..Self::default()
        }
    }

    /// Returns the tracker pausing each session once it spent `cap` USD.
    /// A cap that is not positive is ignored.
    #[must_use]
    pub fn with_cap(mut self, cap: f64) -> Self {
        self.cap = (cap > 0.0 && cap.is_finite()).then_some(cap);
        self
    }

    /// A tracker with the `pricing` and `max_session_cost` of `app.opt`.
    ///
    /// # Errors
    ///
    /// [`CostConfigError`] when either option is malformed.
    pub fn from_options(options: &AppOptions) -> Result<Self, CostConfigError> {
        let pricing = match options.extra.get(PRICING_OPTION) {
            None | Some(Value::Null) => PricingTable::new(),
            Some(value) => PricingTable::from_json(value)?,
        };
        let mut tracker = Self::new(pricing);
        if let Some(value) = options.extra.get(CAP_OPTION).filter(|v| !v.is_null()) {
            tracker.cap = Some(cap_from_json(value)?);
        }
        Ok(tracker)
    }

    /// The prices.
    #[must_use]
    pub fn pricing(&self) -> &PricingTable {
        &self.pricing
    }

    /// The configured cap, in USD.
    #[must_use]
    pub fn cap(&self) -> Option<f64> {
        self.cap
    }

    fn with_spend<T>(&self, session: SessionId, f: impl FnOnce(&mut Spend) -> T) -> T {
        let mut sessions = self.sessions.lock().unwrap_or_else(PoisonError::into_inner);
        f(sessions.entry(session).or_default())
    }

    /// Adds the cost of one answer by `model` and returns the session's
    /// cost, or `None` when `model` has no price (the answer adds
    /// nothing).
    pub fn record(&self, session: SessionId, model: &ModelId, usage: &Usage) -> Option<f64> {
        let cost = self.pricing.cost(model, usage)?;
        Some(self.with_spend(session, |spend| {
            spend.cost += cost;
            spend.priced = true;
            spend.cost
        }))
    }

    /// Cost of `session` so far, when any of its answers had a price.
    #[must_use]
    pub fn session_cost(&self, session: SessionId) -> Option<f64> {
        self.with_spend(session, |spend| spend.priced.then_some(spend.cost))
    }

    /// Whether `session` must pause before its next model call.
    #[must_use]
    pub fn cap_reached(&self, session: SessionId) -> Option<CapReached> {
        let configured = self.cap?;
        self.with_spend(session, |spend| {
            let cap = spend.cap.unwrap_or(configured);
            (!spend.uncapped && spend.cost >= cap).then_some(CapReached {
                spent: spend.cost,
                cap,
            })
        })
    }

    /// The user chose to go on: the next pause comes after spending the
    /// configured cap again.
    pub fn raise_cap(&self, session: SessionId) {
        let Some(step) = self.cap else { return };
        self.with_spend(session, |spend| spend.cap = Some(spend.cost + step));
    }

    /// The user chose to go on without a cap for `session`.
    pub fn lift_cap(&self, session: SessionId) {
        self.with_spend(session, |spend| spend.uncapped = true);
    }

    /// The approval request that asks whether to go on past `reached`.
    pub fn request(&self, reached: CapReached) -> ApprovalRequest {
        let n = self.next_request.fetch_add(1, Ordering::Relaxed);
        ApprovalRequest {
            id: ApprovalRequestId(REQUEST_ID_BASE | n),
            tool_use_id: ToolUseId::new(format!("{SPEND_CAP_TOOL}_{n}")),
            tool: SPEND_CAP_TOOL.to_owned(),
            effect: ToolEffect::Network,
            subject: format!("{:.2} USD", reached.cap),
            preview: ApprovalPreview::Text {
                text: reached.question(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn price(input: f64, output: f64) -> ModelPricing {
        ModelPricing {
            input_per_mtok: input,
            output_per_mtok: output,
            cache_read_per_mtok: input / 10.0,
            cache_write_per_mtok: input * 1.25,
        }
    }

    fn usage(input: u64, output: u64) -> Usage {
        Usage {
            input_tokens: input,
            output_tokens: output,
            ..Usage::default()
        }
    }

    #[test]
    fn exact_id_then_longest_prefix() {
        let table = PricingTable::new()
            .with("claude", price(1.0, 1.0))
            .with("claude-opus", price(2.0, 2.0))
            .with("claude-opus-5-5", price(3.0, 3.0));
        let input = |id: &str| table.get(&ModelId::new(id)).map(|p| p.input_per_mtok);
        assert_eq!(input("claude-opus-5-5"), Some(3.0));
        assert_eq!(input("claude-opus-4"), Some(2.0));
        assert_eq!(input("claude-haiku"), Some(1.0));
        assert_eq!(input("gpt"), None);
        let none = PricingTable::new().cost(&ModelId::new("x"), &usage(1, 1));
        assert_eq!(none, None);
    }

    #[test]
    fn opencode_go_models_cost_nothing_per_token_unless_priced() {
        let go = ModelId::new("opencode-go:glm-5.3");
        assert_eq!(PricingTable::new().cost(&go, &usage(1000, 1000)), Some(0.0));
        let zen = ModelId::new("opencode-zen:glm-5.3");
        assert_eq!(PricingTable::new().cost(&zen, &usage(1, 1)), None);
        let priced = PricingTable::new().with("opencode-go:", price(1.0, 1.0));
        assert_eq!(priced.get(&go).map(|p| p.input_per_mtok), Some(1.0));
    }

    #[test]
    fn reads_prices_from_json() {
        let value = json!({
            "m": { "input": 3, "output": 15, "cache_read": 0.3, "cache_write": 3.75 },
        });
        let table = PricingTable::from_json(&value).unwrap();
        let cost = table.cost(&ModelId::new("m-1"), &usage(1_000_000, 1_000_000));
        assert_eq!(cost, Some(18.0));
        assert!(PricingTable::from_json(&json!([])).unwrap().is_empty());
    }

    #[test]
    fn rejects_malformed_prices() {
        let full = |extra: (&str, Value)| {
            let mut m = json!({ "input": 1, "output": 1, "cache_read": 1, "cache_write": 1 });
            m[extra.0] = extra.1;
            json!({ "m": m })
        };
        let cases = [
            (json!(3), "must be a table of model prices"),
            (json!({ "m": 1 }), "must be a table with"),
            (
                full(("cache_write", json!("x"))),
                "`cache_write` must be a number",
            ),
            (full(("input", json!(-1))), "`input` must not be negative"),
            (full(("x", json!(1))), "unknown field `x`"),
        ];
        for (value, want) in cases {
            let err = PricingTable::from_json(&value).unwrap_err().to_string();
            assert!(err.contains(want), "{value}: {err}");
        }
        assert_eq!(cap_from_json(&json!(2.5)), Ok(2.5));
        assert_eq!(cap_from_json(&json!(0)), Err(CostConfigError::BadCap));
        assert_eq!(cap_from_json(&json!("5")), Err(CostConfigError::BadCap));
    }

    #[test]
    fn from_options_reads_pricing_and_cap() {
        let mut options = AppOptions::default();
        let tracker = CostTracker::from_options(&options).unwrap();
        assert!(tracker.pricing().is_empty());
        assert_eq!(tracker.cap(), None);
        options.extra.insert(
            PRICING_OPTION.into(),
            json!({ "m": { "input": 1, "output": 2, "cache_read": 0, "cache_write": 0 } }),
        );
        options.extra.insert(CAP_OPTION.into(), json!(5));
        let tracker = CostTracker::from_options(&options).unwrap();
        assert!(tracker.pricing().get(&ModelId::new("m")).is_some());
        assert_eq!(tracker.cap(), Some(5.0));
        options.extra.insert(CAP_OPTION.into(), json!(-1));
        assert!(CostTracker::from_options(&options).is_err());
    }

    #[test]
    fn records_per_session_and_ignores_unpriced_models() {
        let tracker = CostTracker::new(PricingTable::new().with("m", price(1.0, 2.0)));
        let (a, b) = (SessionId::new(), SessionId::new());
        assert_eq!(tracker.session_cost(a), None);
        let m = ModelId::new("m");
        assert_eq!(tracker.record(a, &m, &usage(1_000_000, 0)), Some(1.0));
        assert_eq!(tracker.record(a, &m, &usage(0, 1_000_000)), Some(3.0));
        assert_eq!(tracker.record(a, &ModelId::new("x"), &usage(9, 9)), None);
        assert_eq!(tracker.session_cost(a), Some(3.0));
        assert_eq!(tracker.session_cost(b), None);
    }

    #[test]
    fn cap_raises_by_its_amount_or_lifts() {
        let pricing = PricingTable::new().with("m", price(1.0, 0.0));
        let tracker = CostTracker::new(pricing).with_cap(2.0);
        let s = SessionId::new();
        let m = ModelId::new("m");
        tracker.record(s, &m, &usage(1_000_000, 0));
        assert_eq!(tracker.cap_reached(s), None);
        tracker.record(s, &m, &usage(1_500_000, 0));
        let reached = tracker.cap_reached(s).unwrap();
        assert_eq!(
            reached,
            CapReached {
                spent: 2.5,
                cap: 2.0
            }
        );
        assert_eq!(
            reached.question(),
            "The session spent $2.50 of $2.00; continue?"
        );
        tracker.raise_cap(s);
        assert_eq!(tracker.cap_reached(s), None);
        tracker.record(s, &m, &usage(2_000_000, 0));
        assert_eq!(tracker.cap_reached(s).map(|r| r.cap), Some(4.5));
        tracker.lift_cap(s);
        tracker.record(s, &m, &usage(9_000_000, 0));
        assert_eq!(tracker.cap_reached(s), None);
        assert_eq!(CostTracker::default().with_cap(0.0).cap(), None);
    }

    #[test]
    fn request_asks_with_the_question() {
        let tracker = CostTracker::default();
        let reached = CapReached {
            spent: 1.0,
            cap: 1.0,
        };
        let (a, b) = (tracker.request(reached), tracker.request(reached));
        assert_eq!(a.tool, SPEND_CAP_TOOL);
        assert_eq!(a.effect, ToolEffect::Network);
        assert_eq!(
            a.preview,
            ApprovalPreview::Text {
                text: reached.question()
            }
        );
        assert_ne!(a.id, b.id);
        assert_ne!(a.tool_use_id, b.tool_use_id);
    }
}
