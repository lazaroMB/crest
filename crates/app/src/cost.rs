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

use std::collections::BTreeMap;

use domain::conversation::Usage;
use domain::model::{ModelId, ModelPricing};
use serde_json::Value;

/// Key of the prices in `app.opt`.
pub const PRICING_OPTION: &str = "pricing";

/// Key of the spending cap in `app.opt`.
pub const CAP_OPTION: &str = "max_session_cost";

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

/// Prices per model id or prefix. Empty by default.
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
    /// a prefix of it.
    #[must_use]
    pub fn get(&self, model: &ModelId) -> Option<&ModelPricing> {
        let id = model.as_str();
        self.prices.get(id).or_else(|| {
            self.prices
                .iter()
                .filter(|(prefix, _)| id.starts_with(prefix.as_str()))
                .max_by_key(|(prefix, _)| prefix.len())
                .map(|(_, pricing)| pricing)
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
}
