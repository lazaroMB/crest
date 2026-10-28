//! Hypothesis tests: t-tests, rank tests and contingency tables, with the
//! automatic choice, assumption checks, effect sizes and misuse warnings.
//!
//! Conventions follow R (`t.test`, `wilcox.test`, `chisq.test`,
//! `fisher.test`): the Mann-Whitney statistic is R's `W` (U of the first
//! group), the signed-rank statistic is `V` (sum of positive ranks), rank
//! tests are exact when n < 50 without ties (and, for signed ranks,
//! without zero differences), and χ² on a 2×2 table gets Yates'
//! correction unless `correct = false`. Intervals are always two-sided at
//! `conf_level`. Fisher's odds ratio is the sample (Woolf) estimate, not
//! R's conditional MLE.

use application::ports::compute::{ComputeError, DataColumn, TestChoice, TestRequest};
use domain::computed::{
    Alternative, Assumption, AssumptionStatus, Computed, ComputedBody, ComputedValue, GroupSummary,
    Narrative, TestBody, TestKind, UncertaintyKind, WarningSeverity, apa,
};
use serde_json::Value;
use statrs::function::factorial::ln_binomial;

use crate::checks::{Check, brown_forsythe, shapiro_wilk};
use crate::data::{self, key_part};
use crate::describe::{mean, quantile, variance};
use crate::envelope::{
    self, auto_decimals, bounded, count, df, labelled, number, p_value, warning, with_ci,
};
use crate::math::{p_from_t, p_from_z, pchisq_upper, pnt_inverse_delta, qnorm, qt};
use crate::rank::{mann_whitney_counts, ranks, signed_rank_counts, tails, tie_sum};
use crate::reproduce::{py_col, py_frame, py_str, r_col, r_frame};

/// Significance level of the assumption checks.
const CHECK_ALPHA: f64 = 0.05;

/// The data shape a request describes.
enum Shape {
    /// Two independent samples.
    Two {
        names: [String; 2],
        a: Vec<f64>,
        b: Vec<f64>,
        source: TwoSource,
    },
    /// Paired measurements; `d = x − y`.
    Paired {
        names: [String; 2],
        x: Vec<f64>,
        y: Vec<f64>,
    },
    /// One sample.
    One { name: String, x: Vec<f64> },
    /// A contingency table.
    Table {
        observed: Vec<Vec<f64>>,
        source: Option<(String, String)>,
    },
}

/// Where two samples come from, for the reproduce lines.
enum TwoSource {
    /// `y` split by `group`.
    Grouped {
        y: String,
        group: String,
        numeric_group: bool,
    },
    /// Two columns.
    Columns,
}

fn invalid(text: impl Into<String>) -> ComputeError {
    ComputeError::Invalid(text.into())
}

fn shape_of(request: &TestRequest, dropped: &mut u64) -> Result<Shape, ComputeError> {
    let data = &request.data;
    let choice = request.test;
    let contingency_test = matches!(choice, TestChoice::ChiSquare | TestChoice::Fisher);
    if let Some(table) = &request.table {
        return Ok(Shape::Table {
            observed: table.clone(),
            source: None,
        });
    }
    let y = request
        .y
        .as_deref()
        .ok_or_else(|| invalid("`y` (or `table`) is required"))?;
    let y_column = data::column(data, y)?;
    if contingency_test || !data::is_numeric(y_column) {
        let group = request
            .group
            .as_deref()
            .ok_or_else(|| invalid("a contingency test needs `y` and `group`"))?;
        let (row_labels, rows) = data::labels(data, y)?;
        let (col_labels, cols) = data::labels(data, group)?;
        let mut observed = vec![vec![0.0; cols.len()]; rows.len()];
        for (r, c) in row_labels.iter().zip(&col_labels) {
            match (r, c) {
                (Some(r), Some(c)) => {
                    let i = rows.iter().position(|l| l == r).unwrap_or_default();
                    let j = cols.iter().position(|l| l == c).unwrap_or_default();
                    observed[i][j] += 1.0;
                }
                _ => *dropped += 1,
            }
        }
        return Ok(Shape::Table {
            observed,
            source: Some((y.to_owned(), group.to_owned())),
        });
    }
    let ys = data::numeric(data, y)?;
    let paired = request.paired || choice == TestChoice::PairedT;
    if let Some(y2) = request.y2.as_deref() {
        let y2s = data::numeric(data, y2)?;
        let names = [y.to_owned(), y2.to_owned()];
        if paired || choice == TestChoice::Wilcoxon {
            let (mut x, mut z) = (Vec::new(), Vec::new());
            for (a, b) in ys.iter().zip(&y2s) {
                match (a, b) {
                    (Some(a), Some(b)) => {
                        x.push(*a);
                        z.push(*b);
                    }
                    _ => *dropped += 1,
                }
            }
            return Ok(Shape::Paired { names, x, y: z });
        }
        *dropped += data::missing(&ys) + data::missing(&y2s);
        return Ok(Shape::Two {
            names,
            a: data::present(&ys),
            b: data::present(&y2s),
            source: TwoSource::Columns,
        });
    }
    if paired {
        return Err(invalid("a paired test needs `y` and `y2`"));
    }
    if let Some(group) = request.group.as_deref() {
        let (labels, mut levels) = data::labels(data, group)?;
        // Levels that occur with a value of y.
        levels.retain(|level| {
            labels
                .iter()
                .zip(&ys)
                .any(|(l, v)| l.as_deref() == Some(level.as_str()) && v.is_some())
        });
        if !request.levels.is_empty() {
            if request.levels.len() != 2 {
                return Err(invalid("`levels` must name exactly two groups"));
            }
            for level in &request.levels {
                if !levels.contains(level) {
                    return Err(invalid(format!("group {level:?} has no values of {y}")));
                }
            }
            levels = request.levels.clone();
        }
        if levels.len() != 2 {
            return Err(invalid(format!(
                "`{group}` has {} groups with values ({}); a two-sample test needs two (set `levels`)",
                levels.len(),
                levels.join(", ")
            )));
        }
        let pick = |level: &str| -> Vec<f64> {
            labels
                .iter()
                .zip(&ys)
                .filter(|(l, _)| l.as_deref() == Some(level))
                .filter_map(|(_, v)| *v)
                .collect()
        };
        let (a, b) = (pick(&levels[0]), pick(&levels[1]));
        *dropped += (ys.len() - a.len() - b.len()) as u64;
        let numeric_group = data.column(group).is_some_and(data::is_numeric);
        return Ok(Shape::Two {
            names: [levels[0].clone(), levels[1].clone()],
            a,
            b,
            source: TwoSource::Grouped {
                y: y.to_owned(),
                group: group.to_owned(),
                numeric_group,
            },
        });
    }
    *dropped += data::missing(&ys);
    Ok(Shape::One {
        name: y.to_owned(),
        x: data::present(&ys),
    })
}

/// The assumption entry of a normality check.
fn normality(id: &str, label: &str, xs: &[f64]) -> (Assumption, Option<Check>) {
    let check = shapiro_wilk(xs);
    let assumption = match check {
        Some(c) => Assumption {
            id: id.into(),
            check: Some("Shapiro-Wilk".into()),
            stat: Some(c.stat),
            p: Some(c.p),
            status: if c.p < CHECK_ALPHA {
                AssumptionStatus::Fail
            } else {
                AssumptionStatus::Pass
            },
            consequence: None,
            note: Some(format!("normality of {label}")),
        },
        None => Assumption {
            id: id.into(),
            check: None,
            stat: None,
            p: None,
            status: AssumptionStatus::NotChecked,
            consequence: None,
            note: Some(format!(
                "normality of {label}: needs 3 to 5000 values that are not all equal"
            )),
        },
    };
    (assumption, check)
}

fn design(id: &str, note: &str) -> Assumption {
    Assumption {
        id: id.into(),
        check: None,
        stat: None,
        p: None,
        status: AssumptionStatus::NotChecked,
        consequence: None,
        note: Some(note.into()),
    }
}

fn kind_name(kind: TestKind) -> (&'static str, &'static str) {
    match kind {
        TestKind::WelchT => ("stats.ttest_welch", "Welch two-sample t-test"),
        TestKind::StudentT => ("stats.ttest_student", "Student two-sample t-test"),
        TestKind::PairedT => ("stats.ttest_paired", "Paired t-test"),
        TestKind::OneSampleT => ("stats.ttest_one_sample", "One-sample t-test"),
        TestKind::MannWhitney => ("stats.mann_whitney", "Mann-Whitney U test"),
        TestKind::Wilcoxon => ("stats.wilcoxon", "Wilcoxon signed-rank test"),
        TestKind::ChiSquare => ("stats.chi_square", "Pearson χ² test of independence"),
        TestKind::Fisher => ("stats.fisher", "Fisher's exact test"),
    }
}

fn snake(kind: TestKind) -> &'static str {
    match kind {
        TestKind::WelchT => "welch_t",
        TestKind::StudentT => "student_t",
        TestKind::PairedT => "paired_t",
        TestKind::OneSampleT => "one_sample_t",
        TestKind::MannWhitney => "mann_whitney",
        TestKind::Wilcoxon => "wilcoxon",
        TestKind::ChiSquare => "chi_square",
        TestKind::Fisher => "fisher",
    }
}

fn p_text(p: f64) -> String {
    let (relation, value) = apa::p_value_parts(p);
    format!("p {relation} {value}")
}

/// Cohen's label of a standardized mean difference.
fn d_label(d: f64) -> &'static str {
    match d.abs() {
        x if x < 0.2 => "negligible",
        x if x < 0.5 => "small",
        x if x < 0.8 => "medium",
        _ => "large",
    }
}

/// Cohen's label of a correlation-type effect.
pub fn r_label(r: f64) -> &'static str {
    match r.abs() {
        x if x < 0.1 => "negligible",
        x if x < 0.3 => "small",
        x if x < 0.5 => "medium",
        _ => "large",
    }
}

/// A standardized mean difference with its interval: noncentral t when it
/// converges, else the normal approximation; `t` is the t statistic of
/// `d` on `dof` degrees of freedom and `scale` turns δ into d.
fn d_value(d: f64, t: f64, dof: f64, scale: f64, se_normal: f64, level: f64) -> ComputedValue {
    let alpha = 1.0 - level;
    let noncentral =
        pnt_inverse_delta(t, dof, 1.0 - alpha / 2.0).zip(pnt_inverse_delta(t, dof, alpha / 2.0));
    let value = number(d, 2);
    let value = match noncentral {
        Some((lo, hi)) => with_ci(value, level, lo * scale, hi * scale, "noncentral_t"),
        None => {
            let z = qnorm(1.0 - alpha / 2.0);
            with_ci(
                value,
                level,
                d - z * se_normal,
                d + z * se_normal,
                "normal_approx",
            )
        }
    };
    labelled(value, d_label(d), "Cohen 1988")
}

fn hedges(d: &ComputedValue, dof: f64) -> ComputedValue {
    let j = 1.0 - 3.0 / (4.0 * dof - 1.0);
    let mut g = number(d.v * j, 2);
    if let Some(ci) = &d.ci {
        let method = ci.method.clone().unwrap_or_default();
        g = with_ci(g, ci.level, ci.lo * j, ci.hi * j, &method);
    }
    labelled(g, d_label(d.v * j), "Cohen 1988")
}

fn group_summary(name: &str, xs: &[f64]) -> GroupSummary {
    let mut sorted = xs.to_vec();
    sorted.sort_by(f64::total_cmp);
    let finite = |x: f64| x.is_finite().then_some(x);
    GroupSummary {
        name: name.to_owned(),
        n: xs.len() as u64,
        mean: if xs.is_empty() {
            None
        } else {
            finite(mean(xs))
        },
        sd: finite(variance(xs).sqrt()),
        median: finite(quantile(&sorted, 0.5)),
    }
}

fn group_values(computed: &mut Computed, summary: &GroupSummary, decimals: u32) {
    let key = format!("group.{}", key_part(&summary.name));
    computed.values.insert(format!("{key}.n"), count(summary.n));
    for (field, v) in [
        ("mean", summary.mean),
        ("sd", summary.sd),
        ("median", summary.median),
    ] {
        if let Some(v) = v {
            computed
                .values
                .insert(format!("{key}.{field}"), number(v, decimals));
        }
    }
}

fn alt_py(alternative: Alternative) -> &'static str {
    match alternative {
        Alternative::TwoSided => "two-sided",
        Alternative::Less => "less",
        Alternative::Greater => "greater",
    }
}

fn alt_r(alternative: Alternative) -> &'static str {
    match alternative {
        Alternative::TwoSided => "two.sided",
        Alternative::Less => "less",
        Alternative::Greater => "greater",
    }
}

/// The test the engine runs, with why and the assumption checks.
struct Choice {
    kind: TestKind,
    why: Option<String>,
    assumptions: Vec<Assumption>,
    /// Whether a normality check failed.
    non_normal: bool,
}

fn choose(request: &TestRequest, shape: &Shape) -> Result<Choice, ComputeError> {
    let forced = request.test.kind();
    let mismatch = |kind: TestKind| {
        invalid(format!(
            "{} does not fit these data; {}",
            snake(kind),
            match shape {
                Shape::Two { .. } =>
                    "two independent samples take welch_t, student_t or mann_whitney",
                Shape::Paired { .. } => "paired data take paired_t or wilcoxon",
                Shape::One { .. } => "one sample takes one_sample_t or wilcoxon",
                Shape::Table { .. } => "a contingency table takes chi_square or fisher",
            }
        ))
    };
    match shape {
        Shape::Two { names, a, b, .. } => {
            let (n1, c1) = normality(&format!("normality_{}", key_part(&names[0])), &names[0], a);
            let (n2, c2) = normality(&format!("normality_{}", key_part(&names[1])), &names[1], b);
            let bf = brown_forsythe(&[a, b]);
            let mut equal_var = Assumption {
                id: "equal_var".into(),
                check: bf.map(|_| "Brown-Forsythe".into()),
                stat: bf.map(|c| c.stat),
                p: bf.map(|c| c.p),
                status: match bf {
                    Some(c) if c.p < CHECK_ALPHA => AssumptionStatus::Fail,
                    Some(_) => AssumptionStatus::Pass,
                    None => AssumptionStatus::NotChecked,
                },
                consequence: None,
                note: Some("equal variances".into()),
            };
            let failing: Vec<(&str, Check)> = [(&names[0], c1), (&names[1], c2)]
                .into_iter()
                .filter_map(|(n, c)| c.filter(|c| c.p < CHECK_ALPHA).map(|c| (n.as_str(), c)))
                .collect();
            let non_normal = !failing.is_empty();
            let small = a.len().min(b.len()) < 30;
            let (kind, why) = match forced {
                Some(k @ (TestKind::WelchT | TestKind::StudentT | TestKind::MannWhitney)) => {
                    (k, None)
                }
                Some(k) => return Err(mismatch(k)),
                None => {
                    if let Some((name, c)) = failing.first().filter(|_| small) {
                        (
                            TestKind::MannWhitney,
                            Some(format!(
                                "Shapiro-Wilk {} in group {name} with n < 30 → rank test",
                                p_text(c.p)
                            )),
                        )
                    } else if let Some(c) = bf.filter(|c| c.p < CHECK_ALPHA) {
                        (
                            TestKind::WelchT,
                            Some(format!(
                                "Brown-Forsythe {} → unequal variances",
                                p_text(c.p)
                            )),
                        )
                    } else if non_normal {
                        (
                            TestKind::WelchT,
                            Some("non-normal groups but n ≥ 30 in both → Welch t".into()),
                        )
                    } else {
                        let bf_text = bf.map_or("not checkable".into(), |c| p_text(c.p));
                        (
                            TestKind::StudentT,
                            Some(format!(
                                "Shapiro-Wilk p ≥ .05 in both groups and Brown-Forsythe {bf_text} → equal variances"
                            )),
                        )
                    }
                }
            };
            let mut normals = [n1, n2];
            for n in &mut normals {
                if n.status == AssumptionStatus::Fail {
                    n.consequence = Some(match kind {
                        TestKind::MannWhitney => "rank test does not assume normality".into(),
                        _ if !small => "n ≥ 30: the t-test is robust to this".into(),
                        _ => "the t-test may be unreliable; consider mann_whitney".into(),
                    });
                }
            }
            match kind {
                TestKind::WelchT => {
                    equal_var.consequence = Some("Welch does not assume equal variances".into());
                }
                TestKind::StudentT if equal_var.status == AssumptionStatus::Fail => {
                    equal_var.consequence =
                        Some("Student t assumes equal variances; consider welch_t".into());
                }
                _ => {}
            }
            let mut assumptions: Vec<Assumption> = normals.into();
            if kind == TestKind::MannWhitney {
                assumptions.push(design(
                    "same_shape",
                    "U compares whole distributions; reading it as a shift in medians needs similar shapes",
                ));
            } else {
                assumptions.push(equal_var);
            }
            assumptions.push(design("independence", "design property; confirm"));
            Ok(Choice {
                kind,
                why,
                assumptions,
                non_normal,
            })
        }
        Shape::Paired { x, y, .. } => {
            let d: Vec<f64> = x.iter().zip(y).map(|(a, b)| a - b).collect();
            let (mut normal, check) = normality("normality_diff", "the differences", &d);
            let non_normal = check.is_some_and(|c| c.p < CHECK_ALPHA);
            let (kind, why) = match forced {
                Some(k @ (TestKind::PairedT | TestKind::Wilcoxon)) => (k, None),
                Some(k) => return Err(mismatch(k)),
                None => match check {
                    Some(c) if non_normal && d.len() < 30 => (
                        TestKind::Wilcoxon,
                        Some(format!(
                            "Shapiro-Wilk {} on the differences with n < 30 → signed-rank test",
                            p_text(c.p)
                        )),
                    ),
                    _ => (
                        TestKind::PairedT,
                        Some(
                            "paired measurements; differences consistent with normality or n ≥ 30"
                                .into(),
                        ),
                    ),
                },
            };
            if non_normal {
                normal.consequence = Some(if kind == TestKind::Wilcoxon {
                    "signed-rank test does not assume normality".into()
                } else {
                    "the t-test may be unreliable with small n; consider wilcoxon".into()
                });
            }
            let mut assumptions = vec![normal];
            if kind == TestKind::Wilcoxon {
                assumptions.push(design(
                    "symmetry",
                    "differences symmetric about their median",
                ));
            }
            assumptions.push(design(
                "independence",
                "pairs independent of each other; confirm",
            ));
            Ok(Choice {
                kind,
                why,
                assumptions,
                non_normal,
            })
        }
        Shape::One { name, x } => {
            let (mut normal, check) = normality("normality", name, x);
            let non_normal = check.is_some_and(|c| c.p < CHECK_ALPHA);
            let (kind, why) = match forced {
                Some(k @ (TestKind::OneSampleT | TestKind::Wilcoxon)) => (k, None),
                Some(k) => return Err(mismatch(k)),
                None => match check {
                    Some(c) if non_normal && x.len() < 30 => (
                        TestKind::Wilcoxon,
                        Some(format!(
                            "Shapiro-Wilk {} with n < 30 → signed-rank test",
                            p_text(c.p)
                        )),
                    ),
                    _ => (
                        TestKind::OneSampleT,
                        Some("one sample; consistent with normality or n ≥ 30".into()),
                    ),
                },
            };
            if non_normal {
                normal.consequence = Some(if kind == TestKind::Wilcoxon {
                    "signed-rank test does not assume normality".into()
                } else {
                    "the t-test may be unreliable with small n; consider wilcoxon".into()
                });
            }
            Ok(Choice {
                kind,
                why,
                assumptions: vec![normal, design("independence", "design property; confirm")],
                non_normal,
            })
        }
        Shape::Table { observed, .. } => {
            let two_by_two = observed.len() == 2 && observed.iter().all(|r| r.len() == 2);
            let min_expected = expected(observed).map_or(f64::NAN, |(e, _)| {
                e.iter().flatten().copied().fold(f64::INFINITY, f64::min)
            });
            let (kind, why) = match forced {
                Some(TestKind::Fisher) if !two_by_two => {
                    return Err(invalid("fisher needs a 2×2 table; use chi_square"));
                }
                Some(k @ (TestKind::ChiSquare | TestKind::Fisher)) => (k, None),
                Some(k) => return Err(mismatch(k)),
                None if two_by_two && min_expected < 5.0 => (
                    TestKind::Fisher,
                    Some(format!(
                        "2×2 table with an expected count of {} < 5 → exact test",
                        apa::number(min_expected, 2)
                    )),
                ),
                None => (
                    TestKind::ChiSquare,
                    Some(if min_expected < 5.0 {
                        "table larger than 2×2; Fisher's exact test is only run on 2×2".into()
                    } else {
                        "all expected counts ≥ 5".into()
                    }),
                ),
            };
            let mut assumptions = Vec::new();
            if kind == TestKind::ChiSquare {
                assumptions.push(Assumption {
                    id: "expected_counts".into(),
                    check: Some("minimum expected count".into()),
                    stat: min_expected.is_finite().then_some(min_expected),
                    p: None,
                    status: if min_expected >= 5.0 {
                        AssumptionStatus::Pass
                    } else if min_expected >= 1.0 {
                        AssumptionStatus::Warn
                    } else {
                        AssumptionStatus::Fail
                    },
                    consequence: (min_expected < 5.0)
                        .then(|| "the χ² approximation may be poor".into()),
                    note: Some("every expected count ≥ 5".into()),
                });
            } else {
                assumptions.push(design("fixed_margins", "conditions on both margins"));
            }
            assumptions.push(design(
                "independence",
                "each observation counted once; confirm",
            ));
            Ok(Choice {
                kind,
                why,
                assumptions,
                non_normal: false,
            })
        }
    }
}

/// Expected counts under independence and the total; `None` when a row or
/// column is empty or the table is malformed.
pub fn expected(observed: &[Vec<f64>]) -> Option<(Vec<Vec<f64>>, f64)> {
    let cols = observed.first()?.len();
    if observed.len() < 2
        || cols < 2
        || observed.iter().any(|r| r.len() != cols)
        || observed
            .iter()
            .flatten()
            .any(|x| !(x.is_finite() && *x >= 0.0))
    {
        return None;
    }
    let row_sums: Vec<f64> = observed.iter().map(|r| r.iter().sum()).collect();
    let col_sums: Vec<f64> = (0..cols)
        .map(|j| observed.iter().map(|r| r[j]).sum())
        .collect();
    let n: f64 = row_sums.iter().sum();
    if row_sums.iter().chain(&col_sums).any(|s| *s <= 0.0) {
        return None;
    }
    let e = row_sums
        .iter()
        .map(|r| col_sums.iter().map(|c| r * c / n).collect())
        .collect();
    Some((e, n))
}

/// Runs a test request.
pub fn run(request: &TestRequest, id: String) -> Result<(Computed, ComputedBody), ComputeError> {
    if !(request.conf_level > 0.0 && request.conf_level < 1.0) {
        return Err(invalid("`conf_level` must be between 0 and 1"));
    }
    let mut dropped = 0;
    let shape = shape_of(request, &mut dropped)?;
    let choice = choose(request, &shape)?;
    let (kind_str, method_name) = kind_name(choice.kind);
    let mut computed = Computed::new(id, kind_str, method_name, envelope::engine());
    computed.method.why = choice.why.clone();
    computed.uncertainty_kind = Some(UncertaintyKind::FrequentistCi);
    computed.assumptions = choice.assumptions;
    let params = &mut computed.method.params;
    params.insert("test".into(), Value::from(snake(choice.kind)));
    params.insert(
        "alternative".into(),
        Value::from(alt_py(request.alternative)),
    );
    params.insert("conf_level".into(), Value::from(request.conf_level));
    params.insert("ci".into(), Value::from("two-sided"));
    if dropped > 0 {
        computed.warnings.push(warning(
            "missing_dropped",
            WarningSeverity::Info,
            format!("{dropped} rows with missing values were left out"),
        ));
    }
    let mut body = TestBody {
        test: choice.kind,
        alternative: request.alternative,
        groups: Vec::new(),
        observed: None,
        expected: None,
    };
    let used = used_columns(request);
    let extra = request
        .table
        .as_ref()
        .map(|t| serde_json::to_value(t).unwrap_or_default());
    computed
        .inputs
        .push(envelope::input(&request.data, &used, extra.as_ref()));
    match &shape {
        Shape::Two {
            names,
            a,
            b,
            source,
        } => {
            two_sample(
                &mut computed,
                &mut body,
                request,
                choice.kind,
                names,
                a,
                b,
                choice.non_normal,
            );
            reproduce_two(&mut computed, request, choice.kind, names, source, &used);
        }
        Shape::Paired { names, x, y } => {
            let d: Vec<f64> = x.iter().zip(y).map(|(a, b)| a - b).collect();
            body.groups = vec![group_summary(&names[0], x), group_summary(&names[1], y)];
            one_sample(
                &mut computed,
                request,
                choice.kind,
                &d,
                true,
                choice.non_normal,
            );
            reproduce_paired(&mut computed, request, choice.kind, names, &used);
        }
        Shape::One { name, x } => {
            body.groups = vec![group_summary(name, x)];
            one_sample(
                &mut computed,
                request,
                choice.kind,
                x,
                false,
                choice.non_normal,
            );
            reproduce_one(&mut computed, request, choice.kind, name, &used);
        }
        Shape::Table { observed, source } => {
            contingency(&mut computed, &mut body, request, choice.kind, observed);
            reproduce_table(
                &mut computed,
                request,
                choice.kind,
                observed,
                source.as_ref(),
                &used,
            );
        }
    }
    let alternatives: Vec<&str> = match choice.kind {
        TestKind::WelchT => vec!["student_t", "mann_whitney"],
        TestKind::StudentT => vec!["welch_t", "mann_whitney"],
        TestKind::MannWhitney => vec!["welch_t", "student_t"],
        TestKind::PairedT => vec!["wilcoxon"],
        TestKind::OneSampleT => vec!["wilcoxon"],
        TestKind::Wilcoxon if matches!(shape, Shape::Paired { .. }) => vec!["paired_t"],
        TestKind::Wilcoxon => vec!["one_sample_t"],
        TestKind::ChiSquare if matches!(&shape, Shape::Table { observed, .. } if observed.len() == 2 && observed.iter().all(|r| r.len() == 2)) =>
        {
            vec!["fisher"]
        }
        TestKind::ChiSquare => Vec::new(),
        TestKind::Fisher => vec!["chi_square"],
    };
    computed.method.alternatives = alternatives.into_iter().map(str::to_owned).collect();
    let (template, slots): (&str, &[&str]) = match choice.kind {
        TestKind::WelchT | TestKind::StudentT | TestKind::PairedT | TestKind::OneSampleT => {
            ("apa_ttest", &["statistic", "df", "p_value", "effect"])
        }
        TestKind::MannWhitney => ("apa_mann_whitney", &["statistic", "p_value", "effect"]),
        TestKind::Wilcoxon => ("apa_wilcoxon", &["statistic", "p_value", "effect"]),
        TestKind::ChiSquare => ("apa_chisq", &["statistic", "df", "n", "p_value", "effect"]),
        TestKind::Fisher => ("fisher_exact", &["p_value", "effect"]),
    };
    if computed.status != domain::computed::ComputedStatus::Error {
        computed.narrative = Some(Narrative {
            template: template.into(),
            text_slots: slots
                .iter()
                .filter(|k| computed.values.contains_key(**k))
                .map(|k| (*k).to_owned())
                .collect(),
        });
    }
    let lua_request = serde_json::to_value(application::ports::compute::ComputeRequest::Test(
        request.clone(),
    ))
    .unwrap_or_default();
    computed.reproduce.lua = Some(crate::reproduce::lua(
        "test",
        &lua_request,
        Some(&request.data),
    ));
    Ok((computed, ComputedBody::Test(body)))
}

fn used_columns(request: &TestRequest) -> Vec<DataColumn> {
    let names: Vec<&str> = [&request.y, &request.y2, &request.group]
        .into_iter()
        .flatten()
        .map(String::as_str)
        .collect();
    data::used_columns(&request.data, &names)
}

fn small_n_warning(computed: &mut Computed, sizes: &[usize], non_normal: bool) {
    let min = sizes.iter().copied().min().unwrap_or_default();
    if non_normal && min < 30 {
        computed.warnings.push(warning(
            "small_n",
            WarningSeverity::Warn,
            "n < 30 with non-normal data: the t-test may be unreliable; consider the rank test",
        ));
    } else if min < 20 {
        computed.warnings.push(warning(
            "small_n",
            WarningSeverity::Warn,
            "n < 20 per group: the normality check has little power",
        ));
    }
}

#[allow(clippy::too_many_arguments)]
fn two_sample(
    computed: &mut Computed,
    body: &mut TestBody,
    request: &TestRequest,
    kind: TestKind,
    names: &[String; 2],
    a: &[f64],
    b: &[f64],
    non_normal: bool,
) {
    let level = request.conf_level;
    body.groups = vec![group_summary(&names[0], a), group_summary(&names[1], b)];
    let (n1, n2) = (a.len(), b.len());
    if n1 < 2 || n2 < 2 {
        envelope::fail(
            computed,
            format!(
                "each group needs at least 2 values ({}: {n1}, {}: {n2})",
                names[0], names[1]
            ),
        );
        return;
    }
    let (f1, f2) = (n1 as f64, n2 as f64);
    let (m1, m2) = (mean(a), mean(b));
    let (v1, v2) = (variance(a), variance(b));
    let pooled_df = f1 + f2 - 2.0;
    let sp = (((f1 - 1.0) * v1 + (f2 - 1.0) * v2) / pooled_df).sqrt();
    let decimals = auto_decimals(sp);
    for g in &body.groups {
        group_values(computed, g, decimals);
    }
    let unit = request
        .y
        .as_deref()
        .and_then(|y| request.data.column(y))
        .and_then(|c| c.unit.clone());
    computed.values.insert("n".into(), count((n1 + n2) as u64));
    if kind == TestKind::MannWhitney {
        mann_whitney(computed, request, a, b);
        return;
    }
    if sp == 0.0 {
        envelope::fail(computed, "both groups are constant, so t is undefined");
        return;
    }
    small_n_warning(computed, &[n1, n2], non_normal);
    let (se, dof, ci_method) = if kind == TestKind::WelchT {
        let (s1, s2) = (v1 / f1, v2 / f2);
        let se = (s1 + s2).sqrt();
        let dof = (s1 + s2).powi(2) / (s1 * s1 / (f1 - 1.0) + s2 * s2 / (f2 - 1.0));
        (se, dof, "welch_t")
    } else {
        (sp * (1.0 / f1 + 1.0 / f2).sqrt(), pooled_df, "student_t")
    };
    let diff = m1 - m2;
    let t = (diff - request.mu) / se;
    let p = p_from_t(t, dof, request.alternative);
    let q = qt(1.0 - (1.0 - level) / 2.0, dof);
    computed.values.insert("statistic".into(), number(t, 2));
    computed.values.insert("df".into(), df(dof));
    computed.values.insert("p_value".into(), p_value(p));
    let mut mean_diff = with_ci(
        number(diff, decimals),
        level,
        diff - q * se,
        diff + q * se,
        ci_method,
    );
    mean_diff.unit = unit;
    computed.values.insert("mean_diff".into(), mean_diff);
    // Cohen's d on the pooled SD; its interval inverts the Student t.
    let d = diff / sp;
    let scale = (1.0 / f1 + 1.0 / f2).sqrt();
    let se_normal = ((f1 + f2) / (f1 * f2) + d * d / (2.0 * (f1 + f2))).sqrt();
    let d_value = d_value(d, d / scale, pooled_df, scale, se_normal, level);
    computed
        .values
        .insert("hedges_g".into(), hedges(&d_value, pooled_df));
    computed.values.insert("cohens_d".into(), d_value.clone());
    computed.values.insert("effect".into(), d_value);
}

fn mann_whitney(computed: &mut Computed, request: &TestRequest, a: &[f64], b: &[f64]) {
    let (n1, n2) = (a.len(), b.len());
    let (f1, f2) = (n1 as f64, n2 as f64);
    let all: Vec<f64> = a.iter().chain(b).copied().collect();
    let (r, ties) = ranks(&all);
    let r1: f64 = r[..n1].iter().sum();
    let u = r1 - f1 * (f1 + 1.0) / 2.0;
    let exact = request.exact.unwrap_or(n1 < 50 && n2 < 50) && ties.is_empty();
    if request.exact == Some(true) && !ties.is_empty() {
        computed.warnings.push(warning(
            "ties_normal_approx",
            WarningSeverity::Info,
            "ties: the exact p-value is not available; used the normal approximation",
        ));
    }
    let p = if exact {
        let counts = mann_whitney_counts(n1, n2);
        // Without ties U is a whole number.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let ui = u.round() as usize;
        let (lower, upper) = tails(&counts, ui);
        match request.alternative {
            Alternative::Greater => upper,
            Alternative::Less => lower,
            Alternative::TwoSided => (2.0 * lower.min(upper)).min(1.0),
        }
    } else {
        let n = f1 + f2;
        let sigma = (f1 * f2 / 12.0 * ((n + 1.0) - tie_sum(&ties) / (n * (n - 1.0)))).sqrt();
        if sigma == 0.0 {
            envelope::fail(
                computed,
                "all values are equal, so the rank test is undefined",
            );
            return;
        }
        let z0 = u - f1 * f2 / 2.0;
        let correction = continuity(request, z0);
        let z = (z0 - correction) / sigma;
        computed.values.insert("z".into(), number(z, 2));
        if !ties.is_empty() {
            computed.warnings.push(warning(
                "ties_normal_approx",
                WarningSeverity::Info,
                "ties in the data: p from the normal approximation with tie correction",
            ));
        }
        p_from_z(z, request.alternative)
    };
    computed.method.params.insert(
        "p_method".into(),
        Value::from(if exact { "exact" } else { "normal_approx" }),
    );
    computed.values.insert("statistic".into(), number(u, 1));
    computed.values.insert("p_value".into(), p_value(p));
    let rb = 2.0 * u / (f1 * f2) - 1.0;
    let effect = labelled(bounded(rb, 2), r_label(rb), "Cohen 1988");
    computed
        .values
        .insert("rank_biserial".into(), effect.clone());
    computed.values.insert("effect".into(), effect);
}

fn continuity(request: &TestRequest, z: f64) -> f64 {
    if !request.correct {
        return 0.0;
    }
    match request.alternative {
        Alternative::TwoSided => 0.5 * z.signum() * f64::from(u8::from(z != 0.0)),
        Alternative::Greater => 0.5,
        Alternative::Less => -0.5,
    }
}

fn one_sample(
    computed: &mut Computed,
    request: &TestRequest,
    kind: TestKind,
    xs: &[f64],
    paired: bool,
    non_normal: bool,
) {
    let level = request.conf_level;
    let n = xs.len();
    computed.values.insert("n".into(), count(n as u64));
    if kind == TestKind::Wilcoxon {
        signed_rank(computed, request, xs);
        return;
    }
    if n < 2 {
        envelope::fail(computed, format!("needs at least 2 values, got {n}"));
        return;
    }
    let f = n as f64;
    let m = mean(xs);
    let s = variance(xs).sqrt();
    if s == 0.0 {
        envelope::fail(
            computed,
            if paired {
                "all differences are equal, so t is undefined"
            } else {
                "the values are constant, so t is undefined"
            },
        );
        return;
    }
    small_n_warning(computed, &[n], non_normal);
    let se = s / f.sqrt();
    let dof = f - 1.0;
    let t = (m - request.mu) / se;
    let p = p_from_t(t, dof, request.alternative);
    let q = qt(1.0 - (1.0 - level) / 2.0, dof);
    let decimals = auto_decimals(s);
    computed.values.insert("statistic".into(), number(t, 2));
    computed.values.insert("df".into(), df(dof));
    computed.values.insert("p_value".into(), p_value(p));
    let key = if paired { "mean_diff" } else { "mean" };
    let mut estimate = with_ci(
        number(m, decimals),
        level,
        m - q * se,
        m + q * se,
        "student_t",
    );
    estimate.unit = request
        .y
        .as_deref()
        .and_then(|y| request.data.column(y))
        .and_then(|c| c.unit.clone());
    computed.values.insert(key.into(), estimate);
    let d = (m - request.mu) / s;
    let scale = 1.0 / f.sqrt();
    let se_normal = (1.0 / f + d * d / (2.0 * f)).sqrt();
    let d_value = d_value(d, t, dof, scale, se_normal, level);
    computed
        .values
        .insert("hedges_g".into(), hedges(&d_value, dof));
    computed.values.insert("cohens_d".into(), d_value.clone());
    computed.values.insert("effect".into(), d_value);
}

fn signed_rank(computed: &mut Computed, request: &TestRequest, xs: &[f64]) {
    let d: Vec<f64> = xs.iter().map(|x| x - request.mu).collect();
    let nonzero: Vec<f64> = d.iter().copied().filter(|x| *x != 0.0).collect();
    let zeros = d.len() - nonzero.len();
    let n = nonzero.len();
    if n == 0 {
        envelope::fail(
            computed,
            "every difference is zero, so the signed-rank test is undefined",
        );
        return;
    }
    let abs: Vec<f64> = nonzero.iter().map(|x| x.abs()).collect();
    let (r, ties) = ranks(&abs);
    let v: f64 = r
        .iter()
        .zip(&nonzero)
        .filter(|(_, x)| **x > 0.0)
        .map(|(r, _)| r)
        .sum();
    let f = n as f64;
    let total = f * (f + 1.0) / 2.0;
    if zeros > 0 {
        computed.warnings.push(warning(
            "zero_differences",
            WarningSeverity::Info,
            format!("{zeros} zero differences were dropped"),
        ));
    }
    let exact = request.exact.unwrap_or(n < 50) && ties.is_empty() && zeros == 0;
    let p = if exact {
        let counts = signed_rank_counts(n);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let vi = v.round() as usize;
        let (lower, upper) = tails(&counts, vi);
        match request.alternative {
            Alternative::Greater => upper,
            Alternative::Less => lower,
            Alternative::TwoSided => (2.0 * lower.min(upper)).min(1.0),
        }
    } else {
        let sigma = (f * (f + 1.0) * (2.0 * f + 1.0) / 24.0 - tie_sum(&ties) / 48.0).sqrt();
        let z0 = v - total / 2.0;
        let z = (z0 - continuity(request, z0)) / sigma;
        computed.values.insert("z".into(), number(z, 2));
        if !ties.is_empty() || zeros > 0 {
            computed.warnings.push(warning(
                "ties_normal_approx",
                WarningSeverity::Info,
                "ties or zeros: p from the normal approximation with tie correction",
            ));
        }
        p_from_z(z, request.alternative)
    };
    computed.method.params.insert(
        "p_method".into(),
        Value::from(if exact { "exact" } else { "normal_approx" }),
    );
    computed.values.insert("statistic".into(), number(v, 1));
    computed.values.insert("p_value".into(), p_value(p));
    let rb = (v - (total - v)) / total;
    let effect = labelled(bounded(rb, 2), r_label(rb), "Cohen 1988");
    computed
        .values
        .insert("rank_biserial".into(), effect.clone());
    computed.values.insert("effect".into(), effect);
}

fn contingency(
    computed: &mut Computed,
    body: &mut TestBody,
    request: &TestRequest,
    kind: TestKind,
    observed: &[Vec<f64>],
) {
    body.observed = Some(observed.to_vec());
    let Some((e, n)) = expected(observed) else {
        envelope::fail(
            computed,
            "the table needs at least 2×2 non-negative counts with no empty row or column",
        );
        return;
    };
    body.expected = Some(e.clone());
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let total = n.round() as u64;
    computed.values.insert("n".into(), count(total));
    let two_by_two = observed.len() == 2 && observed[0].len() == 2;
    let min_expected = e.iter().flatten().copied().fold(f64::INFINITY, f64::min);
    if two_by_two {
        let (a, b, c, d) = (
            observed[0][0],
            observed[0][1],
            observed[1][0],
            observed[1][1],
        );
        let (or, lo, hi, method) = odds_ratio(a, b, c, d, request.conf_level);
        let value = with_ci(number(or, 2), request.conf_level, lo, hi, method);
        computed.values.insert("odds_ratio".into(), value);
    }
    if kind == TestKind::Fisher {
        if !two_by_two {
            envelope::fail(computed, "Fisher's exact test needs a 2×2 table");
            return;
        }
        let Some(p) = fisher_p(observed, request.alternative) else {
            envelope::fail(computed, "Fisher's exact test needs whole-number counts");
            return;
        };
        computed.values.insert("p_value".into(), p_value(p));
        if let Some(or) = computed.values.get("odds_ratio").cloned() {
            computed.values.insert("effect".into(), or);
        }
        return;
    }
    let cells = || observed.iter().flatten().zip(e.iter().flatten());
    let raw: f64 = cells().map(|(o, e)| (o - e).powi(2) / e).sum();
    let yates = request.correct && two_by_two;
    let chi2 = if yates {
        cells()
            .map(|(o, e)| {
                let dev = (o - e).abs();
                (dev - dev.min(0.5)).powi(2) / e
            })
            .sum()
    } else {
        raw
    };
    if yates {
        computed.method.name =
            "Pearson χ² test of independence with Yates' continuity correction".into();
    }
    let dof = ((observed.len() - 1) * (observed[0].len() - 1)) as f64;
    let p = pchisq_upper(chi2, dof);
    computed.values.insert("statistic".into(), number(chi2, 2));
    computed.values.insert("df".into(), df(dof));
    computed.values.insert("p_value".into(), p_value(p));
    let k = observed.len().min(observed[0].len()) as f64 - 1.0;
    let v = (raw / (n * k)).sqrt();
    // Cohen's w thresholds, scaled by the smaller dimension.
    let w = v * k.sqrt();
    let effect = labelled(
        bounded(v, 2),
        r_label(w),
        "Cohen 1988, w scaled by min(r, c) − 1",
    );
    computed.values.insert("cramers_v".into(), effect.clone());
    computed.values.insert("effect".into(), effect);
    if min_expected < 5.0 {
        computed.warnings.push(warning(
            "expected_count_lt_5",
            WarningSeverity::Warn,
            if two_by_two {
                format!(
                    "an expected count is {} < 5: the χ² approximation is poor; use fisher",
                    apa::number(min_expected, 2)
                )
            } else {
                format!(
                    "an expected count is {} < 5: the χ² approximation is poor; merge sparse categories",
                    apa::number(min_expected, 2)
                )
            },
        ));
    }
}

/// Sample odds ratio with Woolf's logit interval; Haldane's +0.5 when a
/// cell is zero.
pub fn odds_ratio(a: f64, b: f64, c: f64, d: f64, level: f64) -> (f64, f64, f64, &'static str) {
    let (a, b, c, d, method) = if a * b * c * d == 0.0 {
        (a + 0.5, b + 0.5, c + 0.5, d + 0.5, "woolf_logit_haldane")
    } else {
        (a, b, c, d, "woolf_logit")
    };
    let or = a * d / (b * c);
    let se = (1.0 / a + 1.0 / b + 1.0 / c + 1.0 / d).sqrt();
    let z = qnorm(1.0 - (1.0 - level) / 2.0);
    (
        or,
        (or.ln() - z * se).exp(),
        (or.ln() + z * se).exp(),
        method,
    )
}

/// Fisher's exact p for a 2×2 table of whole counts (R's two-sided rule:
/// tables no more probable than the observed one, relative tolerance
/// 1e-7).
pub fn fisher_p(observed: &[Vec<f64>], alternative: Alternative) -> Option<f64> {
    let whole = |x: f64| -> Option<u64> {
        // Counts are checked whole and non-negative before the cast.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        ((x >= 0.0 && x.fract() == 0.0 && x < 1e12).then_some(x as u64))
    };
    let a = whole(observed[0][0])?;
    let b = whole(observed[0][1])?;
    let c = whole(observed[1][0])?;
    let d = whole(observed[1][1])?;
    let (r1, c1, n) = (a + b, a + c, a + b + c + d);
    let lo = c1.saturating_sub(n - r1);
    let hi = r1.min(c1);
    let ln_p = |k: u64| ln_binomial(r1, k) + ln_binomial(n - r1, c1 - k) - ln_binomial(n, c1);
    let probs: Vec<(u64, f64)> = (lo..=hi).map(|k| (k, ln_p(k).exp())).collect();
    let observed_p = ln_p(a).exp();
    let p = match alternative {
        Alternative::Less => probs.iter().filter(|(k, _)| *k <= a).map(|(_, p)| p).sum(),
        Alternative::Greater => probs.iter().filter(|(k, _)| *k >= a).map(|(_, p)| p).sum(),
        Alternative::TwoSided => probs
            .iter()
            .filter(|(_, p)| *p <= observed_p * (1.0 + 1e-7))
            .map(|(_, p)| p)
            .sum::<f64>(),
    };
    Some(p.min(1.0))
}

fn level_py(level: &str, numeric: bool) -> String {
    if numeric {
        level.to_owned()
    } else {
        py_str(level)
    }
}

fn level_r(level: &str, numeric: bool) -> String {
    if numeric {
        level.to_owned()
    } else {
        crate::reproduce::r_str(level)
    }
}

fn reproduce_two(
    computed: &mut Computed,
    request: &TestRequest,
    kind: TestKind,
    names: &[String; 2],
    source: &TwoSource,
    used: &[DataColumn],
) {
    let (py_a, py_b, r_a, r_b) = match source {
        TwoSource::Grouped {
            y,
            group,
            numeric_group,
        } => {
            let pick_py = |level: &str| {
                format!(
                    "{}[{} == {}].dropna()",
                    py_col(y),
                    py_col(group),
                    level_py(level, *numeric_group)
                )
            };
            let pick_r = |level: &str| {
                format!(
                    "{}[{} == {}]",
                    r_col(y),
                    r_col(group),
                    level_r(level, *numeric_group)
                )
            };
            (
                pick_py(&names[0]),
                pick_py(&names[1]),
                pick_r(&names[0]),
                pick_r(&names[1]),
            )
        }
        TwoSource::Columns => (
            format!("{}.dropna()", py_col(&names[0])),
            format!("{}.dropna()", py_col(&names[1])),
            r_col(&names[0]),
            r_col(&names[1]),
        ),
    };
    let alt_py = alt_py(request.alternative);
    let alt_r = alt_r(request.alternative);
    let mu = request.mu;
    let py_frame = py_frame(&request.data, used);
    let r_frame = r_frame(&request.data, used);
    let (py, r) = match kind {
        TestKind::MannWhitney => {
            let method = if computed.method.params.get("p_method") == Some(&Value::from("exact")) {
                "exact"
            } else {
                "asymptotic"
            };
            (
                format!(
                    "import scipy.stats as st; {py_frame}; st.mannwhitneyu({py_a}, {py_b}, alternative='{alt_py}', use_continuity={}, method='{method}')",
                    if request.correct { "True" } else { "False" }
                ),
                format!(
                    "{r_frame}; wilcox.test({r_a}, {r_b}, alternative = \"{alt_r}\", correct = {}, exact = {})",
                    r_bool(request.correct),
                    r_bool(method == "exact")
                ),
            )
        }
        _ => {
            let welch = kind == TestKind::WelchT;
            (
                format!(
                    "import scipy.stats as st; {py_frame}; st.ttest_ind({py_a}, {py_b}, equal_var={}, alternative='{alt_py}')",
                    if welch { "False" } else { "True" }
                ),
                format!(
                    "{r_frame}; t.test({r_a}, {r_b}, var.equal = {}, alternative = \"{alt_r}\", mu = {mu}, conf.level = {})",
                    r_bool(!welch),
                    request.conf_level
                ),
            )
        }
    };
    computed.reproduce.python = Some(py);
    computed.reproduce.r = Some(r);
}

fn r_bool(b: bool) -> &'static str {
    if b { "TRUE" } else { "FALSE" }
}

fn reproduce_paired(
    computed: &mut Computed,
    request: &TestRequest,
    kind: TestKind,
    names: &[String; 2],
    used: &[DataColumn],
) {
    let alt_py = alt_py(request.alternative);
    let alt_r = alt_r(request.alternative);
    let py_frame = py_frame(&request.data, used);
    let r_frame = r_frame(&request.data, used);
    let pair = format!(
        "p = df[[{}, {}]].dropna()",
        py_str(&names[0]),
        py_str(&names[1])
    );
    let (x, y) = (
        format!("p[{}] - {}", py_str(&names[0]), request.mu),
        format!("p[{}]", py_str(&names[1])),
    );
    let (py, r) = if kind == TestKind::Wilcoxon {
        (
            format!(
                "import scipy.stats as st; {py_frame}; {pair}; st.wilcoxon({x}, {y}, alternative='{alt_py}', correction={})",
                if request.correct { "True" } else { "False" }
            ),
            format!(
                "{r_frame}; wilcox.test({}, {}, paired = TRUE, mu = {}, alternative = \"{alt_r}\", correct = {})",
                r_col(&names[0]),
                r_col(&names[1]),
                request.mu,
                r_bool(request.correct)
            ),
        )
    } else {
        (
            format!(
                "import scipy.stats as st; {py_frame}; {pair}; st.ttest_rel({x}, {y}, alternative='{alt_py}')"
            ),
            format!(
                "{r_frame}; t.test({}, {}, paired = TRUE, mu = {}, alternative = \"{alt_r}\", conf.level = {})",
                r_col(&names[0]),
                r_col(&names[1]),
                request.mu,
                request.conf_level
            ),
        )
    };
    computed.reproduce.python = Some(py);
    computed.reproduce.r = Some(r);
}

fn reproduce_one(
    computed: &mut Computed,
    request: &TestRequest,
    kind: TestKind,
    name: &str,
    used: &[DataColumn],
) {
    let alt_py = alt_py(request.alternative);
    let alt_r = alt_r(request.alternative);
    let py_frame = py_frame(&request.data, used);
    let r_frame = r_frame(&request.data, used);
    let x = format!("{}.dropna()", py_col(name));
    let (py, r) = if kind == TestKind::Wilcoxon {
        (
            format!(
                "import scipy.stats as st; {py_frame}; st.wilcoxon({x} - {}, alternative='{alt_py}')",
                request.mu
            ),
            format!(
                "{r_frame}; wilcox.test({}, mu = {}, alternative = \"{alt_r}\", correct = {})",
                r_col(name),
                request.mu,
                r_bool(request.correct)
            ),
        )
    } else {
        (
            format!(
                "import scipy.stats as st; {py_frame}; st.ttest_1samp({x}, popmean={}, alternative='{alt_py}')",
                request.mu
            ),
            format!(
                "{r_frame}; t.test({}, mu = {}, alternative = \"{alt_r}\", conf.level = {})",
                r_col(name),
                request.mu,
                request.conf_level
            ),
        )
    };
    computed.reproduce.python = Some(py);
    computed.reproduce.r = Some(r);
}

fn reproduce_table(
    computed: &mut Computed,
    request: &TestRequest,
    kind: TestKind,
    observed: &[Vec<f64>],
    source: Option<&(String, String)>,
    used: &[DataColumn],
) {
    let rows: Vec<String> = observed
        .iter()
        .map(|r| {
            let cells: Vec<String> = r.iter().map(|x| format!("{x}")).collect();
            format!("[{}]", cells.join(", "))
        })
        .collect();
    let (py_table, r_table) = match source {
        Some((y, g)) => (
            format!(
                "{}; t = pd.crosstab({}, {})",
                py_frame(&request.data, used),
                py_col(y),
                py_col(g)
            ),
            format!(
                "{}; t <- table({}, {})",
                r_frame(&request.data, used),
                r_col(y),
                r_col(g)
            ),
        ),
        None => {
            let cells: Vec<String> = observed.iter().flatten().map(|x| format!("{x}")).collect();
            (
                format!("t = [{}]", rows.join(", ")),
                format!(
                    "t <- matrix(c({}), nrow = {}, byrow = TRUE)",
                    cells.join(", "),
                    observed.len()
                ),
            )
        }
    };
    let (py, r) = if kind == TestKind::Fisher {
        (
            format!(
                "import scipy.stats as st; {py_table}; st.fisher_exact(t, alternative='{}')",
                alt_py(request.alternative)
            ),
            format!(
                "{r_table}; fisher.test(t, alternative = \"{}\")",
                alt_r(request.alternative)
            ),
        )
    } else {
        (
            format!(
                "import scipy.stats as st; {py_table}; st.chi2_contingency(t, correction={})",
                if request.correct { "True" } else { "False" }
            ),
            format!(
                "{r_table}; chisq.test(t, correct = {})",
                r_bool(request.correct)
            ),
        )
    };
    computed.reproduce.python = Some(py);
    computed.reproduce.r = Some(r);
}
