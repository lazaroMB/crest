//! The deterministic compute engine behind the statistics cards.
//!
//! The model decides what to compute; a [`ComputePort`] computes it and
//! returns a [`Computed`] envelope (values, method, assumption checks,
//! warnings, input hashes, engine, reproduce lines, provenance) with the
//! [`ComputedBody`] the card draws. The model never writes these numbers.
//!
//! Requests carry the data already loaded as named columns ([`DataSet`]);
//! reading a file inside the workspace is the tool's job, which records
//! where the data came from in [`DataSet::source`]. Requests double as the
//! JSON input of the statistics tools (their `JsonSchema` becomes the tool
//! schema) and are stored whole, data included, in the envelope's
//! `method.params["request"]` ([`REQUEST_PARAM`]), so
//! [`ComputePort::recompute`] can re-run an envelope on its own.
//!
//! Engines are CPU-bound and synchronous: async callers run them on a
//! blocking thread.

use domain::computed::{
    Alternative, Computed, ComputedBody, CorrelationMethod, InputRef, PAdjust, TestKind,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Key of `method.params` that holds the serialized [`ComputeRequest`].
pub const REQUEST_PARAM: &str = "request";

/// What every engine call returns: the envelope and the card body.
pub type ComputeResult = Result<(Computed, ComputedBody), ComputeError>;

/// Why a request could not be computed at all.
///
/// Problems with the data itself (a constant column, collinear
/// predictors, too few observations) are not errors of the call: the
/// engine returns an envelope with `status: error` and its `error` text,
/// so the card can say what went wrong.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ComputeError {
    /// The request does not make sense: an unknown column, a numeric test
    /// on a text column, a missing argument.
    #[error("invalid request: {0}")]
    Invalid(String),
    /// The envelope does not carry a request this engine can re-run.
    #[error("cannot recompute: {0}")]
    NotRecomputable(String),
}

/// Data handed to the engine: named columns of equal length.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DataSet {
    /// Where the data came from: a file name and workspace path, or a label
    /// such as `inline`. The engine fills in `hash`, `rows` and `columns`.
    #[serde(default = "inline_source")]
    pub source: InputRef,
    /// The columns, all with the same number of rows.
    pub columns: Vec<DataColumn>,
}

impl DataSet {
    /// Inline data with the default `inline` source.
    #[must_use]
    pub fn inline(columns: Vec<DataColumn>) -> Self {
        Self {
            source: inline_source(),
            columns,
        }
    }

    /// The column called `name`.
    #[must_use]
    pub fn column(&self, name: &str) -> Option<&DataColumn> {
        self.columns.iter().find(|c| c.name == name)
    }
}

impl Default for DataSet {
    fn default() -> Self {
        Self::inline(Vec::new())
    }
}

/// The source of data typed into the request.
#[must_use]
pub fn inline_source() -> InputRef {
    InputRef {
        name: "inline".into(),
        role: Some("data".into()),
        path: None,
        hash: None,
        rows: None,
        columns: Vec::new(),
        filter: None,
    }
}

/// One named column.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DataColumn {
    /// Column name, as the other request fields refer to it.
    pub name: String,
    /// The values; `null` is a missing value.
    pub values: ColumnValues,
    /// Unit of numeric values, e.g. `mi/gal`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
}

impl DataColumn {
    /// A numeric column without missing values.
    #[must_use]
    pub fn numeric(name: impl Into<String>, values: &[f64]) -> Self {
        Self {
            name: name.into(),
            values: ColumnValues::Numeric(values.iter().copied().map(Some).collect()),
            unit: None,
        }
    }

    /// A categorical column without missing values.
    #[must_use]
    pub fn categorical<S: AsRef<str>>(name: impl Into<String>, values: &[S]) -> Self {
        Self {
            name: name.into(),
            values: ColumnValues::Categorical(
                values.iter().map(|v| Some(v.as_ref().to_owned())).collect(),
            ),
            unit: None,
        }
    }

    /// Number of rows, missing ones included.
    #[must_use]
    pub fn len(&self) -> usize {
        match &self.values {
            ColumnValues::Numeric(v) => v.len(),
            ColumnValues::Categorical(v) => v.len(),
        }
    }

    /// Whether the column has no rows.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// The values of a column: all numbers or all text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum ColumnValues {
    /// Numbers (`null` = missing).
    Numeric(Vec<Option<f64>>),
    /// Categories (`null` = missing).
    Categorical(Vec<Option<String>>),
}

/// Input of `describe`: summary statistics per variable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DescribeRequest {
    /// The data.
    pub data: DataSet,
    /// Columns to summarise; all of them (except `by`) when empty.
    #[serde(default)]
    pub variables: Vec<String>,
    /// Grouping column: one summary per variable and group.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
}

/// Which hypothesis test to run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TestChoice {
    /// Let the engine choose from the data and the assumption checks, and
    /// say why in `method.why`.
    #[default]
    Auto,
    /// Welch two-sample t-test.
    WelchT,
    /// Student two-sample t-test (equal variances).
    StudentT,
    /// Paired t-test.
    PairedT,
    /// One-sample t-test against `mu`.
    OneSampleT,
    /// Mann-Whitney U (Wilcoxon rank-sum).
    MannWhitney,
    /// Wilcoxon signed-rank (one sample or paired).
    Wilcoxon,
    /// Pearson χ² test of independence.
    ChiSquare,
    /// Fisher's exact test (2×2).
    Fisher,
}

impl TestChoice {
    /// The test this choice names; `None` for [`TestChoice::Auto`].
    #[must_use]
    pub fn kind(self) -> Option<TestKind> {
        Some(match self {
            Self::Auto => return None,
            Self::WelchT => TestKind::WelchT,
            Self::StudentT => TestKind::StudentT,
            Self::PairedT => TestKind::PairedT,
            Self::OneSampleT => TestKind::OneSampleT,
            Self::MannWhitney => TestKind::MannWhitney,
            Self::Wilcoxon => TestKind::Wilcoxon,
            Self::ChiSquare => TestKind::ChiSquare,
            Self::Fisher => TestKind::Fisher,
        })
    }
}

fn default_conf_level() -> f64 {
    0.95
}

fn default_true() -> bool {
    true
}

/// Input of `test`: one hypothesis test.
///
/// Shapes:
/// - two groups: numeric `y` split by a two-level `group` column, or two
///   numeric columns `y` and `y2`;
/// - paired: `y` and `y2` with `paired = true` (or `test = paired_t`);
/// - one sample: `y` alone, against `mu`;
/// - contingency (χ², Fisher): two categorical columns `y` and `group`, or
///   the counts in `table`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TestRequest {
    /// The data; may be empty when `table` gives the counts.
    #[serde(default)]
    pub data: DataSet,
    /// The test; `auto` chooses one.
    #[serde(default)]
    pub test: TestChoice,
    /// Outcome column (numeric), or the row variable of a contingency
    /// table (categorical).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<String>,
    /// Grouping column with two levels, or the column variable of a
    /// contingency table.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// Second numeric column: the other sample, or the second measurement
    /// when paired.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y2: Option<String>,
    /// Whether `y` and `y2` are paired measurements of the same units.
    #[serde(default)]
    pub paired: bool,
    /// Order of the two groups (first − second); sorted levels when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub levels: Vec<String>,
    /// Null value of the mean (one-sample) or mean difference (paired).
    #[serde(default)]
    pub mu: f64,
    /// Counts of a contingency table, rows × columns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub table: Option<Vec<Vec<f64>>>,
    /// Direction of the alternative hypothesis.
    #[serde(default)]
    pub alternative: Alternative,
    /// Confidence level of the intervals (default 0.95).
    #[serde(default = "default_conf_level")]
    pub conf_level: f64,
    /// Continuity correction for the normal approximations of the rank
    /// tests and for χ² on 2×2 tables (default true, as in R).
    #[serde(default = "default_true")]
    pub correct: bool,
    /// Exact p-values for the rank tests; by default exact when n < 50 and
    /// there are no ties (R's rule).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exact: Option<bool>,
}

impl Default for TestRequest {
    fn default() -> Self {
        Self {
            data: DataSet::default(),
            test: TestChoice::Auto,
            y: None,
            group: None,
            y2: None,
            paired: false,
            levels: Vec::new(),
            mu: 0.0,
            table: None,
            alternative: Alternative::TwoSided,
            conf_level: default_conf_level(),
            correct: true,
            exact: None,
        }
    }
}

/// Input of `regress`: ordinary least squares with an intercept.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RegressRequest {
    /// The data.
    pub data: DataSet,
    /// Dependent variable (numeric).
    pub y: String,
    /// Predictors; categorical ones are dummy coded against their first
    /// sorted level.
    pub x: Vec<String>,
    /// Level of the coefficient intervals (default 0.95).
    #[serde(default = "default_conf_level")]
    pub conf_level: f64,
}

fn default_correlation_method() -> CorrelationMethod {
    CorrelationMethod::Pearson
}

fn default_adjustment() -> PAdjust {
    PAdjust::Holm
}

/// Input of `correlate`: a correlation matrix.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CorrelateRequest {
    /// The data.
    pub data: DataSet,
    /// Numeric columns; all numeric columns when empty.
    #[serde(default)]
    pub variables: Vec<String>,
    /// Coefficient (default pearson).
    #[serde(default = "default_correlation_method")]
    pub method: CorrelationMethod,
    /// Adjustment of the p-values for the number of pairs (default holm).
    #[serde(default = "default_adjustment")]
    pub adjustment: PAdjust,
    /// Level of the interval reported for a single pair (default 0.95).
    #[serde(default = "default_conf_level")]
    pub conf_level: f64,
}

/// Any engine request, tagged by `kind`; what an envelope stores to be
/// recomputed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ComputeRequest {
    /// [`ComputePort::describe`].
    Describe(DescribeRequest),
    /// [`ComputePort::test`].
    Test(TestRequest),
    /// [`ComputePort::regress`].
    Regress(RegressRequest),
    /// [`ComputePort::correlate`].
    Correlate(CorrelateRequest),
}

impl ComputeRequest {
    /// The request stored in `envelope.method.params["request"]`.
    ///
    /// # Errors
    ///
    /// [`ComputeError::NotRecomputable`] when it is missing or malformed.
    pub fn from_envelope(envelope: &Computed) -> Result<Self, ComputeError> {
        let value = envelope.method.params.get(REQUEST_PARAM).ok_or_else(|| {
            ComputeError::NotRecomputable(format!("envelope {} has no stored request", envelope.id))
        })?;
        serde_json::from_value(value.clone())
            .map_err(|e| ComputeError::NotRecomputable(format!("stored request: {e}")))
    }

    /// The data the request reads.
    #[must_use]
    pub fn data(&self) -> &DataSet {
        match self {
            Self::Describe(r) => &r.data,
            Self::Test(r) => &r.data,
            Self::Regress(r) => &r.data,
            Self::Correlate(r) => &r.data,
        }
    }

    /// Mutable access to the data, e.g. to reload a changed file before a
    /// recompute.
    pub fn data_mut(&mut self) -> &mut DataSet {
        match self {
            Self::Describe(r) => &mut r.data,
            Self::Test(r) => &mut r.data,
            Self::Regress(r) => &mut r.data,
            Self::Correlate(r) => &mut r.data,
        }
    }
}

/// The statistics engine.
pub trait ComputePort: Send + Sync {
    /// Descriptive statistics per variable (and group).
    ///
    /// # Errors
    ///
    /// [`ComputeError::Invalid`] for unknown columns.
    fn describe(&self, request: &DescribeRequest) -> ComputeResult;

    /// A hypothesis test, chosen by the engine when `test` is `auto`.
    ///
    /// # Errors
    ///
    /// [`ComputeError::Invalid`] when the request does not fit any test
    /// shape.
    fn test(&self, request: &TestRequest) -> ComputeResult;

    /// An OLS regression with intercept.
    ///
    /// # Errors
    ///
    /// [`ComputeError::Invalid`] for unknown or non-numeric columns.
    fn regress(&self, request: &RegressRequest) -> ComputeResult;

    /// A correlation matrix with adjusted p-values.
    ///
    /// # Errors
    ///
    /// [`ComputeError::Invalid`] for unknown or non-numeric columns.
    fn correlate(&self, request: &CorrelateRequest) -> ComputeResult;

    /// Re-runs `envelope` from its stored request. The result keeps the
    /// envelope's id and fills `verification` (time, match, the values
    /// that changed) against `envelope`.
    ///
    /// # Errors
    ///
    /// [`ComputeError::NotRecomputable`] when the envelope has no stored
    /// request this engine runs.
    fn recompute(&self, envelope: &Computed) -> ComputeResult;

    /// Runs any request.
    ///
    /// # Errors
    ///
    /// As the method it dispatches to.
    fn run(&self, request: &ComputeRequest) -> ComputeResult {
        match request {
            ComputeRequest::Describe(r) => self.describe(r),
            ComputeRequest::Test(r) => self.test(r),
            ComputeRequest::Regress(r) => self.regress(r),
            ComputeRequest::Correlate(r) => self.correlate(r),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn requests_read_tool_json_with_defaults() {
        let request: ComputeRequest = serde_json::from_value(json!({
            "kind": "test",
            "data": {"columns": [
                {"name": "mpg", "values": [21.0, null, 22.8]},
                {"name": "am", "values": ["a", "b", null]}
            ]},
            "y": "mpg", "group": "am"
        }))
        .unwrap_or_else(|e| panic!("{e}"));
        let ComputeRequest::Test(test) = &request else {
            panic!("not a test request");
        };
        assert_eq!(test.test, TestChoice::Auto);
        assert_eq!(test.conf_level, 0.95);
        assert!(test.correct);
        assert_eq!(test.data.source.name, "inline");
        assert!(matches!(
            test.data.column("mpg").map(|c| &c.values),
            Some(ColumnValues::Numeric(v)) if v[1].is_none()
        ));
        assert!(matches!(
            test.data.column("am").map(|c| &c.values),
            Some(ColumnValues::Categorical(_))
        ));
        let back: ComputeRequest =
            serde_json::from_value(serde_json::to_value(&request).unwrap_or_default())
                .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(back, request);
    }

    #[test]
    fn from_envelope_needs_a_stored_request() {
        let engine = domain::computed::Engine {
            name: "crest-stats".into(),
            version: "0".into(),
            backend: None,
            platform: None,
        };
        let mut envelope = Computed::new("a", "stats.describe", "Descriptives", engine);
        assert!(matches!(
            ComputeRequest::from_envelope(&envelope),
            Err(ComputeError::NotRecomputable(_))
        ));
        let request = ComputeRequest::Describe(DescribeRequest {
            data: DataSet::inline(vec![DataColumn::numeric("x", &[1.0, 2.0])]),
            variables: Vec::new(),
            by: None,
        });
        envelope.method.params.insert(
            REQUEST_PARAM.into(),
            serde_json::to_value(&request).unwrap_or_default(),
        );
        assert_eq!(ComputeRequest::from_envelope(&envelope), Ok(request));
    }
}
