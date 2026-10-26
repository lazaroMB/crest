//! Engine-computed results and their provenance.
//!
//! A [`Computed`] envelope carries the numbers a deterministic engine (the
//! statistics engine, the calc sheet, a unit conversion) produced, with how
//! they were produced: method, assumption checks, warnings, input hashes,
//! engine version, a reproduce command and the result of a recompute. The
//! envelope is versioned by its `crest_computed` field ([`COMPUTED_VERSION`]).
//!
//! The model never writes these numbers: it asks a tool to compute them and
//! cites them in its prose through number slots ([`slots`]). The
//! [`ComputedBody`] holds what each card draws (tables, matrices, sheet
//! lines); the envelope's `values` hold the headline numbers that slots and
//! the APA line ([`apa`]) refer to; [`narrative`] fills the sentence and
//! APA line templates the engine names. [`lint`] finds numbers in prose that did
//! not come from an engine.
//!
//! See `docs/research/chat-blocks-for-technical-work.md`, "The `Computed`
//! envelope".

pub mod ab_test;
pub mod apa;
pub mod beam;
pub mod bode;
pub mod bootstrap;
pub mod cash_flow;
pub mod contingency;
pub mod decision_matrix;
pub mod distribution;
pub mod equations;
pub mod forecast;
pub mod karnaugh;
pub mod lint;
pub mod narrative;
pub mod optimization;
pub mod power;
pub mod simulation;
pub mod slots;

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use self::ab_test::{AbArm, AbMethod, AbTestBody};
pub use self::beam::{
    BeamBody, BeamLoad, BeamLoadKind, BeamReaction, BeamSupport, ForceUnit, InertiaUnit,
    LengthUnit, ModulusUnit, SupportKind,
};
pub use self::bode::{BodeBody, BodePoint, Complex, Crossover, FrequencyUnit};
pub use self::bootstrap::{BootInterval, BootMethod, BootStatistic, BootstrapBody};
pub use self::cash_flow::{CashFlowBody, CashFlowRow};
pub use self::contingency::{ContingencyBody, ContingencyTest};
pub use self::decision_matrix::{
    CriterionDirection, DecisionCriterion, DecisionMatrixBody, DecisionOption, Normalization,
    WeightSensitivity, WeightThreshold,
};
pub use self::distribution::{
    BandwidthRule, BinRule, BoxSummary, Curve, DistributionBody, DistributionGroup, EcdfStep,
    QqPoint,
};
pub use self::equations::{EquationsBody, PlugBack, SolveMethod, SolvedUnknown};
pub use self::forecast::{ForecastBody, Holdout, SeasonalComponent};
pub use self::karnaugh::{CoverMethod, Implicant, KarnaughBody};
pub use self::optimization::{
    ConstraintOp, OptConstraint, OptVariable, OptimizationBody, OptimizationStatus, Sense,
};
pub use self::power::{PowerBody, PowerTarget, PowerTest};
pub use self::simulation::{SimInputSummary, SimulationBody, TornadoBar};

/// Version written in `crest_computed`. Readers accept any `1.x`.
pub const COMPUTED_VERSION: &str = "1.0";

/// The result of one engine computation, with its provenance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Computed {
    /// Envelope format version, `"1.0"`.
    pub crest_computed: String,
    /// Id that number slots use (`⟦id.key⟧`); unique within a session.
    pub id: String,
    /// What was computed, dotted: `stats.ttest_welch`, `calc.sheet`,
    /// `units.convert`.
    pub kind: String,
    /// Whether the computation succeeded.
    pub status: ComputedStatus,
    /// Why it failed, when `status` is `error`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Headline numbers by key (`t`, `df`, `p_value`...), the targets of
    /// number slots.
    #[serde(default)]
    pub values: BTreeMap<String, ComputedValue>,
    /// What kind of uncertainty the intervals express.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uncertainty_kind: Option<UncertaintyKind>,
    /// The method used and why.
    pub method: Method,
    /// Assumption checks, including the ones that were not checked.
    #[serde(default)]
    pub assumptions: Vec<Assumption>,
    /// Misuse and data warnings.
    #[serde(default)]
    pub warnings: Vec<ComputedWarning>,
    /// The data and parameters the engine read.
    #[serde(default)]
    pub inputs: Vec<InputRef>,
    /// The engine that computed it.
    pub engine: Engine,
    /// RNG seed; required for resampling kinds (bootstrap, Monte Carlo).
    #[serde(default)]
    pub seed: Option<u64>,
    /// RNG algorithm, e.g. `chacha20`.
    #[serde(default)]
    pub rng: Option<String>,
    /// Hash of the engine environment, `blake3:<hex>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment_hash: Option<String>,
    /// Commands that reproduce the result elsewhere.
    #[serde(default)]
    pub reproduce: Reproduce,
    /// The template the plain-language sentence comes from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub narrative: Option<Narrative>,
    /// When and for whom it ran.
    #[serde(default)]
    pub provenance: Provenance,
    /// Result of the last recompute.
    #[serde(default)]
    pub verification: Verification,
}

impl Computed {
    /// An empty successful envelope: no values, no checks.
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        kind: impl Into<String>,
        method: impl Into<String>,
        engine: Engine,
    ) -> Self {
        Self {
            crest_computed: COMPUTED_VERSION.to_owned(),
            id: id.into(),
            kind: kind.into(),
            status: ComputedStatus::Ok,
            error: None,
            values: BTreeMap::new(),
            uncertainty_kind: None,
            method: Method {
                name: method.into(),
                why: None,
                alternatives: Vec::new(),
                params: BTreeMap::new(),
            },
            assumptions: Vec::new(),
            warnings: Vec::new(),
            inputs: Vec::new(),
            engine,
            seed: None,
            rng: None,
            environment_hash: None,
            reproduce: Reproduce::default(),
            narrative: None,
            provenance: Provenance::default(),
            verification: Verification::default(),
        }
    }

    /// The value under `key`, if any.
    #[must_use]
    pub fn value(&self, key: &str) -> Option<&ComputedValue> {
        self.values.get(key)
    }

    /// Checks the version, the id (no `.`, which slots use as separator)
    /// and that every value and interval is finite and well ordered.
    ///
    /// # Errors
    ///
    /// The first [`ComputedError`] found.
    pub fn validate(&self) -> Result<(), ComputedError> {
        if self.crest_computed.split('.').next() != Some("1") {
            return Err(ComputedError::Version(self.crest_computed.clone()));
        }
        if !slots::is_slot_id(&self.id) {
            return Err(ComputedError::Id(self.id.clone()));
        }
        for (key, value) in &self.values {
            value.validate(key)?;
        }
        Ok(())
    }

    /// The values that differ between `self` and a recomputed `other`.
    ///
    /// Two values match when `|a - b| <= tolerance` (0 for closed-form
    /// kinds, k·MCSE for Monte Carlo ones). A key on one side only is a
    /// difference too.
    #[must_use]
    pub fn diff_values(&self, other: &Self, tolerance: f64) -> Vec<ValueDiff> {
        let mut diffs: Vec<ValueDiff> = self
            .values
            .iter()
            .filter_map(|(key, old)| {
                let new = other.values.get(key).map(|v| v.v);
                let same = new.is_some_and(|n| (n - old.v).abs() <= tolerance);
                (!same).then(|| ValueDiff {
                    key: key.clone(),
                    old: Some(old.v),
                    new,
                })
            })
            .collect();
        diffs.extend(
            other
                .values
                .iter()
                .filter(|(key, _)| !self.values.contains_key(*key))
                .map(|(key, new)| ValueDiff {
                    key: key.clone(),
                    old: None,
                    new: Some(new.v),
                }),
        );
        diffs
    }
}

/// Outcome of a computation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ComputedStatus {
    /// Computed without problems.
    #[default]
    Ok,
    /// Computed, but with warnings the card shows.
    Warning,
    /// Not computed; `error` says why.
    Error,
}

/// One engine number. `v` is the stored value; the other fields only change
/// how it is shown.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ComputedValue {
    /// Raw value, never rounded.
    pub v: f64,
    /// Preformatted text the engine chose, e.g. `−3.77`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<String>,
    /// Unit, from the engine (never the model), e.g. `mi/gal`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// Significant digits to show.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sig: Option<u32>,
    /// Decimals to show (used when `sig` is absent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decimals: Option<u32>,
    /// Formatting rule.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fmt: Option<ValueFormat>,
    /// Confidence or credible interval.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ci: Option<Interval>,
    /// A verbal label, e.g. `large` for an effect size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Where the label comes from, e.g. `Cohen 1988`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label_rule: Option<String>,
}

impl ComputedValue {
    /// A bare value with no formatting hints.
    #[must_use]
    pub fn new(v: f64) -> Self {
        Self {
            v,
            display: None,
            unit: None,
            sig: None,
            decimals: None,
            fmt: None,
            ci: None,
            label: None,
            label_rule: None,
        }
    }

    /// The text a number slot shows: `display` when the engine gave one,
    /// otherwise `v` formatted by `fmt`, `sig` or `decimals` (two by
    /// default), followed by the unit.
    #[must_use]
    pub fn display_text(&self) -> String {
        let number = match &self.display {
            Some(display) => display.clone(),
            None => self.format_number(),
        };
        match &self.unit {
            Some(unit) if !unit.is_empty() => format!("{number} {unit}"),
            _ => number,
        }
    }

    fn format_number(&self) -> String {
        let decimals = self.decimals.unwrap_or(2);
        match self.fmt {
            Some(ValueFormat::Apa) => apa::p_value_text(self.v),
            Some(ValueFormat::Bounded) => apa::bounded(self.v, decimals),
            Some(ValueFormat::Percent) => format!("{}%", apa::number(self.v * 100.0, decimals)),
            Some(ValueFormat::Scientific) => {
                apa::with_minus(&format!("{:.*e}", decimals as usize, self.v))
            }
            Some(ValueFormat::Fixed) | None => match self.sig {
                Some(sig) => apa::significant(self.v, sig),
                None => apa::number(self.v, decimals),
            },
        }
    }

    fn validate(&self, key: &str) -> Result<(), ComputedError> {
        if !self.v.is_finite() {
            return Err(ComputedError::NonFinite {
                key: key.to_owned(),
            });
        }
        if let Some(ci) = &self.ci {
            let finite = ci.lo.is_finite() && ci.hi.is_finite();
            if !finite || ci.lo > ci.hi || !(ci.level > 0.0 && ci.level < 1.0) {
                return Err(ComputedError::Interval {
                    key: key.to_owned(),
                });
            }
        }
        Ok(())
    }
}

/// How a [`ComputedValue`] is formatted when it has no `display`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ValueFormat {
    /// An APA p-value: `.043`, `< .001`.
    Apa,
    /// A statistic bounded by 1 (r, R², proportions): no leading zero.
    Bounded,
    /// Fixed decimals, or `sig` significant digits.
    Fixed,
    /// `v` is a fraction shown as a percentage.
    Percent,
    /// Scientific notation, `1.23e−4`.
    Scientific,
}

/// A confidence or credible interval.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Interval {
    /// Coverage, e.g. `0.95`.
    pub level: f64,
    /// Lower bound.
    pub lo: f64,
    /// Upper bound.
    pub hi: f64,
    /// How it was computed, e.g. `welch_t`, `noncentral_t`, `bca`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
}

/// What the intervals of an envelope express.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum UncertaintyKind {
    /// Frequentist confidence intervals.
    FrequentistCi,
    /// Bootstrap intervals.
    BootstrapCi,
    /// Bayesian credible intervals.
    CredibleInterval,
    /// Monte Carlo standard errors.
    MonteCarlo,
    /// Exact arithmetic; no uncertainty.
    Exact,
}

/// The method an engine used.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Method {
    /// Human name, e.g. `Welch two-sample t-test`.
    pub name: String,
    /// Why it was chosen, e.g. `Levene p = .02 → unequal variances`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    /// Kinds that could be run instead, e.g. `mann_whitney`.
    #[serde(default)]
    pub alternatives: Vec<String>,
    /// Parameters, e.g. `{"alternative": "two-sided"}`.
    #[serde(default)]
    pub params: BTreeMap<String, Value>,
}

/// One assumption of the method and whether it holds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Assumption {
    /// Stable id, e.g. `normality_g1`.
    pub id: String,
    /// The check run, e.g. `Shapiro-Wilk`; absent when not checked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<String>,
    /// The check's statistic.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stat: Option<f64>,
    /// The check's p-value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p: Option<f64>,
    /// Outcome.
    pub status: AssumptionStatus,
    /// What the engine did about it, e.g. `used Welch correction`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consequence: Option<String>,
    /// A note for the user, e.g. `design property; confirm`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Outcome of an assumption check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AssumptionStatus {
    /// Holds.
    Pass,
    /// Doubtful.
    Warn,
    /// Violated.
    Fail,
    /// Not checkable by the engine (a design property); shown on purpose.
    NotChecked,
}

/// A misuse or data warning attached by the engine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ComputedWarning {
    /// Stable code, e.g. `small_n`, `expected_count_lt_5`.
    pub code: String,
    /// How serious it is.
    pub severity: WarningSeverity,
    /// Text for the user.
    pub text: String,
}

/// Severity of a [`ComputedWarning`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WarningSeverity {
    /// Worth knowing.
    Info,
    /// May change the conclusion.
    Warn,
    /// The result should not be used as is.
    Error,
}

/// Data or parameters an engine read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InputRef {
    /// File name or a label for inline data.
    pub name: String,
    /// `data` or `param`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// Workspace-relative path, when read from a file; a change to it marks
    /// the block stale.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Content hash, `blake3:<hex>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
    /// Number of rows read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rows: Option<u64>,
    /// Columns used.
    #[serde(default)]
    pub columns: Vec<String>,
    /// Row filter applied, e.g. `none`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<String>,
}

/// The engine that produced an envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Engine {
    /// Engine name, e.g. `crest-stats`.
    pub name: String,
    /// Engine version.
    pub version: String,
    /// Library underneath, e.g. `statrs 0.19.1`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<String>,
    /// Target triple, e.g. `x86_64-linux`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
}

/// One-line commands that reproduce a result.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Reproduce {
    /// crest Lua, e.g. `app.compute.test{...}`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lua: Option<String>,
    /// Python (scipy, statsmodels).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub python: Option<String>,
    /// R.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r: Option<String>,
    /// A shell command of a standalone tool, e.g. `numbat -e '…'`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell: Option<String>,
}

/// The template of the plain-language sentence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Narrative {
    /// Template name, e.g. `apa_ttest`.
    pub template: String,
    /// Value keys the sentence uses.
    #[serde(default)]
    pub text_slots: Vec<String>,
}

/// When and for whom a computation ran.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Provenance {
    /// Start time, RFC 3339.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity_started: Option<String>,
    /// How long it took.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// The turn or agent that asked for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// `user` or `model`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_by: Option<String>,
}

/// Result of re-running an envelope.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Verification {
    /// When it was recomputed, RFC 3339; `None` until then.
    #[serde(default)]
    pub recomputed_at: Option<String>,
    /// Whether every value matched; `None` until recomputed.
    #[serde(default, rename = "match")]
    pub matched: Option<bool>,
    /// The values that changed; `null` reads as none.
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "null_as_default"
    )]
    #[schemars(with = "Option<Vec<ValueDiff>>")]
    pub diff: Vec<ValueDiff>,
}

/// Reads `null` as the type's default.
fn null_as_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Option::unwrap_or_default)
}

/// A value that changed on recompute.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ValueDiff {
    /// Value key.
    pub key: String,
    /// Stored value; `None` when only the recompute has it.
    pub old: Option<f64>,
    /// Recomputed value; `None` when the recompute lost it.
    pub new: Option<f64>,
}

/// Why an envelope or card body is not valid.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ComputedError {
    /// A `crest_computed` version this build cannot read.
    #[error("unsupported crest_computed version {0:?}, expected 1.x")]
    Version(String),
    /// An id that is empty or has characters slots cannot carry.
    #[error("computed id {0:?} must be letters, digits, '_' or '-'")]
    Id(String),
    /// A NaN or infinite value.
    #[error("value {key:?} is not finite")]
    NonFinite {
        /// Value key.
        key: String,
    },
    /// An interval with a bad level or reversed bounds.
    #[error("value {key:?} has an invalid interval")]
    Interval {
        /// Value key.
        key: String,
    },
    /// A card body whose shape is inconsistent.
    #[error("{0}")]
    Body(String),
}

/// What a computed card draws, tagged by `type`.
// The block boxes the whole body, so a large variant costs nothing there.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ComputedBody {
    /// Per-variable summary statistics.
    Descriptive(DescriptiveBody),
    /// A hypothesis test.
    Test(TestBody),
    /// A linear regression.
    Regression(RegressionBody),
    /// A correlation matrix.
    Correlation(CorrelationBody),
    /// A unit-checked calc sheet.
    CalcSheet(CalcSheetBody),
    /// A unit conversion.
    Quantity(QuantityBody),
    /// Histogram, density, box/violin, QQ and ECDF of one variable.
    Distribution(DistributionBody),
    /// A contingency table with residuals and a test of independence.
    Contingency(ContingencyBody),
    /// A power or sample-size calculation.
    Power(PowerBody),
    /// An A/B test of two conversion rates.
    AbTest(AbTestBody),
    /// A bootstrap of one statistic.
    Bootstrap(BootstrapBody),
    /// A time-series decomposition and forecast.
    Forecast(ForecastBody),
    /// A linear or mixed-integer program.
    Optimization(OptimizationBody),
    /// An equation or a small system solved numerically.
    Equations(EquationsBody),
    /// A Monte Carlo simulation, tornado and Fermi estimate.
    Simulation(SimulationBody),
    /// A cash flow's NPV, IRR and MIRR.
    CashFlow(CashFlowBody),
    /// A weighted decision matrix.
    DecisionMatrix(DecisionMatrixBody),
    /// A transfer function's Bode and Nyquist plots with its margins.
    Bode(BodeBody),
    /// A Boolean function's Karnaugh map with minimal SOP and POS.
    Karnaugh(KarnaughBody),
    /// A beam's reactions, shear, moment and deflection.
    Beam(BeamBody),
}

impl ComputedBody {
    /// Checks that tables and matrices have consistent shapes.
    ///
    /// # Errors
    ///
    /// [`ComputedError::Body`] describing the first problem.
    pub fn validate(&self) -> Result<(), ComputedError> {
        match self {
            Self::Descriptive(body) => body
                .variables
                .iter()
                .try_for_each(|v| v.histogram.as_ref().map_or(Ok(()), Histogram::validate)),
            Self::Regression(body) => {
                if body.fitted.len() == body.residuals.len() {
                    Ok(())
                } else {
                    Err(ComputedError::Body(format!(
                        "regression has {} fitted values but {} residuals",
                        body.fitted.len(),
                        body.residuals.len()
                    )))
                }
            }
            Self::Correlation(body) => body.validate(),
            Self::Distribution(body) => body.validate(),
            Self::Contingency(body) => body.validate(),
            Self::Power(body) => body.validate(),
            Self::AbTest(body) => body.validate(),
            Self::Bootstrap(body) => body.validate(),
            Self::Forecast(body) => body.validate(),
            Self::Optimization(body) => body.validate(),
            Self::Equations(body) => body.validate(),
            Self::Simulation(body) => body.validate(),
            Self::CashFlow(body) => body.validate(),
            Self::DecisionMatrix(body) => body.validate(),
            Self::Bode(body) => body.validate(),
            Self::Karnaugh(body) => body.validate(),
            Self::Beam(body) => body.validate(),
            Self::Test(_) | Self::CalcSheet(_) | Self::Quantity(_) => Ok(()),
        }
    }
}

/// Binned counts: `edges` has one more entry than `counts`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Histogram {
    /// Bin edges, ascending.
    pub edges: Vec<f64>,
    /// Count per bin.
    pub counts: Vec<u64>,
}

impl Histogram {
    /// Checks that there is one more edge than counts and edges ascend.
    ///
    /// # Errors
    ///
    /// [`ComputedError::Body`] when the shape is wrong.
    pub fn validate(&self) -> Result<(), ComputedError> {
        let ascending = self.edges.windows(2).all(|w| w[0] <= w[1]);
        if self.edges.len() == self.counts.len() + 1 && ascending {
            Ok(())
        } else {
            Err(ComputedError::Body(
                "histogram needs ascending edges, one more than counts".into(),
            ))
        }
    }
}

/// Body of a descriptive summary: one row per variable (and group).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DescriptiveBody {
    /// Grouping column (`by=`), when rows are split by group.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    /// One summary per variable, or per variable and group.
    pub variables: Vec<VariableSummary>,
}

/// Summary of one variable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct VariableSummary {
    /// Column name.
    pub name: String,
    /// Group value, when split by `by`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// Numeric or categorical.
    pub kind: VariableKind,
    /// Unit of the values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// Non-missing values.
    pub n: u64,
    /// Missing values.
    #[serde(default)]
    pub missing: u64,
    /// Mean (numeric).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mean: Option<f64>,
    /// Sample standard deviation (numeric).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sd: Option<f64>,
    /// Median (numeric).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub median: Option<f64>,
    /// First quartile (numeric).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub q1: Option<f64>,
    /// Third quartile (numeric).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub q3: Option<f64>,
    /// Minimum (numeric).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    /// Maximum (numeric).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    /// Sample skewness (numeric).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skew: Option<f64>,
    /// Excess kurtosis (numeric).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kurtosis: Option<f64>,
    /// Distribution for the sparkline (numeric).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub histogram: Option<Histogram>,
    /// Counts per level (categorical), most frequent first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub levels: Vec<LevelCount>,
}

/// Measurement level of a variable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum VariableKind {
    /// Numbers.
    Numeric,
    /// Categories.
    Categorical,
}

/// Count of one level of a categorical variable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct LevelCount {
    /// Level.
    pub level: String,
    /// Rows with this level.
    pub count: u64,
    /// Share of non-missing rows, 0–100.
    pub percent: f64,
}

/// Body of a hypothesis test. The statistic, df, p and effect size are in
/// the envelope's `values` (`statistic`, `df`, `p_value`, `effect`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TestBody {
    /// Which test ran.
    pub test: TestKind,
    /// Direction of the alternative hypothesis.
    #[serde(default)]
    pub alternative: Alternative,
    /// Per-group summary, in input order.
    #[serde(default)]
    pub groups: Vec<GroupSummary>,
    /// Observed counts, for χ² and Fisher (rows × columns).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed: Option<Vec<Vec<f64>>>,
    /// Expected counts under independence, same shape as `observed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected: Option<Vec<Vec<f64>>>,
}

/// A hypothesis test the engine runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TestKind {
    /// Welch two-sample t-test (unequal variances).
    WelchT,
    /// Student two-sample t-test (equal variances).
    StudentT,
    /// Paired t-test.
    PairedT,
    /// One-sample t-test.
    OneSampleT,
    /// Mann-Whitney U (Wilcoxon rank-sum).
    MannWhitney,
    /// Wilcoxon signed-rank.
    Wilcoxon,
    /// Pearson χ² test of independence.
    ChiSquare,
    /// Fisher's exact test, 2×2.
    Fisher,
}

impl TestKind {
    /// Symbol of the test statistic in an APA line (`t`, `U`, `W`, `χ²`;
    /// Fisher has none and reports an odds ratio).
    #[must_use]
    pub fn statistic_symbol(self) -> &'static str {
        match self {
            Self::WelchT | Self::StudentT | Self::PairedT | Self::OneSampleT => "t",
            Self::MannWhitney => "U",
            Self::Wilcoxon => "W",
            Self::ChiSquare => "χ²",
            Self::Fisher => "OR",
        }
    }
}

/// Alternative hypothesis.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Alternative {
    /// Different in either direction.
    #[default]
    TwoSided,
    /// First less than second.
    Less,
    /// First greater than second.
    Greater,
}

/// Summary of one group in a test.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct GroupSummary {
    /// Group name.
    pub name: String,
    /// Size.
    pub n: u64,
    /// Mean.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mean: Option<f64>,
    /// Standard deviation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sd: Option<f64>,
    /// Median.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub median: Option<f64>,
}

/// Body of an OLS regression.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RegressionBody {
    /// Dependent variable.
    pub dependent: String,
    /// Level of the coefficient intervals, e.g. `0.95`.
    pub ci_level: f64,
    /// One row per term, intercept first.
    pub coefficients: Vec<CoefficientRow>,
    /// Goodness of fit.
    pub fit: FitStats,
    /// Residual diagnostics.
    #[serde(default)]
    pub diagnostics: Diagnostics,
    /// Fitted values, in row order (for residual plots).
    #[serde(default)]
    pub fitted: Vec<f64>,
    /// Residuals, aligned with `fitted`.
    #[serde(default)]
    pub residuals: Vec<f64>,
}

/// One coefficient of a regression.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CoefficientRow {
    /// Term, e.g. `(Intercept)`, `wt`.
    pub term: String,
    /// Estimate.
    pub estimate: f64,
    /// Standard error.
    pub se: f64,
    /// t statistic.
    pub t: f64,
    /// Two-sided p-value.
    pub p: f64,
    /// Lower interval bound.
    pub ci_lo: f64,
    /// Upper interval bound.
    pub ci_hi: f64,
    /// Standardised coefficient.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub std_beta: Option<f64>,
    /// Variance inflation factor (not for the intercept).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vif: Option<f64>,
}

/// Goodness of fit of a regression.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct FitStats {
    /// Observations used.
    pub n: u64,
    /// R².
    pub r_squared: f64,
    /// Adjusted R².
    pub adj_r_squared: f64,
    /// F statistic of the overall test.
    pub f: f64,
    /// Model degrees of freedom.
    pub df_model: f64,
    /// Residual degrees of freedom.
    pub df_resid: f64,
    /// p-value of the F test.
    pub f_p: f64,
    /// Residual standard error.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sigma: Option<f64>,
    /// Akaike information criterion.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aic: Option<f64>,
    /// Bayesian information criterion.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bic: Option<f64>,
}

/// Residual diagnostics of a regression (the statsmodels footer).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Diagnostics {
    /// Durbin-Watson statistic.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub durbin_watson: Option<f64>,
    /// Jarque-Bera statistic.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jarque_bera: Option<f64>,
    /// Jarque-Bera p-value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jarque_bera_p: Option<f64>,
    /// Breusch-Pagan statistic.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub breusch_pagan: Option<f64>,
    /// Breusch-Pagan p-value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub breusch_pagan_p: Option<f64>,
    /// Condition number of the design matrix.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition_number: Option<f64>,
    /// Largest VIF.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_vif: Option<f64>,
    /// Largest Cook's distance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_cooks_d: Option<f64>,
}

/// Body of a correlation matrix. `r`, `p` and `n` are square, one row and
/// column per variable; a `None` cell could not be computed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CorrelationBody {
    /// Coefficient.
    pub method: CorrelationMethod,
    /// Multiple-comparison adjustment applied to `p`.
    #[serde(default)]
    pub adjustment: PAdjust,
    /// Variable names, the matrix order.
    pub variables: Vec<String>,
    /// Coefficients.
    pub r: Vec<Vec<Option<f64>>>,
    /// Adjusted p-values; the diagonal is `None`.
    pub p: Vec<Vec<Option<f64>>>,
    /// Pairwise complete observations.
    pub n: Vec<Vec<u64>>,
}

impl CorrelationBody {
    fn validate(&self) -> Result<(), ComputedError> {
        let size = self.variables.len();
        fn square<T>(matrix: &[Vec<T>], size: usize) -> bool {
            matrix.len() == size && matrix.iter().all(|row| row.len() == size)
        }
        if square(&self.r, size) && square(&self.p, size) && square(&self.n, size) {
            Ok(())
        } else {
            Err(ComputedError::Body(format!(
                "correlation matrices must be {size}×{size}"
            )))
        }
    }
}

/// Correlation coefficient.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CorrelationMethod {
    /// Pearson's r.
    Pearson,
    /// Spearman's ρ.
    Spearman,
    /// Kendall's τ-b.
    Kendall,
}

/// Multiple-comparison adjustment of p-values.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PAdjust {
    /// Raw p-values.
    #[default]
    None,
    /// Holm step-down.
    Holm,
    /// Benjamini-Hochberg false discovery rate.
    Bh,
    /// Bonferroni.
    Bonferroni,
}

/// Body of a calc sheet: lines evaluated in order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CalcSheetBody {
    /// Lines, in evaluation order.
    pub lines: Vec<CalcLine>,
}

/// One line of a calc sheet, `name = expr`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CalcLine {
    /// Name later lines use; `None` for a bare expression.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Expression as written.
    pub expr: String,
    /// Numeric value in `unit`; `None` on error or for non-numeric results.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
    /// The engine's rendering of the result, e.g. `9.81 m/s²`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<String>,
    /// Unit of `value`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// LaTeX of the line, when the engine can produce it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latex: Option<String>,
    /// Physical dimension of the result, e.g. `Length` or `Energy`;
    /// `None` for a plain number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dimension: Option<String>,
    /// Evaluation error, e.g. a dimension mismatch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// How the engine read an ambiguous name, e.g. `ton read as the metric
    /// tonne (1000 kg)`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// Body of a unit conversion.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct QuantityBody {
    /// What was converted.
    pub from: Quantity,
    /// The result.
    pub to: Quantity,
    /// Multiplier from `from.unit` to `to.unit` (for affine units like °C,
    /// the slope).
    pub factor: f64,
    /// Whether the factor is exact by definition (inch → mm) rather than
    /// rounded.
    pub exact: bool,
    /// Physical dimension, e.g. `length`, `energy / time`.
    pub dimension: String,
    /// Steps through base units, in order.
    #[serde(default)]
    pub chain: Vec<ConversionStep>,
}

/// A value with a unit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Quantity {
    /// Magnitude.
    pub value: f64,
    /// Unit, e.g. `km/h`.
    pub unit: String,
    /// Standard uncertainty of `value`, in `unit`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uncertainty: Option<f64>,
}

/// One step of a conversion chain.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ConversionStep {
    /// Unit converted from.
    pub from_unit: String,
    /// Unit converted to.
    pub to_unit: String,
    /// Multiplier of this step.
    pub factor: f64,
    /// E.g. `exact by definition (1959)`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn engine() -> Engine {
        Engine {
            name: "crest-stats".into(),
            version: "0.15.0".into(),
            backend: Some("statrs 0.19.1".into()),
            platform: None,
        }
    }

    fn envelope() -> Computed {
        let mut c = Computed::new(
            "calc_7f3a",
            "stats.ttest_welch",
            "Welch two-sample t-test",
            engine(),
        );
        c.values.insert("t".into(), ComputedValue::new(-3.767));
        c.values.insert(
            "p_value".into(),
            ComputedValue {
                fmt: Some(ValueFormat::Apa),
                ..ComputedValue::new(0.001_374)
            },
        );
        c
    }

    #[test]
    fn envelope_roundtrips_with_doc_field_names() {
        let mut c = envelope();
        c.verification.matched = Some(true);
        let value = serde_json::to_value(&c).unwrap();
        assert_eq!(value["crest_computed"], "1.0");
        assert_eq!(value["status"], "ok");
        assert_eq!(value["values"]["p_value"]["fmt"], "apa");
        assert_eq!(value["verification"]["match"], true);
        assert!(value["values"]["t"].get("display").is_none());
        let back: Computed = serde_json::from_value(value).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn envelope_reads_the_research_doc_example() {
        let c: Computed = serde_json::from_value(json!({
            "crest_computed": "1.0", "id": "calc_7f3a", "kind": "stats.ttest_welch",
            "status": "ok",
            "values": {"df": {"v": 18.33, "display": "18.33"},
                       "mean_diff": {"v": -7.25, "unit": "mi/gal",
                                     "ci": {"level": 0.95, "lo": -11.28, "hi": -3.21, "method": "welch_t"}}},
            "uncertainty_kind": "frequentist_ci",
            "method": {"name": "Welch", "alternatives": ["student_t"], "params": {"alternative": "two-sided"}},
            "assumptions": [{"id": "independence", "status": "not_checked", "note": "confirm"}],
            "warnings": [{"code": "small_n", "severity": "warn", "text": "n < 20 per group"}],
            "inputs": [{"name": "mtcars.csv", "role": "data", "hash": "blake3:9c1e", "rows": 32,
                        "columns": ["mpg", "am"], "filter": "none"}],
            "engine": {"name": "crest-stats", "version": "0.15.0"},
            "seed": null, "rng": null,
            "reproduce": {"r": "t.test(mpg ~ am, data = mtcars)"},
            "narrative": {"template": "apa_ttest", "text_slots": ["df"]},
            "provenance": {"duration_ms": 3},
            "verification": {"recomputed_at": null, "match": null, "diff": null}
        }))
        .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(c.validate(), Ok(()));
        assert_eq!(c.assumptions[0].status, AssumptionStatus::NotChecked);
        assert_eq!(c.value("mean_diff").unwrap().display_text(), "−7.25 mi/gal");
    }

    #[test]
    fn validate_rejects_bad_envelopes() {
        let mut c = envelope();
        c.crest_computed = "2.0".into();
        assert_eq!(c.validate(), Err(ComputedError::Version("2.0".into())));
        let mut c = envelope();
        c.id = "a.b".into();
        assert_eq!(c.validate(), Err(ComputedError::Id("a.b".into())));
        let mut c = envelope();
        c.values.insert("x".into(), ComputedValue::new(f64::NAN));
        assert_eq!(
            c.validate(),
            Err(ComputedError::NonFinite { key: "x".into() })
        );
        let mut c = envelope();
        c.values.insert(
            "d".into(),
            ComputedValue {
                ci: Some(Interval {
                    level: 0.95,
                    lo: 1.0,
                    hi: 0.0,
                    method: None,
                }),
                ..ComputedValue::new(0.5)
            },
        );
        assert_eq!(
            c.validate(),
            Err(ComputedError::Interval { key: "d".into() })
        );
    }

    #[test]
    fn display_text_follows_format_hints() {
        let show = |value: ComputedValue| value.display_text();
        assert_eq!(show(ComputedValue::new(2.0)), "2.00");
        assert_eq!(
            show(ComputedValue {
                sig: Some(3),
                ..ComputedValue::new(-0.012_345)
            }),
            "−0.0123"
        );
        assert_eq!(
            show(ComputedValue {
                fmt: Some(ValueFormat::Apa),
                ..ComputedValue::new(0.0004)
            }),
            "< .001"
        );
        assert_eq!(
            show(ComputedValue {
                fmt: Some(ValueFormat::Bounded),
                ..ComputedValue::new(0.456)
            }),
            ".46"
        );
        assert_eq!(
            show(ComputedValue {
                fmt: Some(ValueFormat::Percent),
                decimals: Some(1),
                ..ComputedValue::new(0.1234)
            }),
            "12.3%"
        );
        assert_eq!(
            show(ComputedValue {
                display: Some("9.81".into()),
                unit: Some("m/s²".into()),
                ..ComputedValue::new(9.806_65)
            }),
            "9.81 m/s²"
        );
    }

    #[test]
    fn diff_values_compares_within_tolerance() {
        let old = envelope();
        let mut new = envelope();
        assert!(old.diff_values(&new, 0.0).is_empty());
        new.values.get_mut("t").unwrap().v = -3.7671;
        new.values.remove("p_value");
        new.values.insert("df".into(), ComputedValue::new(18.0));
        assert!(old.diff_values(&new, 1e-3).iter().all(|d| d.key != "t"));
        let diffs = old.diff_values(&new, 0.0);
        let keys: Vec<_> = diffs.iter().map(|d| d.key.as_str()).collect();
        assert_eq!(keys, ["p_value", "t", "df"]);
        assert_eq!(diffs[0].new, None);
        assert_eq!(diffs[2].old, None);
    }

    #[test]
    fn bodies_roundtrip_and_validate_shapes() {
        let body = ComputedBody::Correlation(CorrelationBody {
            method: CorrelationMethod::Spearman,
            adjustment: PAdjust::Holm,
            variables: vec!["a".into(), "b".into()],
            r: vec![vec![Some(1.0), Some(0.4)], vec![Some(0.4), Some(1.0)]],
            p: vec![vec![None, Some(0.03)], vec![Some(0.03), None]],
            n: vec![vec![10, 10], vec![10, 10]],
        });
        assert_eq!(body.validate(), Ok(()));
        let value = serde_json::to_value(&body).unwrap();
        assert_eq!(value["type"], "correlation");
        assert_eq!(value["adjustment"], "holm");
        let back: ComputedBody = serde_json::from_value(value).unwrap();
        assert_eq!(back, body);

        let ComputedBody::Correlation(mut bad) = body else {
            unreachable!()
        };
        bad.n.pop();
        assert!(ComputedBody::Correlation(bad).validate().is_err());

        let hist = Histogram {
            edges: vec![0.0, 1.0],
            counts: vec![3, 4],
        };
        assert!(hist.validate().is_err());

        let sheet: ComputedBody = serde_json::from_value(json!({
            "type": "calc_sheet",
            "lines": [{"name": "g", "expr": "9.81 m/s^2", "value": 9.81, "unit": "m/s²"},
                      {"expr": "g + 1 kg", "error": "cannot add acceleration and mass"}]
        }))
        .unwrap();
        assert_eq!(sheet.validate(), Ok(()));
        assert_eq!(TestKind::ChiSquare.statistic_symbol(), "χ²");
    }

    #[test]
    fn computed_schema() {
        insta::assert_json_snapshot!("computed_schema", schemars::schema_for!(Computed));
        insta::assert_json_snapshot!("computed_body_schema", schemars::schema_for!(ComputedBody));
    }
}
