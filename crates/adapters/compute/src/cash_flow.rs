//! The cash-flow card in decimal arithmetic (`rust_decimal`, 28
//! significant digits): amounts and rates enter as their shortest decimal
//! text, so 0.1 is exactly one tenth and money sums do not drift.
//!
//! - NPV at the discount rate, period 0 undiscounted (numpy-financial's
//!   convention; Excel's `NPV()` discounts its first value too, so it
//!   equals this NPV / (1 + rate)).
//! - Every IRR: the NPV is scanned for sign changes over rates from −99%
//!   to +1000% (a grid even in log(1 + r)) and each is refined by
//!   bisection in decimals. More than one sign change of the flows allows
//!   more than one IRR (Descartes' rule); the card then lists them all and
//!   warns that "the" IRR is not defined.
//! - MIRR (Excel's): positive flows compounded at the reinvestment rate,
//!   negative ones discounted at the finance rate; the final root is taken
//!   in double precision.
//! - Payback and discounted payback, interpolated within the period.

use std::str::FromStr as _;

use application::ports::compute::{CashFlowRequest, ComputeError};
use domain::computed::{
    Assumption, AssumptionStatus, CashFlowBody, CashFlowRow, Computed, ComputedBody, ComputedValue,
    Curve, Narrative, UncertaintyKind, ValueFormat, WarningSeverity, apa,
};
use rust_decimal::prelude::ToPrimitive as _;
use rust_decimal::{Decimal, RoundingStrategy};
use serde_json::Value;

use crate::envelope::{self, count, number, warning};

/// Backend recorded in the envelope.
pub const DECIMAL: &str = "rust_decimal 1.43.0";
/// Most periods.
pub const MAX_PERIODS: usize = 1200;
/// Grid points of the IRR scan.
const SCAN_POINTS: usize = 4000;
/// Points of the NPV curve.
const CURVE_POINTS: usize = 120;

fn invalid(text: impl Into<String>) -> ComputeError {
    ComputeError::Invalid(text.into())
}

/// `x` as the decimal its shortest text names.
fn dec(x: f64, what: &str) -> Result<Decimal, ComputeError> {
    if !x.is_finite() {
        return Err(invalid(format!("{what} must be a finite number")));
    }
    Decimal::from_str(&format!("{x}"))
        .map_err(|_| invalid(format!("{what} {x} is too large for decimal arithmetic")))
}

fn float(d: Decimal) -> f64 {
    d.to_f64().unwrap_or(f64::NAN)
}

/// NPV of `flows` at `rate` in decimals; `None` on overflow or a rate at
/// −100%.
pub fn npv(flows: &[Decimal], rate: Decimal) -> Option<Decimal> {
    let growth = Decimal::ONE.checked_add(rate)?;
    if growth <= Decimal::ZERO {
        return None;
    }
    let mut factor = Decimal::ONE;
    let mut total = Decimal::ZERO;
    for flow in flows {
        total = total.checked_add(flow.checked_div(factor)?)?;
        factor = factor.checked_mul(growth)?;
    }
    Some(total)
}

/// NPV in doubles, for the scan and the curve.
fn npv_f64(flows: &[f64], rate: f64) -> f64 {
    let g = 1.0 + rate;
    let mut factor = 1.0;
    let mut total = 0.0;
    for f in flows {
        total += f / factor;
        factor *= g;
    }
    total
}

/// Bisection of the NPV between `lo` and `hi` (a sign change), in decimals.
fn refine(flows: &[Decimal], lo: f64, hi: f64) -> Option<Decimal> {
    let (mut a, mut b) = (
        Decimal::from_str(&format!("{lo}")).ok()?,
        Decimal::from_str(&format!("{hi}")).ok()?,
    );
    let mut fa = npv(flows, a)?;
    let tiny = Decimal::new(1, 20);
    let two = Decimal::TWO;
    for _ in 0..200 {
        let m = (a + b) / two;
        let fm = npv(flows, m)?;
        if fm.is_zero() || (b - a).abs() < tiny {
            return Some(m);
        }
        if fm.is_sign_negative() == fa.is_sign_negative() {
            a = m;
            fa = fm;
        } else {
            b = m;
        }
    }
    Some((a + b) / two)
}

/// Every IRR in (−99%, 1000%], ascending.
pub fn irrs(flows: &[Decimal]) -> Vec<Decimal> {
    let as_f64: Vec<f64> = flows.iter().map(|d| float(*d)).collect();
    let (lo, hi) = (0.01_f64.ln(), 11.0_f64.ln());
    let rate = |i: usize| (lo + (hi - lo) * i as f64 / SCAN_POINTS as f64).exp() - 1.0;
    let mut roots = Vec::new();
    let mut prev = (rate(0), npv_f64(&as_f64, rate(0)));
    for i in 1..=SCAN_POINTS {
        let r = rate(i);
        let v = npv_f64(&as_f64, r);
        let crossed =
            prev.1 != 0.0 && v.is_finite() && prev.1.is_finite() && (v > 0.0) != (prev.1 > 0.0);
        let root = if v == 0.0 {
            Decimal::from_str(&format!("{r}")).ok()
        } else if crossed {
            refine(flows, prev.0, r)
        } else {
            None
        };
        roots.extend(root);
        prev = (r, v);
    }
    roots
}

/// Excel's MIRR; `None` without both a positive and a negative flow.
pub fn mirr(flows: &[Decimal], finance: Decimal, reinvest: Decimal) -> Option<f64> {
    let n = flows.len();
    let (gf, gr) = (Decimal::ONE + finance, Decimal::ONE + reinvest);
    let mut pv_negative = Decimal::ZERO;
    let mut fv_positive = Decimal::ZERO;
    let mut discount = Decimal::ONE;
    for (t, flow) in flows.iter().enumerate() {
        if flow.is_sign_negative() {
            pv_negative = pv_negative.checked_add(flow.checked_div(discount)?)?;
        } else if !flow.is_zero() {
            let mut grow = Decimal::ONE;
            for _ in 0..n - 1 - t {
                grow = grow.checked_mul(gr)?;
            }
            fv_positive = fv_positive.checked_add(flow.checked_mul(grow)?)?;
        }
        discount = discount.checked_mul(gf)?;
    }
    if pv_negative.is_zero() || fv_positive.is_zero() {
        return None;
    }
    let ratio = float(fv_positive.checked_div(-pv_negative)?);
    Some(ratio.powf(1.0 / (n as f64 - 1.0)) - 1.0)
}

/// The period at which `cumulative` (per period, after its flow) first
/// turns non-negative for good, interpolated within that period.
fn payback(flows: &[f64], cumulative: &[f64]) -> Option<f64> {
    if cumulative.first().is_some_and(|c| *c >= 0.0) {
        return Some(0.0);
    }
    let last_negative = cumulative.iter().rposition(|c| *c < 0.0)?;
    let t = last_negative + 1;
    let flow = *flows.get(t)?;
    (flow > 0.0).then(|| last_negative as f64 + -cumulative[last_negative] / flow)
}

/// Money rounded half away from zero to 2 decimals, as text.
fn money(d: Decimal) -> String {
    let rounded = d.round_dp_with_strategy(2, RoundingStrategy::MidpointAwayFromZero);
    apa::with_minus(&format!("{rounded:.2}"))
}

fn money_value(d: Decimal, currency: Option<&str>) -> ComputedValue {
    ComputedValue {
        display: Some(money(d)),
        unit: currency.map(str::to_owned),
        decimals: Some(2),
        ..ComputedValue::new(float(d))
    }
}

fn rate_value(r: f64) -> ComputedValue {
    ComputedValue {
        display: Some(format!("{}%", apa::number(r * 100.0, 2))),
        decimals: Some(2),
        fmt: Some(ValueFormat::Percent),
        ..ComputedValue::new(r)
    }
}

/// Runs a cash-flow request.
pub fn run(
    request: &CashFlowRequest,
    id: String,
) -> Result<(Computed, ComputedBody), ComputeError> {
    let n = request.flows.len();
    if !(2..=MAX_PERIODS).contains(&n) {
        return Err(invalid(format!("give 2 to {MAX_PERIODS} flows")));
    }
    let flows: Vec<Decimal> = request
        .flows
        .iter()
        .enumerate()
        .map(|(t, x)| dec(*x, &format!("flow {t}")))
        .collect::<Result<_, _>>()?;
    let rate = dec(request.rate, "`rate`")?;
    let finance = dec(
        request.finance_rate.unwrap_or(request.rate),
        "`finance_rate`",
    )?;
    let reinvest = dec(
        request.reinvest_rate.unwrap_or(request.rate),
        "`reinvest_rate`",
    )?;
    if [rate, finance, reinvest]
        .iter()
        .any(|r| *r <= -Decimal::ONE)
    {
        return Err(invalid("rates must be above −100% (−1)"));
    }
    let currency = request.currency.as_deref();
    let mut engine = envelope::solve_engine();
    engine.backend = Some(DECIMAL.into());
    let mut computed = Computed::new(id, "solve.cash_flow", "Cash-flow analysis", engine);
    computed.uncertainty_kind = Some(UncertaintyKind::Exact);
    computed.inputs.push(domain::computed::InputRef {
        name: "flows".into(),
        role: Some("data".into()),
        path: None,
        hash: Some(envelope::hash(
            &serde_json::to_vec(&request.flows).unwrap_or_default(),
        )),
        rows: Some(n as u64),
        columns: Vec::new(),
        filter: None,
    });
    // The period table.
    let growth = Decimal::ONE + rate;
    let mut factor = Decimal::ONE;
    let (mut cum, mut cum_pv) = (Decimal::ZERO, Decimal::ZERO);
    let mut rows = Vec::with_capacity(n);
    for (t, flow) in flows.iter().enumerate() {
        let pv = flow
            .checked_div(factor)
            .ok_or_else(|| invalid("the discounting overflows decimal arithmetic"))?;
        cum += *flow;
        cum_pv += pv;
        rows.push(CashFlowRow {
            period: u32::try_from(t).unwrap_or(u32::MAX),
            cash_flow: float(*flow),
            discount_factor: float(Decimal::ONE.checked_div(factor).unwrap_or_default()),
            present_value: float(pv),
            cumulative: float(cum),
            cumulative_pv: float(cum_pv),
        });
        factor = factor
            .checked_mul(growth)
            .ok_or_else(|| invalid("the discounting overflows decimal arithmetic"))?;
    }
    let npv_at = cum_pv;
    let roots = irrs(&flows);
    let signs: Vec<bool> = flows
        .iter()
        .filter(|f| !f.is_zero())
        .map(|f| f.is_sign_negative())
        .collect();
    let sign_changes = signs.windows(2).filter(|w| w[0] != w[1]).count();
    let modified = mirr(&flows, finance, reinvest);
    let as_f64: Vec<f64> = request.flows.clone();
    let cumulative: Vec<f64> = rows.iter().map(|r| r.cumulative).collect();
    let discounted: Vec<f64> = rows.iter().map(|r| r.cumulative_pv).collect();
    let present: Vec<f64> = rows.iter().map(|r| r.present_value).collect();
    let payback_at = payback(&as_f64, &cumulative);
    let discounted_payback = payback(&present, &discounted);
    let values = &mut computed.values;
    values.insert("npv".into(), money_value(npv_at, currency));
    values.insert("total".into(), money_value(cum, currency));
    values.insert("sign_changes".into(), count(sign_changes as u64));
    match roots.as_slice() {
        [one] => {
            values.insert("irr".into(), rate_value(float(*one)));
        }
        many => {
            for (i, r) in many.iter().enumerate() {
                values.insert(format!("irr_{}", i + 1), rate_value(float(*r)));
            }
        }
    }
    if let Some(m) = modified.filter(|m| m.is_finite()) {
        values.insert("mirr".into(), rate_value(m));
    }
    if let Some(p) = payback_at {
        values.insert("payback".into(), number(p, 2));
    }
    if let Some(p) = discounted_payback {
        values.insert("discounted_payback".into(), number(p, 2));
    }
    // The NPV curve, over a range that shows every IRR.
    let max_irr = roots
        .iter()
        .map(|r| float(*r))
        .fold(f64::NEG_INFINITY, f64::max);
    let min_irr = roots
        .iter()
        .map(|r| float(*r))
        .fold(f64::INFINITY, f64::min);
    let rate_f = float(rate);
    let lo = (min_irr.min(rate_f) - 0.1).max(-0.9).min(rate_f.min(0.0));
    let hi = (max_irr.max(rate_f) * 1.5 + 0.05).max(0.3);
    let x: Vec<f64> = (0..CURVE_POINTS)
        .map(|i| lo + (hi - lo) * i as f64 / (CURVE_POINTS - 1) as f64)
        .collect();
    let y: Vec<f64> = x.iter().map(|&r| npv_f64(&as_f64, r)).collect();
    let (cx, cy): (Vec<f64>, Vec<f64>) =
        x.into_iter().zip(y).filter(|(_, v)| v.is_finite()).unzip();
    let params = &mut computed.method.params;
    params.insert("rate".into(), Value::from(request.rate));
    params.insert(
        "convention".into(),
        Value::from("period 0 undiscounted; flows at the end of each period"),
    );
    params.insert(
        "excel_npv".into(),
        Value::from("Excel's NPV() discounts its first value: NPV / (1 + rate)"),
    );
    computed.method.why =
        Some("decimal arithmetic for money; every IRR by scan and bisection".into());
    if sign_changes > 1 {
        computed.warnings.push(warning(
            "multiple_irr",
            WarningSeverity::Warn,
            format!(
                "the flows change sign {sign_changes} times, so there can be up to \
                 {sign_changes} IRRs ({} found): the IRR is not a single number here; decide on \
                 the NPV at your rate or on the MIRR",
                roots.len()
            ),
        ));
    }
    if roots.is_empty() {
        computed.warnings.push(warning(
            "no_irr",
            WarningSeverity::Info,
            "no rate between −99% and 1000% makes the NPV zero",
        ));
    }
    if payback_at.is_none() {
        computed.warnings.push(warning(
            "no_payback",
            WarningSeverity::Info,
            "the cumulative flow never turns non-negative: no payback",
        ));
    }
    computed.assumptions.push(Assumption {
        id: "timing".into(),
        check: None,
        stat: None,
        p: None,
        status: AssumptionStatus::NotChecked,
        consequence: None,
        note: Some("period 0 is now; later flows fall at the end of their period".into()),
    });
    let flows_text: Vec<String> = request.flows.iter().map(|f| format!("{f}")).collect();
    computed.reproduce.python = Some(format!(
        "import numpy_financial as npf; cf = [{}]; npf.npv({}, cf), npf.irr(cf), npf.mirr(cf, {}, {})",
        flows_text.join(", "),
        request.rate,
        request.finance_rate.unwrap_or(request.rate),
        request.reinvest_rate.unwrap_or(request.rate)
    ));
    computed.reproduce.r = Some(format!(
        "cf <- c({}); npv <- function(r) sum(cf / (1 + r)^(seq_along(cf) - 1)); npv({}); \
         uniroot(npv, c(-0.99, 10))$root",
        flows_text.join(", "),
        request.rate
    ));
    computed.narrative = Some(Narrative {
        template: "cash_flow".into(),
        text_slots: ["npv", "irr", "mirr", "payback"]
            .into_iter()
            .filter(|k| computed.values.contains_key(*k))
            .map(str::to_owned)
            .collect(),
    });
    let body = CashFlowBody {
        rate: request.rate,
        finance_rate: request.finance_rate.unwrap_or(request.rate),
        reinvest_rate: request.reinvest_rate.unwrap_or(request.rate),
        currency: request.currency.clone(),
        period_label: request.period.clone(),
        rows,
        irrs: roots.iter().map(|r| float(*r)).collect(),
        sign_changes: u32::try_from(sign_changes).unwrap_or(u32::MAX),
        npv_curve: Curve { x: cx, y: cy },
    };
    Ok((computed, ComputedBody::CashFlow(body)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimals_keep_tenths_exact() {
        let flows: Vec<Decimal> = [-100.0, 110.0]
            .iter()
            .map(|x| dec(*x, "f").unwrap())
            .collect();
        assert_eq!(npv(&flows, dec(0.1, "r").unwrap()), Some(Decimal::ZERO));
        assert_eq!(money(Decimal::new(-12345, 3)), "−12.35");
        assert_eq!(
            payback(&[-100.0, 60.0, 60.0], &[-100.0, -40.0, 20.0]),
            Some(1.0 + 40.0 / 60.0)
        );
        assert_eq!(payback(&[-100.0, 50.0], &[-100.0, -50.0]), None);
    }
}
