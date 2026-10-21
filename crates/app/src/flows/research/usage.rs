//! Time, model calls, tokens and cost of each research stage.
//!
//! Every model call of a node goes through
//! [`NodeContext::complete_with_citations`](super::node::NodeContext::complete_with_citations)
//! or [`NodeContext::run_agent_turn`](super::node::NodeContext::run_agent_turn),
//! which record what they used in the [`UsageMeter`] of the run. When the
//! node returns, the frame shows a notice such as
//! "Discovery: 46s · 38 calls · 210k in / 9k out · $0.41" and adds it to
//! the flow's per-stage totals. It does not emit `UsageUpdated`: that one
//! belongs to the chat's own cost.

use std::collections::BTreeMap;
use std::ops::AddAssign;
use std::sync::{Arc, Mutex, PoisonError};

use domain::conversation::Usage;
use domain::model::ModelId;
use domain::research::Stage;
use serde::{Deserialize, Serialize};

use crate::cost::PricingTable;

/// What one stage (or a whole flow) used.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StageUsage {
    /// Model calls made.
    pub calls: u32,
    /// Tokens of those calls.
    pub usage: Usage,
    /// Cost in USD of the calls whose model has a price; `None` when none
    /// had one.
    pub cost_usd: Option<f64>,
    /// Wall time of the node runs, in milliseconds.
    pub elapsed_ms: u64,
}

impl AddAssign for StageUsage {
    fn add_assign(&mut self, rhs: Self) {
        self.calls = self.calls.saturating_add(rhs.calls);
        self.usage += rhs.usage;
        self.cost_usd = match (self.cost_usd, rhs.cost_usd) {
            (None, None) => None,
            (a, b) => Some(a.unwrap_or(0.0) + b.unwrap_or(0.0)),
        };
        self.elapsed_ms = self.elapsed_ms.saturating_add(rhs.elapsed_ms);
    }
}

impl StageUsage {
    /// Input tokens of every kind (uncached, cache reads and writes).
    #[must_use]
    pub fn input_tokens(&self) -> u64 {
        self.usage
            .input_tokens
            .saturating_add(self.usage.cache_read_input_tokens)
            .saturating_add(self.usage.cache_creation_input_tokens)
    }

    /// "46s · 38 calls · 210k in / 9k out · $0.41" (the cost only when
    /// known).
    #[must_use]
    pub fn summary(&self) -> String {
        let calls = if self.calls == 1 { "call" } else { "calls" };
        let mut out = format!(
            "{} · {} {calls} · {} in / {} out",
            duration(self.elapsed_ms),
            self.calls,
            tokens(self.input_tokens()),
            tokens(self.usage.output_tokens),
        );
        if let Some(cost) = self.cost_usd {
            out.push_str(&format!(" · ${cost:.2}"));
        }
        out
    }

    /// The total of `by_stage`.
    #[must_use]
    pub fn total<'a>(by_stage: impl IntoIterator<Item = &'a Self>) -> Self {
        let mut total = Self::default();
        for usage in by_stage {
            total += *usage;
        }
        total
    }
}

/// Usage per stage, as the flow keeps it.
pub type UsageByStage = BTreeMap<Stage, StageUsage>;

/// `46s`, `3m 05s`.
fn duration(ms: u64) -> String {
    let secs = ms / 1000;
    if secs < 60 {
        format!("{secs}s")
    } else {
        format!("{}m {:02}s", secs / 60, secs % 60)
    }
}

/// `950`, `9k`, `1.2M`.
fn tokens(n: u64) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..1_000_000 => format!("{}k", n / 1_000),
        // Display only: the precision lost by the cast does not matter.
        #[allow(clippy::cast_precision_loss)]
        _ => format!("{:.1}M", n as f64 / 1_000_000.0),
    }
}

/// Adds up the model calls of one node run. Shared by the parallel parts
/// of a node, hence the lock.
#[derive(Debug, Default)]
pub struct UsageMeter {
    pricing: Option<Arc<PricingTable>>,
    used: Mutex<StageUsage>,
}

impl UsageMeter {
    /// A meter pricing calls with `pricing` (cost unknown without it).
    #[must_use]
    pub fn new(pricing: Option<Arc<PricingTable>>) -> Self {
        Self {
            pricing,
            used: Mutex::default(),
        }
    }

    /// Adds `calls` model calls of `model` that used `usage`.
    pub fn record(&self, model: &ModelId, usage: Usage, calls: u32) {
        let cost_usd = self.pricing.as_ref().and_then(|p| p.cost(model, &usage));
        let mut used = self.used.lock().unwrap_or_else(PoisonError::into_inner);
        *used += StageUsage {
            calls,
            usage,
            cost_usd,
            elapsed_ms: 0,
        };
    }

    /// What was recorded so far (`elapsed_ms` is left to the caller).
    #[must_use]
    pub fn snapshot(&self) -> StageUsage {
        *self.used.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use domain::model::ModelPricing;

    use super::*;

    fn usage(input: u64, output: u64) -> Usage {
        Usage {
            input_tokens: input,
            output_tokens: output,
            ..Usage::default()
        }
    }

    #[test]
    fn the_meter_adds_calls_tokens_and_priced_cost() {
        let pricing = PricingTable::new().with(
            "claude-opus",
            ModelPricing {
                input_per_mtok: 10.0,
                output_per_mtok: 50.0,
                cache_read_per_mtok: 1.0,
                cache_write_per_mtok: 12.5,
            },
        );
        let meter = UsageMeter::new(Some(Arc::new(pricing)));
        meter.record(&ModelId::new("other"), usage(5, 5), 1);
        assert_eq!(meter.snapshot().cost_usd, None);
        meter.record(&ModelId::new("claude-opus-5"), usage(200_000, 8_000), 3);
        let used = meter.snapshot();
        assert_eq!(used.calls, 4);
        assert_eq!(used.usage, usage(200_005, 8_005));
        assert!((used.cost_usd.unwrap() - 2.4).abs() < 1e-9);
    }

    #[test]
    fn summaries_read_like_the_notice() {
        let used = StageUsage {
            calls: 38,
            usage: Usage {
                input_tokens: 10_000,
                cache_read_input_tokens: 200_000,
                output_tokens: 9_400,
                ..Usage::default()
            },
            cost_usd: Some(0.414),
            elapsed_ms: 46_200,
        };
        assert_eq!(used.summary(), "46s · 38 calls · 210k in / 9k out · $0.41");
        let unpriced = StageUsage {
            calls: 1,
            elapsed_ms: 185_000,
            ..StageUsage::default()
        };
        assert_eq!(unpriced.summary(), "3m 05s · 1 call · 0 in / 0 out");
        let total = StageUsage::total([&used, &unpriced]);
        assert_eq!(total.calls, 39);
        assert_eq!(total.cost_usd, Some(0.414));
        assert_eq!(tokens(1_250_000), "1.2M");
    }
}
