//! Step 5, Verify hypotheses: once every task is done, runs the plan's
//! `hypothesis_tests` and `constraint_tests` and shows expected against
//! obtained in a `Table` and, when there are measurements, a `Chart`.

use async_trait::async_trait;
use domain::block::{BlockKind, Cell, ChartKind, ChartSpec, Column, Series, TableSpec};
use domain::plan::{ConstraintTest, HypothesisTest};

use crate::flows::implement::checks::{CheckRun, run_check};
use crate::flows::implement::measure::{find_measurement, format_number, parse_expectation};
use crate::flows::implement::node::{ImplementNode, NodeError, StepContext, StepOutcome};
use crate::flows::implement::state::{CheckResult, CheckSubject, EntryKind, Step};

/// What a constraint test is expected to do.
const PASSES: &str = "passes (exit 0)";

/// The result of a hypothesis test from its run.
#[must_use]
pub fn hypothesis_result(test: &HypothesisTest, run: &CheckRun) -> CheckResult {
    let mut result = CheckResult {
        subject: CheckSubject::Hypothesis,
        kind: test.kind,
        command: run.command.clone(),
        expected: test.expect.clone(),
        obtained: run.verdict(),
        measured: None,
        threshold: None,
        unit: None,
        passed: run.passed(),
    };
    let Some(expect) = parse_expectation(&test.expect) else {
        return result;
    };
    result.threshold = Some(expect.threshold);
    result.unit.clone_from(&expect.unit);
    let measured = find_measurement(&expect, &run.text());
    result.measured = measured;
    let unit = expect.unit.as_deref().unwrap_or_default();
    result.obtained = match measured {
        Some(value) if run.passed() => {
            let name = expect
                .metric
                .as_deref()
                .map_or(String::new(), |m| format!("{m} "));
            format!("{name}{}{unit}", format_number(value))
        }
        Some(value) => format!("{}{unit} · {}", format_number(value), run.verdict()),
        None => format!("no measurement · {}", run.verdict()),
    };
    result.passed =
        run.passed() && measured.is_some_and(|m| expect.comparison.holds(m, expect.threshold));
    result
}

/// The result of a constraint test from its run.
#[must_use]
pub fn constraint_result(test: &ConstraintTest, run: &CheckRun) -> CheckResult {
    CheckResult {
        subject: CheckSubject::Constraint {
            id: test.constraint,
        },
        kind: test.kind,
        command: run.command.clone(),
        expected: PASSES.to_owned(),
        obtained: run.verdict(),
        measured: None,
        threshold: None,
        unit: None,
        passed: run.passed(),
    }
}

/// Name of a check in tables and charts: `Hypothesis` or the constraint id.
#[must_use]
pub fn check_label(result: &CheckResult) -> String {
    match &result.subject {
        CheckSubject::Hypothesis => "Hypothesis".to_owned(),
        CheckSubject::Constraint { id } => id.to_string(),
    }
}

/// The table of expected against obtained.
#[must_use]
pub fn results_table(results: &[CheckResult]) -> TableSpec {
    let columns = ["Test", "Command", "Expected", "Obtained", "Holds"]
        .into_iter()
        .map(Column::new)
        .collect();
    let rows = results
        .iter()
        .map(|r| {
            vec![
                Cell::Text(check_label(r)),
                Cell::Text(r.command.clone()),
                Cell::Text(r.expected.clone()),
                Cell::Text(r.obtained.clone()),
                Cell::Bool(r.passed),
            ]
        })
        .collect();
    TableSpec { columns, rows }
}

/// A bar chart of expected against obtained for the checks with a
/// measurement, or `None` when there is none.
#[must_use]
pub fn results_chart(results: &[CheckResult]) -> Option<ChartSpec> {
    let measured: Vec<_> = results
        .iter()
        .filter_map(|r| Some((r, r.measured?, r.threshold?)))
        .collect();
    if measured.is_empty() {
        return None;
    }
    let labels = measured
        .iter()
        .map(|(r, _, _)| match &r.unit {
            Some(unit) => format!("{} ({unit})", r.expected),
            None => r.expected.clone(),
        })
        .collect();
    Some(ChartSpec {
        title: "Expected against obtained".to_owned(),
        kind: ChartKind::Bar,
        labels,
        series: vec![
            Series {
                name: "Expected".to_owned(),
                values: measured.iter().map(|(_, _, t)| *t).collect(),
            },
            Series {
                name: "Obtained".to_owned(),
                values: measured.iter().map(|(_, m, _)| *m).collect(),
            },
        ],
    })
}

/// Step 5, Verify hypotheses.
#[derive(Debug, Default)]
pub struct VerifyPlan;

impl VerifyPlan {
    /// The step.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ImplementNode for VerifyPlan {
    fn step(&self) -> Step {
        Step::Verify
    }

    async fn run(&self, ctx: &mut StepContext) -> Result<StepOutcome, NodeError> {
        let Some(plan) = ctx.state().plan.clone() else {
            return Err(NodeError::Failed("no plan was loaded".to_owned()));
        };
        let mut results = Vec::new();
        for test in &plan.hypothesis_tests {
            ctx.activity(format!("running {}", test.reference.trim()));
            let run = run_check(ctx.shell().as_ref(), &test.reference).await;
            results.push(hypothesis_result(test, &run));
        }
        for test in &plan.constraint_tests {
            ctx.activity(format!("running {}", test.reference.trim()));
            let run = run_check(ctx.shell().as_ref(), &test.reference).await;
            results.push(constraint_result(test, &run));
        }
        if results.is_empty() {
            ctx.log(
                EntryKind::Note,
                "The plan has no hypothesis or constraint tests.",
            );
        } else {
            ctx.emit(BlockKind::Table(results_table(&results)))?;
            if let Some(chart) = results_chart(&results) {
                ctx.emit(BlockKind::Chart(chart))?;
            }
        }
        for r in &results {
            let (kind, verb) = if r.passed {
                (EntryKind::Achievement, "holds")
            } else {
                (EntryKind::Failure, "does not hold")
            };
            ctx.log(
                kind,
                format!(
                    "{} {verb}: `{}` expected {} and got {}.",
                    check_label(r),
                    r.command,
                    r.expected,
                    r.obtained
                ),
            );
        }
        ctx.state_mut().checks = results;
        Ok(StepOutcome::Next)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use domain::research::{ConstraintId, TestKind};

    use super::*;
    use crate::flows::implement::nodes::tests::{Harness, exit, loaded_state};
    use crate::ports::events::DomainEvent;
    use crate::testing::fs::ScriptedShell;

    #[tokio::test]
    async fn shows_expected_against_obtained() {
        let shell = ScriptedShell::new()
            .on(
                "make",
                ["bench-sessions"],
                exit(0, "p50: 4ms\np95: 12.5ms\n"),
            )
            .on("make", ["bench-cold"], exit(0, "cold start 340 ms"))
            .on(
                "cargo",
                ["test", "-p", "store", "offline"],
                exit(101, "FAILED"),
            );
        let h = Harness::new([], Arc::new(shell));
        let mut state = loaded_state(&[("T1", &[])]);
        let plan = state.plan.as_mut().unwrap();
        plan.hypothesis_tests = vec![
            HypothesisTest {
                kind: TestKind::Command,
                reference: "make bench-sessions".into(),
                expect: "p95 < 20ms".into(),
            },
            HypothesisTest {
                kind: TestKind::Command,
                reference: "make bench-cold".into(),
                expect: "< 200 ms".into(),
            },
        ];
        plan.constraint_tests = vec![ConstraintTest {
            constraint: ConstraintId::new(2).unwrap(),
            kind: TestKind::Test,
            reference: "cargo test -p store offline".into(),
        }];
        state.current = 1;
        let mut ctx = h.ctx(Step::Verify, state);
        assert_eq!(
            VerifyPlan::new().run(&mut ctx).await.unwrap(),
            StepOutcome::Next
        );

        let checks = &ctx.state().checks;
        let summary: Vec<_> = checks
            .iter()
            .map(|c| (c.expected.as_str(), c.obtained.as_str(), c.passed))
            .collect();
        assert_eq!(
            summary,
            [
                ("p95 < 20ms", "p95 12.5ms", true),
                ("< 200 ms", "340ms", false),
                ("passes (exit 0)", "fails (exit 101)", false),
            ]
        );
        let blocks: Vec<_> = h
            .sink
            .events()
            .into_iter()
            .filter_map(|e| match e {
                DomainEvent::BlockAdded { block } => Some(block.kind),
                _ => None,
            })
            .collect();
        let BlockKind::Table(table) = &blocks[0] else {
            panic!("{blocks:?}")
        };
        assert_eq!(table.rows.len(), 3);
        assert_eq!(table.rows[2][0], Cell::Text("C2".into()));
        let BlockKind::Chart(chart) = &blocks[1] else {
            panic!("{blocks:?}")
        };
        assert_eq!(chart.series[0].values, [20.0, 200.0]);
        assert_eq!(chart.series[1].values, [12.5, 340.0]);
        assert_eq!(ctx.state().entries(EntryKind::Failure).count(), 2);
    }

    #[tokio::test]
    async fn without_measurements_there_is_no_chart() {
        let h = Harness::without_shell([]);
        let mut ctx = h.ctx(Step::Verify, loaded_state(&[("T1", &[])]));
        assert_eq!(
            VerifyPlan::new().run(&mut ctx).await.unwrap(),
            StepOutcome::Next
        );
        assert!(h.sink.events().is_empty());
        let run = CheckRun {
            command: "make check".into(),
            output: Some(exit(0, "")),
            error: None,
        };
        let test = HypothesisTest {
            kind: TestKind::Command,
            reference: "make check".into(),
            expect: "no regressions".into(),
        };
        let result = hypothesis_result(&test, &run);
        assert!(result.passed && result.obtained == "passes");
        assert!(results_chart(&[result]).is_none());
    }
}
