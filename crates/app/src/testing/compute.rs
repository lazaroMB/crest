//! [`FakeCompute`]: a [`ComputePort`] that answers with small, predictable
//! envelopes, for tests of the statistics tools and the recompute flow
//! without the real engine.
//!
//! Every call returns an envelope of engine `fake` whose id and input hash
//! come from the request's JSON, whose values are the mean and count of
//! each numeric column (`<column>.mean`, `<column>.n`) plus a fixed
//! `statistic` and `p_value`, and which stores the request so
//! [`ComputePort::recompute`] can re-run it.

use std::collections::BTreeMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::{Mutex, PoisonError};

use domain::computed::{
    Alternative, Computed, ComputedBody, ComputedValue, CorrelationBody, CorrelationMethod,
    DescriptiveBody, Diagnostics, Engine, FitStats, Narrative, PAdjust, RegressionBody, TestBody,
    TestKind, VariableKind, VariableSummary,
};

use crate::ports::compute::{
    ColumnValues, ComputeError, ComputePort, ComputeRequest, ComputeResult, CorrelateRequest,
    DataSet, DescribeRequest, REQUEST_PARAM, RegressRequest, TestRequest,
};

/// Name of the fake engine.
pub const FAKE_ENGINE: &str = "fake";

/// A scripted compute engine.
#[derive(Debug, Default)]
pub struct FakeCompute {
    requests: Mutex<Vec<ComputeRequest>>,
    error: Mutex<Option<ComputeError>>,
}

fn hex(bytes: &[u8]) -> String {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn mean_and_n(data: &DataSet) -> BTreeMap<String, ComputedValue> {
    let mut values = BTreeMap::new();
    for column in &data.columns {
        if let ColumnValues::Numeric(cells) = &column.values {
            let present: Vec<f64> = cells.iter().flatten().copied().collect();
            if present.is_empty() {
                continue;
            }
            #[allow(clippy::cast_precision_loss)]
            let n = present.len() as f64;
            let mean = present.iter().sum::<f64>() / n;
            values.insert(format!("{}.mean", column.name), shown(mean, 2));
            values.insert(format!("{}.n", column.name), shown(n, 0));
        }
    }
    values
}

fn shown(v: f64, decimals: u32) -> ComputedValue {
    ComputedValue {
        display: Some(domain::computed::apa::number(v, decimals)),
        decimals: Some(decimals),
        ..ComputedValue::new(v)
    }
}

impl FakeCompute {
    /// An engine that answers every call.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Makes every later call fail with `error`.
    #[must_use]
    pub fn failing(self, error: ComputeError) -> Self {
        *self.error.lock().unwrap_or_else(PoisonError::into_inner) = Some(error);
        self
    }

    /// The requests received, in order (recomputes included).
    pub fn requests(&self) -> Vec<ComputeRequest> {
        self.requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn answer(&self, request: &ComputeRequest, id: Option<String>) -> ComputeResult {
        self.requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(request.clone());
        if let Some(error) = self
            .error
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
        {
            return Err(error);
        }
        let json = serde_json::to_vec(request).unwrap_or_default();
        let id = id.unwrap_or_else(|| format!("st_{}", &hex(&json)[..12]));
        let (kind, name, template, body) = match request {
            ComputeRequest::Describe(_) => (
                "stats.describe",
                "Descriptive statistics",
                "describe",
                describe_body(request.data()),
            ),
            ComputeRequest::Test(_) => (
                "stats.ttest_welch",
                "Welch two-sample t-test",
                "apa_ttest",
                ComputedBody::Test(TestBody {
                    test: TestKind::WelchT,
                    alternative: Alternative::TwoSided,
                    groups: Vec::new(),
                    observed: None,
                    expected: None,
                }),
            ),
            ComputeRequest::Regress(r) => (
                "stats.ols",
                "Ordinary least squares",
                "ols",
                regression_body(&r.y),
            ),
            ComputeRequest::Correlate(_) => (
                "stats.correlation_pearson",
                "Pearson correlation",
                "correlation_matrix",
                correlation_body(),
            ),
        };
        let engine = Engine {
            name: FAKE_ENGINE.into(),
            version: "0".into(),
            backend: None,
            platform: None,
        };
        let mut computed = Computed::new(id, kind, name, engine);
        computed.values = mean_and_n(request.data());
        computed.values.insert("statistic".into(), shown(2.5, 2));
        computed.values.insert("df".into(), shown(10.0, 0));
        computed.values.insert("p_value".into(), shown(0.03, 3));
        let data_json = serde_json::to_vec(&request.data().columns).unwrap_or_default();
        let mut input = request.data().source.clone();
        input.hash = Some(format!("fake:{}", hex(&data_json)));
        computed.inputs.push(input);
        computed.narrative = Some(Narrative {
            template: template.into(),
            text_slots: vec!["statistic".into(), "p_value".into()],
        });
        computed.reproduce.lua = Some("app.compute.fake{}".into());
        computed.method.params.insert(
            REQUEST_PARAM.into(),
            serde_json::to_value(request).unwrap_or_default(),
        );
        Ok((computed, body))
    }
}

fn describe_body(data: &DataSet) -> ComputedBody {
    let variables = data
        .columns
        .iter()
        .map(|c| VariableSummary {
            name: c.name.clone(),
            group: None,
            kind: VariableKind::Numeric,
            unit: None,
            n: c.len() as u64,
            missing: 0,
            mean: None,
            sd: None,
            median: None,
            q1: None,
            q3: None,
            min: None,
            max: None,
            skew: None,
            kurtosis: None,
            histogram: None,
            levels: Vec::new(),
        })
        .collect();
    ComputedBody::Descriptive(DescriptiveBody {
        by: None,
        variables,
    })
}

fn regression_body(y: &str) -> ComputedBody {
    ComputedBody::Regression(RegressionBody {
        dependent: y.to_owned(),
        ci_level: 0.95,
        coefficients: Vec::new(),
        fit: FitStats {
            n: 0,
            r_squared: 0.5,
            adj_r_squared: 0.4,
            f: 2.5,
            df_model: 1.0,
            df_resid: 10.0,
            f_p: 0.03,
            sigma: None,
            aic: None,
            bic: None,
        },
        diagnostics: Diagnostics::default(),
        fitted: Vec::new(),
        residuals: Vec::new(),
    })
}

fn correlation_body() -> ComputedBody {
    ComputedBody::Correlation(CorrelationBody {
        method: CorrelationMethod::Pearson,
        adjustment: PAdjust::Holm,
        variables: Vec::new(),
        r: Vec::new(),
        p: Vec::new(),
        n: Vec::new(),
    })
}

impl ComputePort for FakeCompute {
    fn describe(&self, request: &DescribeRequest) -> ComputeResult {
        self.answer(&ComputeRequest::Describe(request.clone()), None)
    }

    fn test(&self, request: &TestRequest) -> ComputeResult {
        self.answer(&ComputeRequest::Test(request.clone()), None)
    }

    fn regress(&self, request: &RegressRequest) -> ComputeResult {
        self.answer(&ComputeRequest::Regress(request.clone()), None)
    }

    fn correlate(&self, request: &CorrelateRequest) -> ComputeResult {
        self.answer(&ComputeRequest::Correlate(request.clone()), None)
    }

    fn recompute(&self, envelope: &Computed) -> ComputeResult {
        let request = ComputeRequest::from_envelope(envelope)?;
        let (mut computed, body) = self.answer(&request, Some(envelope.id.clone()))?;
        let diff = envelope.diff_values(&computed, 0.0);
        computed.verification.recomputed_at = Some("2026-10-24T00:00:00Z".into());
        computed.verification.matched = Some(diff.is_empty());
        computed.verification.diff = diff;
        Ok((computed, body))
    }
}
