//! The phase-2 compute tools: `stats_distribution`, `stats_contingency`,
//! `stats_power`, `stats_ab_test`, `stats_bootstrap`, `stats_forecast`,
//! `solve_lp`, `solve_equations`, `simulate`, and what the model is
//! told about each card beyond its slots ([`notes`]).
//!
//! Tools that read data are [`StatsKind`]s (their `data` is a
//! [`super::DataInput`]); tools that take only parameters are
//! [`UnitsKind`]s with a check before the engine runs.

use std::fmt::Write as _;

use domain::computed::ComputedBody;

use super::{StatsKind, StatsTool, UnitsKind, UnitsTool};
use crate::ports::compute::*;

/// `stats_distribution`.
#[derive(Debug, Clone, Copy)]
pub struct Distribution;

impl StatsKind for Distribution {
    type Request = DistributionRequest;
    const NAME: &'static str = "stats_distribution";
    const DESCRIPTION: &'static str = "\
Shows the distribution of one numeric variable with crest's deterministic engine: a histogram \
(binning rule and width stated), a Gaussian kernel density (bandwidth rule stated), box and \
violin plots with 1.5×IQR outliers, a normal QQ plot with a pointwise band and the ECDF with \
its DKW band; one per group of `by`. The user sees the plots; you get n, mean, SD, quartiles, \
outlier count, bin width and bandwidth as slots to cite.";
    fn wrap(request: DistributionRequest) -> ComputeRequest {
        ComputeRequest::Distribution(request)
    }
    fn subject(request: &DistributionRequest) -> String {
        match &request.by {
            Some(by) => format!("{} by {by}", request.variable),
            None => request.variable.clone(),
        }
    }
}

/// `stats_distribution`.
pub type StatsDistributionTool = StatsTool<Distribution>;

/// `stats_contingency`.
#[derive(Debug, Clone, Copy)]
pub struct Contingency;

impl StatsKind for Contingency {
    type Request = ContingencyRequest;
    const NAME: &'static str = "stats_contingency";
    const DESCRIPTION: &'static str = "\
Analyses a two-way contingency table with crest's deterministic engine: observed and expected \
counts, row and column percentages, adjusted standardised residuals (a heatmap) and a test of \
independence. With `test: auto` it runs Fisher's exact test when an expected count is below 5 \
(any r×c table; simulated p-value with a seed when too large to enumerate) and Pearson's χ² \
otherwise, and says why; Cramér's V, and on 2×2 tables the odds ratio with its interval. Give \
two categorical columns `row` and `column`, or the counts in `table`. The user sees the card; \
you get the values as slots to cite.";
    fn wrap(request: ContingencyRequest) -> ComputeRequest {
        ComputeRequest::Contingency(request)
    }
    fn needs_data(request: &ContingencyRequest) -> bool {
        request.table.is_none()
    }
    fn subject(request: &ContingencyRequest) -> String {
        match (&request.row, &request.column) {
            (Some(r), Some(c)) => format!("{r} × {c}"),
            _ => match &request.table {
                Some(t) => format!("{}×{} table", t.len(), t.first().map_or(0, Vec::len)),
                None => "table".into(),
            },
        }
    }
}

/// `stats_contingency`.
pub type StatsContingencyTool = StatsTool<Contingency>;

/// `stats_bootstrap`.
#[derive(Debug, Clone, Copy)]
pub struct Bootstrap;

impl StatsKind for Bootstrap {
    type Request = BootstrapRequest;
    const NAME: &'static str = "stats_bootstrap";
    const DESCRIPTION: &'static str = "\
Bootstraps one statistic with crest's deterministic engine: B resamples (default 2000) from a \
seeded generator, with percentile, BCa, basic and normal intervals, the bootstrap SE and bias, \
and the Monte Carlo error of the endpoints; the same seed gives the same numbers. Statistics: \
mean, median, sd, trimmed_mean, mean_difference and median_difference (with a two-level \
`group`), correlation (with `y2`). The user sees the replicate histogram and the intervals; \
you get the values as slots to cite.";
    fn wrap(request: BootstrapRequest) -> ComputeRequest {
        ComputeRequest::Bootstrap(request)
    }
    fn subject(request: &BootstrapRequest) -> String {
        let what = request.statistic.label();
        match (&request.group, &request.y2) {
            (Some(g), _) => format!("{what} of {} by {g}", request.y),
            (None, Some(y2)) => format!("{what} of {} and {y2}", request.y),
            _ => format!("{what} of {}", request.y),
        }
    }
}

/// `stats_bootstrap`.
pub type StatsBootstrapTool = StatsTool<Bootstrap>;

/// `stats_forecast`.
#[derive(Debug, Clone, Copy)]
pub struct ForecastKind;

impl StatsKind for ForecastKind {
    type Request = ForecastRequest;
    const NAME: &'static str = "stats_forecast";
    const DESCRIPTION: &'static str = "\
Decomposes and forecasts a time series with crest's deterministic engine (augurs): MSTL splits \
it into trend, one seasonal component per period in `periods` (e.g. [12] for monthly data) and \
remainder; AutoETS forecasts `horizon` steps with 80% and 95% intervals. The last `holdout` \
observations are forecast by a model fitted without them, for an honest MAPE/sMAPE/MAE. Rows \
must be equally spaced time steps in order, without gaps. The user sees the fan chart and the \
components; you get the values as slots to cite.";
    fn wrap(request: ForecastRequest) -> ComputeRequest {
        ComputeRequest::Forecast(request)
    }
    fn subject(request: &ForecastRequest) -> String {
        format!("{}, {} steps", request.y, request.horizon)
    }
}

/// `stats_forecast`.
pub type StatsForecastTool = StatsTool<ForecastKind>;

/// `solve_lp`.
#[derive(Debug, Clone, Copy)]
pub struct Lp;

impl UnitsKind for Lp {
    type Request = OptimizeRequest;
    const NAME: &'static str = "solve_lp";
    const DESCRIPTION: &'static str = "\
Solves a linear program, or a mixed-integer one, with crest's deterministic solver (microlp): \
`objective` and each constraint's `expr` are linear expressions of the named `variables` \
(`3*x + 5*y`, `2x + y <= 100`, `a + b = 1`); variables default to ≥ 0 and may be `integer` or \
`binary`. The user sees the status, the variable values with reduced costs, and each \
constraint's slack, whether it binds and its shadow price (dual value); you get them as slots \
to cite. Infeasible and unbounded problems are reported as such.";
    fn wrap(request: OptimizeRequest) -> ComputeRequest {
        ComputeRequest::Optimize(request)
    }
    fn subject(request: &OptimizeRequest) -> String {
        let sense = match request.sense {
            domain::computed::Sense::Maximize => "max",
            domain::computed::Sense::Minimize => "min",
        };
        format!(
            "{sense} {} · {} variables, {} constraints",
            request.objective,
            request.variables.len(),
            request.constraints.len()
        )
    }
    fn check(request: &OptimizeRequest) -> Result<(), String> {
        if request.variables.is_empty() {
            return Err("give at least one variable".into());
        }
        if request.objective.trim().is_empty() {
            return Err("`objective` is empty".into());
        }
        Ok(())
    }
}

/// `solve_lp`.
pub type SolveLpTool = UnitsTool<Lp>;

/// `solve_equations`.
#[derive(Debug, Clone, Copy)]
pub struct Equations;

impl UnitsKind for Equations {
    type Request = SolveRequest;
    const NAME: &'static str = "solve_equations";
    const DESCRIPTION: &'static str = "\
Solves an equation or a small square system numerically with crest's deterministic engine. \
Equations are `lhs = rhs` in calculator notation (`x^2 + y^2 = 4`, `cos(x) = x`, `2x - y = 1`) \
over the named `unknowns`. With one unknown and a `range` it finds every root crossing zero \
in the range; otherwise damped Newton starts from each unknown's `guess`. The user sees the \
solution, each equation plugged back (both sides and their difference), iterations and the \
condition number; you get the values as slots to cite.";
    fn wrap(request: SolveRequest) -> ComputeRequest {
        ComputeRequest::Solve(request)
    }
    fn subject(request: &SolveRequest) -> String {
        match request.equations.as_slice() {
            [one] => one.clone(),
            many => format!("{} equations", many.len()),
        }
    }
    fn check(request: &SolveRequest) -> Result<(), String> {
        if request.equations.is_empty() || request.unknowns.is_empty() {
            return Err("give the equations and their unknowns".into());
        }
        if request.equations.len() != request.unknowns.len() {
            return Err("give as many equations as unknowns".into());
        }
        Ok(())
    }
}

/// `solve_equations`.
pub type SolveEquationsTool = UnitsTool<Equations>;

/// `simulate`.
#[derive(Debug, Clone, Copy)]
pub struct Simulate;

impl UnitsKind for Simulate {
    type Request = SimulateRequest;
    const NAME: &'static str = "simulate";
    const DESCRIPTION: &'static str = "\
Runs a seeded Monte Carlo simulation with crest's deterministic engine: each input has a \
distribution (`fixed`, `uniform`, `normal`, `lognormal` from a 90% range low–high, \
`triangular`, `pert`) and the output is the `model` expression of the inputs (`price * volume \
- cost`). The user sees mean ± MCSE, the 5th/50th/95th percentiles, the histogram, the \
convergence trace and a tornado of each input's swing from its 10th to 90th percentile. \
Without `model` it is a Fermi estimate: the product of the inputs, each a lognormal range with \
its `source`, with exact percentiles. You get the values as slots to cite.";
    fn wrap(request: SimulateRequest) -> ComputeRequest {
        ComputeRequest::Simulate(request)
    }
    fn subject(request: &SimulateRequest) -> String {
        match &request.model {
            Some(model) => model.clone(),
            None => format!("Fermi estimate of {} factors", request.inputs.len()),
        }
    }
    fn check(request: &SimulateRequest) -> Result<(), String> {
        if request.inputs.is_empty() {
            return Err("give at least one input".into());
        }
        Ok(())
    }
}

/// `simulate`.
pub type SimulateTool = UnitsTool<Simulate>;

/// `stats_power`.
#[derive(Debug, Clone, Copy)]
pub struct Power;

impl UnitsKind for Power {
    type Request = PowerRequest;
    const NAME: &'static str = "stats_power";
    const DESCRIPTION: &'static str = "\
Power and sample-size calculator with crest's deterministic engine (the formulas of R's pwr: \
noncentral t for t-tests, noncentral F for one-way ANOVA, arcsine h for proportions). Give \
three of `n`, `effect` (Cohen's d, f or h; or `p1` and `p2` for proportions), `power` and \
`alpha` (default .05); the fourth is solved for. Designs: two_sample_t, one_sample_t, \
paired_t, anova (with `groups`), two_proportions, one_proportion. The user sees the \
quantities and the power curve; you get them as slots to cite.";
    fn wrap(request: PowerRequest) -> ComputeRequest {
        ComputeRequest::Power(request)
    }
    fn subject(request: &PowerRequest) -> String {
        request.test.label().to_owned()
    }
    fn check(request: &PowerRequest) -> Result<(), String> {
        let finite = [
            request.n,
            request.effect,
            request.p1,
            request.p2,
            request.alpha,
            request.power,
        ]
        .into_iter()
        .flatten()
        .all(f64::is_finite);
        if finite {
            Ok(())
        } else {
            Err("every number must be finite".into())
        }
    }
}

/// `stats_power`.
pub type StatsPowerTool = UnitsTool<Power>;

/// `stats_ab_test`.
#[derive(Debug, Clone, Copy)]
pub struct AbTest;

impl UnitsKind for AbTest {
    type Request = AbTestRequest;
    const NAME: &'static str = "stats_ab_test";
    const DESCRIPTION: &'static str = "\
Analyses an A/B test from its counts with crest's deterministic engine: each arm's conversion \
rate with a Wilson interval, the absolute lift (Newcombe interval) and relative lift, the \
two-proportion z-test (Fisher's exact test for small counts), P(B > A) under uniform priors, \
the minimum detectable lift, a sample-ratio-mismatch check against `expected_split`, and a \
warning when the results were looked at several times (`looks`). The user sees the card; you \
get the values as slots to cite.";
    fn wrap(request: AbTestRequest) -> ComputeRequest {
        ComputeRequest::AbTest(request)
    }
    fn subject(request: &AbTestRequest) -> String {
        let name = |a: &crate::ports::compute::ArmCounts, d: &str| {
            a.name.clone().unwrap_or_else(|| d.to_owned())
        };
        format!(
            "{} vs {}",
            name(&request.treatment, "B"),
            name(&request.control, "A")
        )
    }
    fn check(request: &AbTestRequest) -> Result<(), String> {
        for (arm, which) in [
            (&request.control, "control"),
            (&request.treatment, "treatment"),
        ] {
            if arm.visitors == 0 || arm.conversions > arm.visitors {
                return Err(format!(
                    "the {which} arm needs visitors > 0 and conversions ≤ visitors"
                ));
            }
        }
        if request.minimum_lift.is_some_and(|l| !l.is_finite()) {
            return Err("`minimum_lift` must be a finite number".into());
        }
        Ok(())
    }
}

/// `stats_ab_test`.
pub type StatsAbTestTool = UnitsTool<AbTest>;

/// Lines for the model about a phase-2 card that the slots do not say.
pub(super) fn notes(body: &ComputedBody, out: &mut String) {
    match body {
        ComputedBody::Distribution(d) if d.groups.len() > 1 => {
            let names: Vec<&str> = d.groups.iter().filter_map(|g| g.name.as_deref()).collect();
            let _ = writeln!(
                out,
                "Groups of {}: {} (value keys are prefixed `<group>.`).",
                d.by.as_deref().unwrap_or("by"),
                names.join(", ")
            );
        }
        ComputedBody::Contingency(t) => {
            let cells: Vec<String> = t
                .row_levels
                .iter()
                .enumerate()
                .flat_map(|(i, row)| {
                    t.column_levels
                        .iter()
                        .enumerate()
                        .filter_map(move |(j, col)| {
                            let r = t.std_residuals[i][j];
                            (r.abs() > 2.0).then(|| {
                                format!("{row} × {col} ({})", domain::computed::apa::number(r, 2))
                            })
                        })
                })
                .collect();
            if cells.is_empty() {
                let _ = writeln!(out, "No cell has an adjusted residual beyond ±2.");
            } else {
                let _ = writeln!(
                    out,
                    "Cells beyond ±2 adjusted residuals (positive: more than independence \
                     predicts): {}.",
                    cells.join(", ")
                );
            }
        }
        ComputedBody::Optimization(o) => {
            let _ = writeln!(
                out,
                "Status: {}. Shadow prices are the change of the objective per unit increase of \
                 a constraint's right-hand side; ranging is not available.",
                o.status.label()
            );
        }
        ComputedBody::Simulation(m) if !m.tornado.is_empty() => {
            let order: Vec<&str> = m.tornado.iter().map(|b| b.input.as_str()).collect();
            let _ = writeln!(
                out,
                "Tornado order (largest swing first): {}.",
                order.join(", ")
            );
        }
        ComputedBody::Forecast(f) => {
            let _ = writeln!(
                out,
                "Model: {}; the forecast covers t = {} to {} (t counts rows from 0).",
                f.model,
                f.values.len(),
                f.values.len() + f.forecast.len().saturating_sub(1)
            );
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use domain::block::BlockKind;
    use domain::conversation::{SessionId, ToolUseId, TurnId};
    use serde_json::json;

    use super::*;
    use crate::testing::compute::FakeCompute;
    use crate::testing::fs::{InMemoryFs, test_root};
    use crate::tools::compute::ComputeToolPorts;
    use crate::tools::registry::{Tool as _, ToolContext, Typed};
    use crate::usecases::compute::inline_offload;

    fn ports(engine: Arc<FakeCompute>) -> ComputeToolPorts {
        let fs = InMemoryFs::new(test_root()).with_file("data/cars.csv", "mpg,am\n21,1\n22.8,0\n");
        ComputeToolPorts {
            engine,
            fs: Arc::new(fs),
            offload: inline_offload(),
        }
    }

    fn ctx() -> ToolContext {
        ToolContext::new(SessionId::new(), TurnId::new(), ToolUseId::new("toolu_c"))
    }

    #[tokio::test]
    async fn a_distribution_reads_its_file_and_shows_a_card() {
        let engine = Arc::new(FakeCompute::new());
        let tool = Typed(StatsDistributionTool::new(ports(Arc::clone(&engine))));
        let input = json!({ "data": { "path": "data/cars.csv" }, "variable": "mpg", "by": "am" });
        let prepared = tool.prepare(input, &ctx()).await.unwrap();
        assert_eq!(prepared.subject, "mpg by am · data/cars.csv");
        let out = prepared.run.await.unwrap();
        assert!(!out.is_error, "{}", out.text_content());
        assert!(matches!(
            out.block_update,
            Some(BlockKind::Computed { ref body, .. }) if matches!(**body, ComputedBody::Distribution(_))
        ));
        let ComputeRequest::Distribution(request) = &engine.requests()[0] else {
            panic!("not a distribution request");
        };
        assert_eq!(request.data.source.path.as_deref(), Some("data/cars.csv"));
        let _ = tool.name();
    }

    #[tokio::test]
    async fn a_contingency_table_needs_no_data_when_counts_are_given() {
        let engine = Arc::new(FakeCompute::new());
        let tool = Typed(StatsContingencyTool::new(ports(Arc::clone(&engine))));
        let prepared = tool
            .prepare(json!({ "table": [[3, 1], [1, 3]] }), &ctx())
            .await
            .unwrap();
        assert_eq!(prepared.subject, "2×2 table");
        let out = prepared.run.await.unwrap();
        assert!(!out.is_error, "{}", out.text_content());
        assert!(
            out.text_content().contains("Cells beyond ±2"),
            "{}",
            out.text_content()
        );
        let without = tool
            .prepare(json!({ "row": "a", "column": "b" }), &ctx())
            .await;
        assert!(without.is_err(), "columns without data are refused");
        assert_eq!(engine.requests().len(), 1);
    }

    #[tokio::test]
    async fn power_takes_parameters_only() {
        let engine = Arc::new(FakeCompute::new());
        let tool = Typed(StatsPowerTool::new(ports(Arc::clone(&engine))));
        let input = json!({ "test": "two_sample_t", "effect": 0.8, "power": 0.8 });
        let prepared = tool.prepare(input, &ctx()).await.unwrap();
        assert_eq!(prepared.subject, "two-sample t-test");
        let out = prepared.run.await.unwrap();
        assert!(!out.is_error, "{}", out.text_content());
        let ComputeRequest::Power(request) = &engine.requests()[0] else {
            panic!("not a power request");
        };
        assert_eq!(request.effect, Some(0.8));
        let bad = tool.prepare(json!({ "effect": 0.5 }), &ctx()).await;
        assert!(bad.is_err(), "a design is needed");
    }

    #[tokio::test]
    async fn an_ab_test_checks_its_counts_first() {
        let engine = Arc::new(FakeCompute::new());
        let tool = Typed(StatsAbTestTool::new(ports(Arc::clone(&engine))));
        let input = json!({
            "control": { "visitors": 1000, "conversions": 100 },
            "treatment": { "name": "new", "visitors": 1000, "conversions": 120 },
            "looks": 3
        });
        let prepared = tool.prepare(input, &ctx()).await.unwrap();
        assert_eq!(prepared.subject, "new vs A");
        assert!(!prepared.run.await.unwrap().is_error);
        let bad = json!({
            "control": { "visitors": 10, "conversions": 11 },
            "treatment": { "visitors": 10, "conversions": 1 }
        });
        assert!(tool.prepare(bad, &ctx()).await.is_err());
        assert_eq!(engine.requests().len(), 1);
    }
}
