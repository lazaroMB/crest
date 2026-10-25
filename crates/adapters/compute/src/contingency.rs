//! The contingency-table card: observed and expected counts, row and
//! column percentages, adjusted standardised residuals (R's
//! `chisq.test(...)$stdres`) and a test of independence.
//!
//! `auto` runs Fisher's exact test when an expected count is below 5 and
//! Pearson's χ² otherwise, and says so in `method.why`. Fisher's test on
//! an r×c table sums the probabilities of every table with the observed
//! margins that is no more probable than the observed one (R's rule,
//! relative tolerance 1e-7). The tables are enumerated column by column
//! and met in the middle: the first half of the columns is enumerated
//! once and grouped by the row totals it leaves, the second half once per
//! such remainder, and the two halves are combined through sorted
//! partial sums, so a 4×4 table of 96 counts takes milliseconds. Beyond
//! [`ENUMERATION_BUDGET`] steps the p-value is simulated instead
//! (seeded, as R's `simulate.p.value`), with its Monte Carlo SE.
//!
//! On a 2×2 table the odds ratio is R's conditional maximum-likelihood
//! estimate with its exact interval when Fisher's test runs, and the
//! sample odds ratio with Woolf's interval under χ².

use std::collections::HashMap;

use application::ports::compute::{ComputeError, ContingencyChoice, ContingencyRequest};
use domain::computed::{
    Alternative, Assumption, AssumptionStatus, Computed, ComputedBody, ContingencyBody,
    ContingencyTest, Narrative, UncertaintyKind, WarningSeverity, apa,
};
use serde_json::Value;
use statrs::function::factorial::ln_binomial;

use crate::data;
use crate::envelope::{
    self, bounded, count, df, labelled, number, p_value, significant, warning, with_ci,
};
use crate::math::pchisq_upper;
use crate::reproduce::{py_col, py_frame, r_col, r_frame};
use crate::rng::{RNG_NAME, Rng};
use crate::tests::{expected, fisher_p, odds_ratio, r_label};

/// Most enumeration steps of an exact r×c Fisher test before it falls
/// back to simulation.
pub const ENUMERATION_BUDGET: u64 = 40_000_000;
/// Most simulated tables.
pub const MAX_SIMULATIONS: u32 = 1_000_000;
/// Relative tolerance of "no more probable than the observed table".
const REL_TOL: f64 = 1e-7;

fn invalid(text: impl Into<String>) -> ComputeError {
    ComputeError::Invalid(text.into())
}

/// `ln k!` for `k` in `0..=n`.
fn ln_factorials(n: u64) -> Vec<f64> {
    let mut out = Vec::with_capacity(usize::try_from(n).unwrap_or(0) + 1);
    let mut acc = 0.0;
    out.push(0.0);
    for k in 1..=n {
        #[allow(clippy::cast_precision_loss)]
        let k = k as f64;
        acc += k.ln();
        out.push(acc);
    }
    out
}

/// Whole counts of `table`, or `None` when a cell is not a whole number.
fn whole(table: &[Vec<f64>]) -> Option<Vec<Vec<u64>>> {
    table
        .iter()
        .map(|row| {
            row.iter()
                .map(|&x| {
                    // Checked whole and non-negative before the cast.
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    (x >= 0.0 && x.fract() == 0.0 && x < 1e12).then_some(x as u64)
                })
                .collect()
        })
        .collect()
}

/// Enumerates the fillings of a sequence of columns.
struct Enumerator<'a> {
    ln_fact: &'a [f64],
    steps: u64,
    budget: u64,
}

impl Enumerator<'_> {
    /// Every way to fill `columns` (their sums) given the row capacities
    /// `rem`, calling `visit(rem_after, Σ −ln x!)`. `false` when the
    /// budget ran out.
    fn columns(
        &mut self,
        columns: &[u64],
        rem: &mut Vec<u64>,
        acc: f64,
        visit: &mut dyn FnMut(&[u64], f64),
    ) -> bool {
        let Some((&first, rest)) = columns.split_first() else {
            visit(rem, acc);
            return true;
        };
        let capacity: u64 = rem.iter().sum();
        self.cells(0, first, capacity, rem, acc, rest, visit)
    }

    /// Fills rows `i..` of the current column with `left` counts.
    #[allow(clippy::too_many_arguments)]
    fn cells(
        &mut self,
        i: usize,
        left: u64,
        capacity: u64,
        rem: &mut Vec<u64>,
        acc: f64,
        rest: &[u64],
        visit: &mut dyn FnMut(&[u64], f64),
    ) -> bool {
        self.steps += 1;
        if self.steps > self.budget {
            return false;
        }
        let ln = |k: u64| self.ln_fact[usize::try_from(k).unwrap_or(0)];
        if i + 1 == rem.len() {
            if left > rem[i] {
                return true;
            }
            let acc = acc - ln(left);
            rem[i] -= left;
            let ok = self.columns(rest, rem, acc, visit);
            rem[i] += left;
            return ok;
        }
        let after = capacity - rem[i];
        let lo = left.saturating_sub(after);
        let hi = left.min(rem[i]);
        for x in lo..=hi {
            let acc = acc - ln(x);
            rem[i] -= x;
            let ok = self.cells(i + 1, left - x, after, rem, acc, rest, visit);
            rem[i] += x;
            if !ok {
                return false;
            }
        }
        true
    }
}

/// Fisher's exact p-value of an r×c table of whole counts (two-sided, R's
/// rule), or `None` when the enumeration exceeds [`ENUMERATION_BUDGET`].
pub fn fisher_exact_rc(table: &[Vec<u64>]) -> Option<f64> {
    // Enumerate over the shorter side: transpose so rows ≤ columns.
    let (rows, cols) = (table.len(), table.first().map_or(0, Vec::len));
    let mut t: Vec<Vec<u64>> = if rows > cols {
        (0..cols)
            .map(|j| table.iter().map(|r| r[j]).collect())
            .collect()
    } else {
        table.to_vec()
    };
    // Columns by descending total: the largest one is the determined last.
    let ncols = t[0].len();
    let mut order: Vec<usize> = (0..ncols).collect();
    order.sort_by_key(|&j| std::cmp::Reverse(t.iter().map(|r| r[j]).sum::<u64>()));
    order.reverse();
    t = t
        .iter()
        .map(|r| order.iter().map(|&j| r[j]).collect())
        .collect();
    let row_sums: Vec<u64> = t.iter().map(|r| r.iter().sum()).collect();
    let col_sums: Vec<u64> = (0..ncols).map(|j| t.iter().map(|r| r[j]).sum()).collect();
    let n: u64 = row_sums.iter().sum();
    let ln_fact = ln_factorials(n);
    let ln = |k: u64| ln_fact[usize::try_from(k).unwrap_or(0)];
    let k_const: f64 = row_sums.iter().map(|&r| ln(r)).sum::<f64>()
        + col_sums.iter().map(|&c| ln(c)).sum::<f64>()
        - ln(n);
    let observed: f64 = -t.iter().flatten().map(|&x| ln(x)).sum::<f64>();
    let threshold = observed + REL_TOL.ln_1p();

    let enumerated = &col_sums[..ncols - 1];
    let split = enumerated.len().div_ceil(2);
    let (prefix_cols, suffix_cols) = enumerated.split_at(split);
    let mut walker = Enumerator {
        ln_fact: &ln_fact,
        steps: 0,
        budget: ENUMERATION_BUDGET,
    };
    let mut states: HashMap<Vec<u64>, Vec<f64>> = HashMap::new();
    let mut rem = row_sums.clone();
    let ok = walker.columns(prefix_cols, &mut rem, 0.0, &mut |state, a| {
        states.entry(state.to_vec()).or_default().push(a);
    });
    if !ok {
        return None;
    }
    let mut p = 0.0;
    for (state, prefixes) in &states {
        let mut tails: Vec<f64> = Vec::new();
        let mut rem = state.clone();
        let ok = walker.columns(suffix_cols, &mut rem, 0.0, &mut |last, b| {
            // The last column takes what is left of every row.
            tails.push(b - last.iter().map(|&x| ln(x)).sum::<f64>());
        });
        if !ok {
            return None;
        }
        tails.sort_by(f64::total_cmp);
        let top = tails.last().copied().unwrap_or(0.0);
        let mut partial = Vec::with_capacity(tails.len() + 1);
        partial.push(0.0);
        for b in &tails {
            let next = partial.last().copied().unwrap_or(0.0) + (b - top).exp();
            partial.push(next);
        }
        for &a in prefixes {
            let k = tails.partition_point(|b| a + b <= threshold);
            if k > 0 {
                p += (k_const + a + top).exp() * partial[k];
            }
        }
    }
    Some(p.min(1.0))
}

/// A Monte Carlo Fisher p-value from `b` random tables with the observed
/// margins: `(1 + #{P(T) ≤ P(observed)}) / (b + 1)`, as R.
pub fn fisher_simulated(table: &[Vec<u64>], b: u32, rng: &mut Rng) -> f64 {
    let row_sums: Vec<u64> = table.iter().map(|r| r.iter().sum()).collect();
    let cols = table[0].len();
    let col_sums: Vec<u64> = (0..cols)
        .map(|j| table.iter().map(|r| r[j]).sum())
        .collect();
    let n: u64 = row_sums.iter().sum();
    let ln_fact = ln_factorials(n);
    let ln = |k: u64| ln_fact[usize::try_from(k).unwrap_or(0)];
    let observed: f64 = -table.iter().flatten().map(|&x| ln(x)).sum::<f64>();
    let threshold = observed + REL_TOL.ln_1p();
    // One label per unit: its column; shuffled and dealt out by rows.
    let mut units: Vec<usize> = col_sums
        .iter()
        .enumerate()
        .flat_map(|(j, &c)| std::iter::repeat_n(j, usize::try_from(c).unwrap_or(0)))
        .collect();
    let mut hits = 0u32;
    let mut cells = vec![0u64; table.len() * cols];
    for _ in 0..b {
        rng.shuffle(&mut units);
        cells.fill(0);
        let mut start = 0;
        for (i, &r) in row_sums.iter().enumerate() {
            let end = start + usize::try_from(r).unwrap_or(0);
            for &j in &units[start..end] {
                cells[i * cols + j] += 1;
            }
            start = end;
        }
        let s: f64 = -cells.iter().map(|&x| ln(x)).sum::<f64>();
        if s <= threshold {
            hits += 1;
        }
    }
    f64::from(hits + 1) / f64::from(b + 1)
}

/// The conditional distribution of the (1, 1) cell of a 2×2 table given
/// its margins: support and log probabilities under independence.
struct Conditional {
    support: Vec<f64>,
    log_density: Vec<f64>,
}

impl Conditional {
    fn new(table: &[Vec<u64>]) -> Self {
        let (a, b, c, d) = (table[0][0], table[0][1], table[1][0], table[1][1]);
        let (m, n, k) = (a + c, b + d, a + b);
        let lo = k.saturating_sub(n);
        let hi = k.min(m);
        let support: Vec<u64> = (lo..=hi).collect();
        let log_density = support
            .iter()
            .map(|&x| ln_binomial(m, x) + ln_binomial(n, k - x) - ln_binomial(m + n, k))
            .collect();
        #[allow(clippy::cast_precision_loss)]
        let support = support.into_iter().map(|x| x as f64).collect();
        Self {
            support,
            log_density,
        }
    }

    fn lo(&self) -> f64 {
        self.support[0]
    }

    fn hi(&self) -> f64 {
        self.support[self.support.len() - 1]
    }

    /// The noncentral hypergeometric probabilities at odds ratio `psi`.
    fn density(&self, psi: f64) -> Vec<f64> {
        let ln_psi = psi.ln();
        let d: Vec<f64> = self
            .log_density
            .iter()
            .zip(&self.support)
            .map(|(l, x)| l + ln_psi * x)
            .collect();
        let top = d.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let e: Vec<f64> = d.iter().map(|x| (x - top).exp()).collect();
        let total: f64 = e.iter().sum();
        e.into_iter().map(|x| x / total).collect()
    }

    fn mean(&self, psi: f64) -> f64 {
        if psi == 0.0 {
            return self.lo();
        }
        if psi.is_infinite() {
            return self.hi();
        }
        self.density(psi)
            .iter()
            .zip(&self.support)
            .map(|(p, x)| p * x)
            .sum()
    }

    /// P(X ≤ q) or, with `upper`, P(X ≥ q) at odds ratio `psi`.
    fn tail(&self, q: f64, psi: f64, upper: bool) -> f64 {
        self.density(psi)
            .iter()
            .zip(&self.support)
            .filter(|(_, x)| if upper { **x >= q } else { **x <= q })
            .map(|(p, _)| p)
            .sum()
    }
}

/// The root of `f` in `[lo, hi]` by bisection; `f(lo)` and `f(hi)` must
/// differ in sign.
fn bisect(f: impl Fn(f64) -> f64, mut lo: f64, mut hi: f64) -> Option<f64> {
    let (mut flo, fhi) = (f(lo), f(hi));
    if !(flo.is_finite() && fhi.is_finite()) || (flo > 0.0) == (fhi > 0.0) {
        return None;
    }
    for _ in 0..200 {
        let mid = 0.5 * (lo + hi);
        let fmid = f(mid);
        if (fmid > 0.0) == (flo > 0.0) {
            lo = mid;
            flo = fmid;
        } else {
            hi = mid;
        }
        if hi - lo <= 1e-14 * mid.abs().max(1e-300) {
            break;
        }
    }
    Some(0.5 * (lo + hi))
}

/// Solves `g(ψ) = target` for an increasing `g` over (0, ∞) as R's
/// `fisher.test` does: on (0, 1) when `g(1) > target`, else for 1/ψ on
/// (ε, 1).
fn solve_odds(g: &dyn Fn(f64) -> f64, target: f64, at_one: f64, increasing: bool) -> Option<f64> {
    let below = if increasing {
        at_one > target
    } else {
        at_one < target
    };
    if (at_one - target).abs() < f64::EPSILON {
        Some(1.0)
    } else if below {
        bisect(|t| g(t) - target, 1e-300, 1.0)
    } else {
        bisect(|t| g(1.0 / t) - target, f64::EPSILON, 1.0).map(|t| 1.0 / t)
    }
}

/// R's conditional MLE of the odds ratio of a 2×2 table and its exact
/// interval at `level` for `alternative` (bounds may be 0 or ∞).
pub fn conditional_odds_ratio(
    table: &[Vec<u64>],
    level: f64,
    alternative: Alternative,
) -> (f64, f64, f64) {
    let dist = Conditional::new(table);
    #[allow(clippy::cast_precision_loss)]
    let x = table[0][0] as f64;
    let estimate = if x == dist.lo() {
        0.0
    } else if x == dist.hi() {
        f64::INFINITY
    } else {
        solve_odds(&|t| dist.mean(t), x, dist.mean(1.0), true).unwrap_or(f64::NAN)
    };
    let upper = |alpha: f64| {
        if x == dist.hi() {
            return f64::INFINITY;
        }
        // P(X ≤ x; ψ) falls as ψ grows.
        solve_odds(
            &|t| dist.tail(x, t, false),
            alpha,
            dist.tail(x, 1.0, false),
            false,
        )
        .unwrap_or(f64::NAN)
    };
    let lower = |alpha: f64| {
        if x == dist.lo() {
            return 0.0;
        }
        // P(X ≥ x; ψ) rises with ψ.
        solve_odds(
            &|t| dist.tail(x, t, true),
            alpha,
            dist.tail(x, 1.0, true),
            true,
        )
        .unwrap_or(f64::NAN)
    };
    let (lo, hi) = match alternative {
        Alternative::TwoSided => {
            let alpha = (1.0 - level) / 2.0;
            (lower(alpha), upper(alpha))
        }
        Alternative::Less => (0.0, upper(1.0 - level)),
        Alternative::Greater => (lower(1.0 - level), f64::INFINITY),
    };
    (estimate, lo, hi)
}

/// The table and its labels from the request.
type Table = (Vec<Vec<f64>>, Vec<String>, Vec<String>);

fn table_of(request: &ContingencyRequest, dropped: &mut u64) -> Result<Table, ComputeError> {
    if let Some(table) = &request.table {
        let rows = table.len();
        let cols = table.first().map_or(0, Vec::len);
        if rows < 2 || cols < 2 || table.iter().any(|r| r.len() != cols) {
            return Err(invalid(
                "`table` needs at least 2 rows and 2 columns of equal length",
            ));
        }
        let labels =
            |given: &[String], n: usize, prefix: &str| -> Result<Vec<String>, ComputeError> {
                match given.len() {
                    0 => Ok((1..=n).map(|i| format!("{prefix}{i}")).collect()),
                    k if k == n => Ok(given.to_vec()),
                    k => Err(invalid(format!("{k} labels for {n} {prefix}s"))),
                }
            };
        return Ok((
            table.clone(),
            labels(&request.row_labels, rows, "R")?,
            labels(&request.column_labels, cols, "C")?,
        ));
    }
    let (Some(row), Some(column)) = (&request.row, &request.column) else {
        return Err(invalid(
            "give two categorical columns `row` and `column`, or the counts in `table`",
        ));
    };
    let (rows, row_levels) = data::labels(&request.data, row)?;
    let (cols, col_levels) = data::labels(&request.data, column)?;
    let mut counts = vec![vec![0.0; col_levels.len()]; row_levels.len()];
    for (r, c) in rows.iter().zip(&cols) {
        match (r, c) {
            (Some(r), Some(c)) => {
                let i = row_levels.iter().position(|l| l == r);
                let j = col_levels.iter().position(|l| l == c);
                if let (Some(i), Some(j)) = (i, j) {
                    counts[i][j] += 1.0;
                }
            }
            _ => *dropped += 1,
        }
    }
    if row_levels.len() < 2 || col_levels.len() < 2 {
        return Err(invalid(format!(
            "`{row}` and `{column}` need at least two levels each"
        )));
    }
    Ok((counts, row_levels, col_levels))
}

/// Runs a contingency request.
pub fn run(
    request: &ContingencyRequest,
    id: String,
) -> Result<(Computed, ComputedBody), ComputeError> {
    if !(request.conf_level > 0.0 && request.conf_level < 1.0) {
        return Err(invalid("`conf_level` must be between 0 and 1"));
    }
    if !(1..=MAX_SIMULATIONS).contains(&request.simulations) {
        return Err(invalid(format!(
            "`simulations` must be between 1 and {MAX_SIMULATIONS}"
        )));
    }
    let mut dropped = 0;
    let (observed, row_levels, column_levels) = table_of(request, &mut dropped)?;
    let mut computed = Computed::new(
        id,
        "stats.contingency",
        ContingencyTest::ChiSquare.label(),
        envelope::engine(),
    );
    computed.uncertainty_kind = Some(UncertaintyKind::FrequentistCi);
    let names: Vec<&str> = [&request.row, &request.column]
        .into_iter()
        .flatten()
        .map(String::as_str)
        .collect();
    let used = data::used_columns(&request.data, &names);
    let extra = request
        .table
        .as_ref()
        .map(|t| serde_json::to_value(t).unwrap_or_default());
    computed
        .inputs
        .push(envelope::input(&request.data, &used, extra.as_ref()));
    if dropped > 0 {
        computed.warnings.push(warning(
            "missing_dropped",
            WarningSeverity::Info,
            format!("{dropped} rows with missing values were left out"),
        ));
    }
    let (r, c) = (row_levels.len(), column_levels.len());
    let empty = |m: f64| vec![vec![m; c]; r];
    let mut body = ContingencyBody {
        row_variable: request.row.clone(),
        column_variable: request.column.clone(),
        row_levels,
        column_levels,
        observed: observed.clone(),
        expected: empty(0.0),
        row_percent: empty(0.0),
        column_percent: empty(0.0),
        std_residuals: empty(0.0),
        test: ContingencyTest::ChiSquare,
    };
    let Some((e, n)) = expected(&observed) else {
        envelope::fail(
            &mut computed,
            "the table needs non-negative counts with no empty row or column",
        );
        return Ok((computed, ComputedBody::Contingency(body)));
    };
    let row_sums: Vec<f64> = observed.iter().map(|row| row.iter().sum()).collect();
    let col_sums: Vec<f64> = (0..c)
        .map(|j| observed.iter().map(|row| row[j]).sum())
        .collect();
    for i in 0..r {
        for j in 0..c {
            let o = observed[i][j];
            body.row_percent[i][j] = 100.0 * o / row_sums[i];
            body.column_percent[i][j] = 100.0 * o / col_sums[j];
            let v = e[i][j] * (1.0 - row_sums[i] / n) * (1.0 - col_sums[j] / n);
            body.std_residuals[i][j] = (o - e[i][j]) / v.sqrt();
        }
    }
    body.expected = e.clone();
    let min_expected = e.iter().flatten().copied().fold(f64::INFINITY, f64::min);
    let below = e.iter().flatten().filter(|x| **x < 5.0).count();
    let two_by_two = r == 2 && c == 2;
    let counts = whole(&observed);
    let test = match request.test {
        ContingencyChoice::ChiSquare => ContingencyTest::ChiSquare,
        ContingencyChoice::Fisher => ContingencyTest::FisherExact,
        ContingencyChoice::Auto if min_expected < 5.0 && counts.is_some() => {
            ContingencyTest::FisherExact
        }
        ContingencyChoice::Auto => ContingencyTest::ChiSquare,
    };
    if test != ContingencyTest::ChiSquare && counts.is_none() {
        return Err(invalid("Fisher's test needs whole-number counts"));
    }
    computed.method.why = Some(match request.test {
        ContingencyChoice::Auto if test == ContingencyTest::ChiSquare => {
            "every expected count is at least 5".to_owned()
        }
        ContingencyChoice::Auto => format!(
            "an expected count is {} < 5, so the χ² approximation is poor",
            apa::number(min_expected, 2)
        ),
        _ => "chosen in the request".to_owned(),
    });
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let total = n.round() as u64;
    computed.values.insert("n".into(), count(total));
    computed
        .values
        .insert("min_expected".into(), number(min_expected, 2));
    computed
        .values
        .insert("cells_expected_lt_5".into(), count(below as u64));
    let cells = || observed.iter().flatten().zip(e.iter().flatten());
    let raw_chi2: f64 = cells().map(|(o, e)| (o - e).powi(2) / e).sum();
    let k = r.min(c) as f64 - 1.0;
    let v = (raw_chi2 / (n * k)).sqrt();
    let effect_v = labelled(
        bounded(v, 2),
        r_label(v * k.sqrt()),
        "Cohen 1988, w scaled by min(r, c) − 1",
    );
    computed.values.insert("cramers_v".into(), effect_v.clone());
    let (max_i, max_j, max_res) = (0..r)
        .flat_map(|i| (0..c).map(move |j| (i, j)))
        .map(|(i, j)| (i, j, body.std_residuals[i][j]))
        .fold((0, 0, 0.0_f64), |best, cell| {
            if cell.2.abs() > best.2.abs() {
                cell
            } else {
                best
            }
        });
    computed
        .values
        .insert("max_residual".into(), number(max_res, 2));
    computed.method.params.insert(
        "max_residual_cell".into(),
        Value::from(format!(
            "{} × {}",
            body.row_levels[max_i], body.column_levels[max_j]
        )),
    );
    computed.assumptions.push(Assumption {
        id: "expected_counts".into(),
        check: Some("expected count ≥ 5".into()),
        stat: Some(min_expected),
        p: None,
        status: if min_expected >= 5.0 {
            AssumptionStatus::Pass
        } else if test == ContingencyTest::ChiSquare {
            AssumptionStatus::Fail
        } else {
            AssumptionStatus::Warn
        },
        consequence: (min_expected < 5.0 && test != ContingencyTest::ChiSquare)
            .then(|| "used Fisher's test, which needs no large counts".into()),
        note: None,
    });
    computed.assumptions.push(Assumption {
        id: "independence".into(),
        check: None,
        stat: None,
        p: None,
        status: AssumptionStatus::NotChecked,
        consequence: None,
        note: Some("each unit counted once, in one cell; a design property, confirm".into()),
    });
    let mut test = test;
    match (test, counts) {
        (ContingencyTest::ChiSquare, _) => {
            let yates = request.correct && two_by_two;
            let chi2 = if yates {
                cells()
                    .map(|(o, e)| {
                        let dev = (o - e).abs();
                        (dev - dev.min(0.5)).powi(2) / e
                    })
                    .sum()
            } else {
                raw_chi2
            };
            let dof = ((r - 1) * (c - 1)) as f64;
            computed.values.insert("statistic".into(), number(chi2, 2));
            computed.values.insert("df".into(), df(dof));
            computed
                .values
                .insert("p_value".into(), p_value(pchisq_upper(chi2, dof)));
            computed.values.insert("effect".into(), effect_v);
            if yates {
                computed.method.name = "Pearson's χ² test with Yates' continuity correction".into();
            }
            if two_by_two {
                let (or, lo, hi, method) = odds_ratio(
                    observed[0][0],
                    observed[0][1],
                    observed[1][0],
                    observed[1][1],
                    request.conf_level,
                );
                computed.values.insert(
                    "odds_ratio".into(),
                    with_ci(number(or, 2), request.conf_level, lo, hi, method),
                );
            }
            if min_expected < 5.0 {
                computed.warnings.push(warning(
                    "expected_count_lt_5",
                    WarningSeverity::Warn,
                    format!(
                        "an expected count is {} < 5: the χ² approximation is poor; use Fisher's test",
                        apa::number(min_expected, 2)
                    ),
                ));
            }
            computed.method.alternatives = vec!["fisher".into()];
        }
        (_, Some(whole)) => {
            computed.method.alternatives = vec!["chi_square".into()];
            let p = if two_by_two {
                fisher_p(&observed, request.alternative)
            } else {
                fisher_exact_rc(&whole)
            };
            let p = match p {
                Some(p) => p,
                None => {
                    test = ContingencyTest::FisherSimulated;
                    let mut rng = Rng::new(request.seed);
                    computed.seed = Some(request.seed);
                    computed.rng = Some(RNG_NAME.into());
                    let p = fisher_simulated(&whole, request.simulations, &mut rng);
                    let b = f64::from(request.simulations);
                    computed
                        .values
                        .insert("p_mcse".into(), significant((p * (1.0 - p) / b).sqrt(), 2));
                    computed
                        .values
                        .insert("simulations".into(), count(u64::from(request.simulations)));
                    computed.uncertainty_kind = Some(UncertaintyKind::MonteCarlo);
                    computed.warnings.push(warning(
                        "simulated_p_value",
                        WarningSeverity::Info,
                        format!(
                            "too many tables to enumerate: the p-value is simulated from {} \
                             random tables with the observed margins (seed {})",
                            request.simulations, request.seed
                        ),
                    ));
                    p
                }
            };
            computed.values.insert("p_value".into(), p_value(p));
            if two_by_two {
                let (or, lo, hi) =
                    conditional_odds_ratio(&whole, request.conf_level, request.alternative);
                let value = with_ci(
                    number(or, 2),
                    request.conf_level,
                    lo,
                    hi,
                    "conditional_mle_exact",
                );
                if !hi.is_finite() || lo == 0.0 {
                    computed.method.params.insert(
                        "odds_ratio_interval".into(),
                        Value::from(format!(
                            "[{}, {}]",
                            apa::number(lo, 4),
                            if hi.is_finite() {
                                apa::number(hi, 4)
                            } else {
                                "∞".into()
                            }
                        )),
                    );
                }
                if or.is_finite() {
                    computed.values.insert("odds_ratio".into(), value.clone());
                    computed.values.insert("effect".into(), value);
                }
                computed.method.params.insert(
                    "alternative".into(),
                    Value::from(match request.alternative {
                        Alternative::TwoSided => "two-sided",
                        Alternative::Less => "less",
                        Alternative::Greater => "greater",
                    }),
                );
            } else {
                computed.values.insert("effect".into(), effect_v);
            }
            computed.method.name = test.label().into();
        }
        (_, None) => unreachable_counts(&mut computed),
    }
    body.test = test;
    computed.kind = match test {
        ContingencyTest::ChiSquare => "stats.contingency_chisq",
        ContingencyTest::FisherExact => "stats.contingency_fisher",
        ContingencyTest::FisherSimulated => "stats.contingency_fisher_simulated",
    }
    .into();
    reproduce(&mut computed, request, &observed, test, &used);
    computed.narrative = Some(Narrative {
        template: "contingency".into(),
        text_slots: ["statistic", "df", "n", "p_value", "effect", "max_residual"]
            .into_iter()
            .filter(|k| computed.values.contains_key(*k))
            .map(str::to_owned)
            .collect(),
    });
    Ok((computed, ComputedBody::Contingency(body)))
}

/// Fisher was chosen on counts already checked whole; kept as a failure
/// rather than a panic.
fn unreachable_counts(computed: &mut Computed) {
    envelope::fail(computed, "Fisher's test needs whole-number counts");
}

fn reproduce(
    computed: &mut Computed,
    request: &ContingencyRequest,
    observed: &[Vec<f64>],
    test: ContingencyTest,
    used: &[application::ports::compute::DataColumn],
) {
    let cells: Vec<String> = observed.iter().flatten().map(|x| format!("{x}")).collect();
    let rows: Vec<String> = observed
        .iter()
        .map(|r| {
            let cells: Vec<String> = r.iter().map(|x| format!("{x}")).collect();
            format!("[{}]", cells.join(", "))
        })
        .collect();
    let (r_table, py_table) = match (&request.row, &request.column, &request.table) {
        (Some(row), Some(col), None) => (
            format!(
                "{}; t <- table({}, {})",
                r_frame(&request.data, used),
                r_col(row),
                r_col(col)
            ),
            format!(
                "{}; t = pd.crosstab({}, {})",
                py_frame(&request.data, used),
                py_col(row),
                py_col(col)
            ),
        ),
        _ => (
            format!(
                "t <- matrix(c({}), nrow = {}, byrow = TRUE)",
                cells.join(", "),
                observed.len()
            ),
            format!("t = [{}]", rows.join(", ")),
        ),
    };
    let two_by_two = observed.len() == 2 && observed[0].len() == 2;
    let alt = match request.alternative {
        Alternative::TwoSided => "two",
        Alternative::Less => "less",
        Alternative::Greater => "greater",
    };
    match test {
        ContingencyTest::ChiSquare => {
            let correct = if request.correct { "TRUE" } else { "FALSE" };
            computed.reproduce.r = Some(format!(
                "{r_table}; x <- chisq.test(t, correct = {correct}); x; x$stdres"
            ));
            computed.reproduce.python = Some(format!(
                "import scipy.stats as st; {py_table}; st.chi2_contingency(t, correction={})",
                if request.correct { "True" } else { "False" }
            ));
        }
        ContingencyTest::FisherExact if two_by_two => {
            let alt_r = if alt == "two" { "two.sided" } else { alt };
            let alt_py = if alt == "two" { "two-sided" } else { alt };
            computed.reproduce.r = Some(format!(
                "{r_table}; fisher.test(t, alternative = \"{alt_r}\", conf.level = {})",
                request.conf_level
            ));
            computed.reproduce.python = Some(format!(
                "import scipy.stats as st; {py_table}; st.fisher_exact(t, alternative='{alt_py}')"
            ));
        }
        ContingencyTest::FisherExact => {
            computed.reproduce.r = Some(format!("{r_table}; fisher.test(t)"));
        }
        ContingencyTest::FisherSimulated => {
            computed.reproduce.r = Some(format!(
                "{r_table}; set.seed({}); fisher.test(t, simulate.p.value = TRUE, B = {})",
                request.seed, request.simulations
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_rc_matches_the_two_by_two_and_sums_to_one() {
        let t = vec![vec![3, 1], vec![1, 3]];
        let rc = fisher_exact_rc(&t).unwrap();
        let two = fisher_p(&[vec![3.0, 1.0], vec![1.0, 3.0]], Alternative::TwoSided).unwrap();
        assert!((rc - two).abs() < 1e-12, "{rc} vs {two}");
        // The most probable table has p = 1.
        let even = vec![vec![2, 2, 2], vec![2, 2, 2]];
        assert!((fisher_exact_rc(&even).unwrap() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn simulation_is_seeded_and_close_to_exact() {
        let t = vec![
            vec![1, 2, 1, 0],
            vec![3, 3, 6, 1],
            vec![10, 10, 14, 9],
            vec![6, 7, 12, 11],
        ];
        let exact = fisher_exact_rc(&t).unwrap();
        let sim = fisher_simulated(&t, 4000, &mut Rng::new(1));
        assert!((sim - exact).abs() < 0.03, "{sim} vs {exact}");
        assert_eq!(sim, fisher_simulated(&t, 4000, &mut Rng::new(1)));
    }
}
