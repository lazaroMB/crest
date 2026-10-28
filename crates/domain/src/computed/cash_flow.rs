//! Body of the cash-flow card: the period table, every internal rate of
//! return found, and the NPV against the discount rate.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{ComputedError, Curve};

/// One period of a cash flow.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CashFlowRow {
    /// Period number, 0 first (undiscounted).
    pub period: u32,
    /// Net cash flow of the period.
    pub cash_flow: f64,
    /// 1 / (1 + rate)^period.
    pub discount_factor: f64,
    /// Present value of the flow.
    pub present_value: f64,
    /// Running total of the flows.
    pub cumulative: f64,
    /// Running total of the present values.
    pub cumulative_pv: f64,
}

/// Body of a cash-flow card.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CashFlowBody {
    /// Discount rate per period, e.g. 0.1.
    pub rate: f64,
    /// Rate paid on the money invested (MIRR).
    pub finance_rate: f64,
    /// Rate earned on the money returned (MIRR).
    pub reinvest_rate: f64,
    /// Currency of the amounts, shown only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
    /// Name of a period, e.g. `year`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub period_label: Option<String>,
    /// The periods.
    pub rows: Vec<CashFlowRow>,
    /// Every rate at which the NPV is 0, ascending.
    #[serde(default)]
    pub irrs: Vec<f64>,
    /// Sign changes of the flows (Descartes: at most this many IRRs).
    pub sign_changes: u32,
    /// NPV (y) against the discount rate (x).
    pub npv_curve: Curve,
}

impl CashFlowBody {
    /// Checks the NPV curve's shape and the period numbering.
    ///
    /// # Errors
    ///
    /// [`ComputedError::Body`] for a mismatch.
    pub fn validate(&self) -> Result<(), ComputedError> {
        let numbered = self
            .rows
            .iter()
            .enumerate()
            .all(|(i, r)| r.period as usize == i);
        if self.npv_curve.x.len() != self.npv_curve.y.len() || !numbered {
            return Err(ComputedError::Body(
                "cash-flow periods must count from 0 and the NPV curve must match".into(),
            ));
        }
        Ok(())
    }
}
