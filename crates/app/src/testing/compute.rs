//! [`FakeCompute`]: a [`ComputePort`] that answers with small, predictable
//! envelopes, for tests of the statistics tools and the recompute flow
//! without the real engine.
//!
//! Every call returns an envelope of engine `fake` whose id and input hash
//! come from the request's JSON, whose values are the mean and count of
//! each numeric column (`<column>.mean`, `<column>.n`) plus a fixed
//! `statistic` and `p_value`, and which stores the request so
//! [`ComputePort::recompute`] can re-run it. A calc sheet gives every line
//! the value 1 in `m` (keyed by its name, or `line_<n>`); a conversion
//! multiplies by 2.54 into `to`.

use std::collections::BTreeMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::{Mutex, PoisonError};

use domain::computed::*;

use crate::ports::compute::*;

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
                describe_body(request.data().unwrap_or(&DataSet::default())),
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
            ComputeRequest::Calc(r) => ("calc.sheet", "Calc sheet", "calc_sheet", calc_body(r)),
            ComputeRequest::Convert(r) => (
                "calc.convert",
                "Unit conversion",
                "convert",
                convert_body(r),
            ),
            ComputeRequest::Contingency(_) => (
                "stats.contingency",
                "Pearson's χ² test",
                "contingency",
                contingency_body(),
            ),
            ComputeRequest::AbTest(r) => (
                "stats.ab_test",
                "A/B test",
                "ab_test",
                ComputedBody::AbTest(AbTestBody {
                    arms: [&r.control, &r.treatment]
                        .iter()
                        .map(|a| {
                            #[allow(clippy::cast_precision_loss)]
                            let rate = a.conversions as f64 / a.visitors.max(1) as f64;
                            AbArm {
                                name: a.name.clone().unwrap_or_default(),
                                visitors: a.visitors,
                                conversions: a.conversions,
                                rate,
                                rate_lo: rate,
                                rate_hi: rate,
                            }
                        })
                        .collect(),
                    expected_split: r.expected_split,
                    conf_level: r.conf_level,
                    method: AbMethod::ZTest,
                    looks: r.looks,
                    sample_ratio_mismatch: false,
                }),
            ),
            ComputeRequest::Bootstrap(r) => (
                "stats.bootstrap",
                "Bootstrap",
                "bootstrap",
                ComputedBody::Bootstrap(BootstrapBody {
                    statistic: r.statistic,
                    variable: r.y.clone(),
                    versus: None,
                    observed: 1.0,
                    resamples: r.resamples,
                    conf_level: r.conf_level,
                    histogram: Histogram {
                        edges: vec![0.0, 1.0, 2.0],
                        counts: vec![1, 1],
                    },
                    intervals: Vec::new(),
                    z0: None,
                    acceleration: None,
                }),
            ),
            ComputeRequest::Forecast(r) => (
                "stats.forecast",
                "Forecast",
                "forecast",
                ComputedBody::Forecast(ForecastBody {
                    variable: r.y.clone(),
                    time: Vec::new(),
                    model: "fake".into(),
                    values: vec![1.0, 2.0],
                    trend: Vec::new(),
                    seasonal: Vec::new(),
                    remainder: Vec::new(),
                    forecast: vec![3.0],
                    lo80: vec![2.5],
                    hi80: vec![3.5],
                    lo95: vec![2.0],
                    hi95: vec![4.0],
                    holdout: None,
                }),
            ),
            ComputeRequest::Optimize(r) => (
                "solve.lp",
                "Linear program",
                "optimization",
                ComputedBody::Optimization(OptimizationBody {
                    sense: r.sense,
                    objective_expr: r.objective.clone(),
                    status: OptimizationStatus::Optimal,
                    objective: Some(36.0),
                    integer: false,
                    mip_gap: None,
                    variables: r
                        .variables
                        .iter()
                        .map(|v| OptVariable {
                            name: v.name.clone(),
                            value: Some(1.0),
                            lower: v.lower,
                            upper: v.upper,
                            integer: v.integer,
                            objective_coefficient: 1.0,
                            reduced_cost: Some(0.0),
                        })
                        .collect(),
                    constraints: Vec::new(),
                }),
            ),
            ComputeRequest::Solve(r) => (
                "solve.equations",
                "Equation solver",
                "equations",
                ComputedBody::Equations(EquationsBody {
                    method: SolveMethod::Newton,
                    converged: true,
                    unknowns: r
                        .unknowns
                        .iter()
                        .map(|u| SolvedUnknown {
                            name: u.name.clone(),
                            value: 1.0,
                        })
                        .collect(),
                    roots: Vec::new(),
                    plug_back: Vec::new(),
                    residual_norm: 0.0,
                    iterations: 1,
                    tolerance: r.tolerance,
                    condition_number: None,
                    curve: None,
                }),
            ),
            ComputeRequest::Simulate(r) => (
                "solve.monte_carlo",
                "Monte Carlo simulation",
                "simulation",
                ComputedBody::Simulation(SimulationBody {
                    output: r.output.clone().unwrap_or_else(|| "result".into()),
                    unit: r.unit.clone(),
                    model: r.model.clone().unwrap_or_default(),
                    fermi: r.model.is_none(),
                    samples: r.samples,
                    inputs: Vec::new(),
                    histogram: Histogram {
                        edges: vec![0.0, 1.0],
                        counts: vec![1],
                    },
                    convergence: Curve::default(),
                    convergence_mcse: Vec::new(),
                    base: 1.0,
                    tornado: Vec::new(),
                }),
            ),
            ComputeRequest::CashFlow(r) => (
                "solve.cash_flow",
                "Cash flow",
                "cash_flow",
                ComputedBody::CashFlow(CashFlowBody {
                    rate: r.rate,
                    finance_rate: r.finance_rate.unwrap_or(r.rate),
                    reinvest_rate: r.reinvest_rate.unwrap_or(r.rate),
                    currency: r.currency.clone(),
                    period_label: r.period.clone(),
                    rows: Vec::new(),
                    irrs: Vec::new(),
                    sign_changes: 1,
                    npv_curve: Curve::default(),
                }),
            ),
            ComputeRequest::Bode(r) => (
                "solve.bode",
                "Frequency response",
                "bode",
                ComputedBody::Bode(BodeBody {
                    numerator: r.numerator.clone(),
                    denominator: r.denominator.clone(),
                    delay: r.delay,
                    unit: r.unit,
                    points: vec![BodePoint {
                        w: 1.0,
                        magnitude_db: -3.0,
                        phase_deg: -45.0,
                        re: 0.5,
                        im: -0.5,
                    }],
                    zeros: Vec::new(),
                    poles: vec![Complex { re: -1.0, im: 0.0 }],
                    gain_crossovers: Vec::new(),
                    phase_crossovers: Vec::new(),
                    closed_loop_poles: vec![Complex { re: -2.0, im: 0.0 }],
                }),
            ),
            ComputeRequest::Decide(r) => (
                "solve.decision_matrix",
                "Decision matrix",
                "decision_matrix",
                ComputedBody::DecisionMatrix(DecisionMatrixBody {
                    normalization: r.normalization,
                    criteria: r
                        .criteria
                        .iter()
                        .map(|c| DecisionCriterion {
                            name: c.name.clone(),
                            raw_weight: c.weight,
                            weight: c.weight,
                            direction: c.direction,
                        })
                        .collect(),
                    options: r
                        .options
                        .iter()
                        .map(|o| DecisionOption {
                            name: o.name.clone(),
                            scores: o.scores.clone(),
                            normalized: o.scores.clone(),
                            total: 0.0,
                            rank: 1,
                        })
                        .collect(),
                    sensitivity: Vec::new(),
                }),
            ),
            ComputeRequest::Power(r) => (
                "stats.power",
                "Power analysis",
                "power",
                ComputedBody::Power(PowerBody {
                    test: r.test,
                    solved_for: PowerTarget::N,
                    alternative: r.alternative,
                    n: 25.5,
                    effect: r.effect.unwrap_or(0.8),
                    alpha: r.alpha.unwrap_or(0.05),
                    power: r.power.unwrap_or(0.8),
                    groups: r.groups,
                    curve: Curve::default(),
                }),
            ),
            ComputeRequest::Distribution(r) => (
                "stats.distribution",
                "Distribution",
                "distribution",
                distribution_body(&r.variable),
            ),
        };
        let engine = Engine {
            name: FAKE_ENGINE.into(),
            version: "0".into(),
            backend: None,
            platform: None,
        };
        let mut computed = Computed::new(id, kind, name, engine);
        match (request.data(), &body) {
            (Some(data), _) => {
                computed.values = mean_and_n(data);
                computed.values.insert("statistic".into(), shown(2.5, 2));
                computed.values.insert("df".into(), shown(10.0, 0));
                computed.values.insert("p_value".into(), shown(0.03, 3));
                let data_json = serde_json::to_vec(&data.columns).unwrap_or_default();
                let mut input = data.source.clone();
                input.hash = Some(format!("fake:{}", hex(&data_json)));
                computed.inputs.push(input);
            }
            (None, ComputedBody::CalcSheet(sheet)) => {
                for (i, line) in sheet.lines.iter().enumerate() {
                    let key = line
                        .name
                        .clone()
                        .unwrap_or_else(|| format!("line_{}", i + 1));
                    let mut value = shown(1.0, 0);
                    value.unit = line.unit.clone();
                    computed.values.insert(key, value);
                }
            }
            (None, ComputedBody::Quantity(quantity)) => {
                let mut value = shown(quantity.to.value, 2);
                value.unit = Some(quantity.to.unit.clone());
                computed.values.insert("result".into(), value);
                computed
                    .values
                    .insert("factor".into(), shown(quantity.factor, 2));
            }
            (None, _) => {}
        }
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

fn contingency_body() -> ComputedBody {
    let square = |a: f64, b: f64| vec![vec![a, b], vec![b, a]];
    ComputedBody::Contingency(ContingencyBody {
        row_variable: None,
        column_variable: None,
        row_levels: vec!["R1".into(), "R2".into()],
        column_levels: vec!["C1".into(), "C2".into()],
        observed: square(3.0, 1.0),
        expected: square(2.0, 2.0),
        row_percent: square(75.0, 25.0),
        column_percent: square(75.0, 25.0),
        std_residuals: square(2.5, -2.5),
        test: ContingencyTest::ChiSquare,
    })
}

fn distribution_body(variable: &str) -> ComputedBody {
    ComputedBody::Distribution(DistributionBody {
        variable: variable.to_owned(),
        unit: None,
        by: None,
        bin_rule: BinRule::FreedmanDiaconis,
        bin_width: 1.0,
        bandwidth_rule: BandwidthRule::Silverman,
        conf_level: 0.95,
        groups: vec![DistributionGroup {
            name: None,
            n: 2,
            histogram: Histogram {
                edges: vec![0.0, 1.0],
                counts: vec![2],
            },
            kde: Curve::default(),
            bandwidth: 0.5,
            boxplot: BoxSummary {
                lower_hinge: 0.25,
                median: 0.5,
                upper_hinge: 0.75,
                whisker_lo: 0.0,
                whisker_hi: 1.0,
                outliers: Vec::new(),
            },
            qq: Vec::new(),
            ecdf: Vec::new(),
            dkw_epsilon: 0.9,
        }],
    })
}

fn calc_body(request: &CalcRequest) -> ComputedBody {
    let lines = request
        .lines
        .iter()
        .map(|line| {
            let (name, expr) = match line.split_once('=') {
                Some((name, expr)) => (Some(name.trim().to_owned()), expr.trim().to_owned()),
                None => (None, line.trim().to_owned()),
            };
            CalcLine {
                name,
                expr,
                value: Some(1.0),
                display: Some("1 m".into()),
                unit: Some("m".into()),
                latex: None,
                dimension: Some("Length".into()),
                error: None,
                notes: Vec::new(),
            }
        })
        .collect();
    ComputedBody::CalcSheet(CalcSheetBody { lines })
}

fn convert_body(request: &ConvertRequest) -> ComputedBody {
    let value = request.value.unwrap_or(1.0);
    ComputedBody::Quantity(QuantityBody {
        from: Quantity {
            value,
            unit: request.from.clone(),
            uncertainty: request.uncertainty,
        },
        to: Quantity {
            value: value * 2.54,
            unit: request.to.clone(),
            uncertainty: request.uncertainty.map(|u| u * 2.54),
        },
        factor: 2.54,
        exact: true,
        dimension: "Length".into(),
        chain: Vec::new(),
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

    fn calc(&self, request: &CalcRequest) -> ComputeResult {
        if request.lines.is_empty() {
            return Err(ComputeError::Invalid("the sheet has no lines".into()));
        }
        self.answer(&ComputeRequest::Calc(request.clone()), None)
    }

    fn convert(&self, request: &ConvertRequest) -> ComputeResult {
        self.answer(&ComputeRequest::Convert(request.clone()), None)
    }

    fn distribution(&self, request: &DistributionRequest) -> ComputeResult {
        self.answer(&ComputeRequest::Distribution(request.clone()), None)
    }

    fn contingency(&self, request: &ContingencyRequest) -> ComputeResult {
        self.answer(&ComputeRequest::Contingency(request.clone()), None)
    }

    fn power(&self, request: &PowerRequest) -> ComputeResult {
        self.answer(&ComputeRequest::Power(request.clone()), None)
    }

    fn ab_test(&self, request: &AbTestRequest) -> ComputeResult {
        self.answer(&ComputeRequest::AbTest(request.clone()), None)
    }

    fn bootstrap(&self, request: &BootstrapRequest) -> ComputeResult {
        self.answer(&ComputeRequest::Bootstrap(request.clone()), None)
    }

    fn forecast(&self, request: &ForecastRequest) -> ComputeResult {
        self.answer(&ComputeRequest::Forecast(request.clone()), None)
    }

    fn optimize(&self, request: &OptimizeRequest) -> ComputeResult {
        self.answer(&ComputeRequest::Optimize(request.clone()), None)
    }

    fn solve(&self, request: &SolveRequest) -> ComputeResult {
        self.answer(&ComputeRequest::Solve(request.clone()), None)
    }

    fn simulate(&self, request: &SimulateRequest) -> ComputeResult {
        self.answer(&ComputeRequest::Simulate(request.clone()), None)
    }

    fn cash_flow(&self, request: &CashFlowRequest) -> ComputeResult {
        self.answer(&ComputeRequest::CashFlow(request.clone()), None)
    }

    fn decide(&self, request: &DecideRequest) -> ComputeResult {
        self.answer(&ComputeRequest::Decide(request.clone()), None)
    }

    fn bode(&self, request: &BodeRequest) -> ComputeResult {
        self.answer(&ComputeRequest::Bode(request.clone()), None)
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
