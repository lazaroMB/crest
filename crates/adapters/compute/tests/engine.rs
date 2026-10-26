//! Envelope behaviour: the automatic choice, assumption checks, misuse
//! warnings, error envelopes, hashes, provenance and recompute.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use adapter_compute::StatsEngine;
use application::ports::compute::{
    ComputeError, ComputePort, CorrelateRequest, DataColumn, DataSet, DescribeRequest,
    RegressRequest, TestChoice, TestRequest,
};
use domain::computed::{
    AssumptionStatus, Computed, ComputedBody, ComputedStatus, CorrelationMethod, PAdjust, TestKind,
};

const SLEEP_1: [f64; 10] = [0.7, -1.6, -0.2, -1.2, -0.1, 3.4, 3.7, 0.8, 0.0, 2.0];
const SLEEP_2: [f64; 10] = [1.9, 0.8, 1.1, 0.1, -0.1, 4.4, 5.5, 1.6, 4.6, 3.4];

fn fixed_engine() -> StatsEngine {
    StatsEngine::with_clock(Arc::new(|| UNIX_EPOCH + Duration::from_secs(1_792_860_000)))
}

fn two_columns(a: &[f64], b: &[f64]) -> DataSet {
    DataSet::inline(vec![
        DataColumn::numeric("a", a),
        DataColumn::numeric("b", b),
    ])
}

fn two_sample(a: &[f64], b: &[f64], test: TestChoice) -> Computed {
    let request = TestRequest {
        data: two_columns(a, b),
        test,
        y: Some("a".into()),
        y2: Some("b".into()),
        ..TestRequest::default()
    };
    let (c, _) = fixed_engine().test(&request).unwrap();
    assert_eq!(c.validate(), Ok(()));
    c
}

#[test]
fn auto_two_sample_records_why_and_checks() {
    let c = two_sample(&SLEEP_1, &SLEEP_2, TestChoice::Auto);
    assert!(c.kind.starts_with("stats.ttest_"), "{}", c.kind);
    assert!(c.method.why.is_some());
    assert!(!c.method.alternatives.is_empty());
    let ids: Vec<&str> = c.assumptions.iter().map(|a| a.id.as_str()).collect();
    assert!(ids.contains(&"normality_a") && ids.contains(&"normality_b"));
    let independence = c
        .assumptions
        .iter()
        .find(|a| a.id == "independence")
        .unwrap();
    assert_eq!(independence.status, AssumptionStatus::NotChecked);
    // n = 10 per group.
    assert!(c.warnings.iter().any(|w| w.code == "small_n"));
    assert_eq!(c.status, ComputedStatus::Warning);
    // effectsize::cohens_d(extra ~ group, data = sleep):
    // d = -0.83, 95% CI [-1.74, 0.10].
    let d = c.value("cohens_d").unwrap();
    assert_eq!(d.display.as_deref(), Some("−0.83"));
    let ci = d.ci.clone().unwrap();
    assert_eq!(ci.method.as_deref(), Some("noncentral_t"));
    assert!(
        (ci.lo + 1.74).abs() < 0.005 && (ci.hi - 0.10).abs() < 0.005,
        "{ci:?}"
    );
    assert_eq!(d.label.as_deref(), Some("large"));
    let narrative = c.narrative.clone().unwrap();
    assert_eq!(narrative.template, "apa_ttest");
}

#[test]
fn auto_picks_a_rank_test_for_skewed_small_samples() {
    let skewed: Vec<f64> = (1..=15).map(|i| (f64::from(i) / 2.0).exp()).collect();
    let other: Vec<f64> = (1..=15).map(|i| f64::from(i) * 10.0).collect();
    let c = two_sample(&skewed, &other, TestChoice::Auto);
    assert_eq!(c.kind, "stats.mann_whitney");
    assert!(c.method.why.as_deref().unwrap().contains("Shapiro-Wilk"));
    assert!(c.value("rank_biserial").is_some());
    let normality = c
        .assumptions
        .iter()
        .find(|a| a.id == "normality_a")
        .unwrap();
    assert_eq!(normality.status, AssumptionStatus::Fail);
    assert!(normality.consequence.is_some());
}

#[test]
fn forcing_a_t_test_on_non_normal_data_warns() {
    let skewed: Vec<f64> = (1..=15).map(|i| (f64::from(i) / 2.0).exp()).collect();
    let other: Vec<f64> = (1..=15).map(|i| f64::from(i) * 10.0).collect();
    let c = two_sample(&skewed, &other, TestChoice::StudentT);
    let w = c.warnings.iter().find(|w| w.code == "small_n").unwrap();
    assert!(w.text.contains("non-normal"));
}

#[test]
fn small_tables_switch_to_fisher_and_chi_square_warns() {
    let table = vec![vec![3.0, 1.0], vec![1.0, 3.0]];
    let request = TestRequest {
        table: Some(table.clone()),
        ..TestRequest::default()
    };
    let (c, body) = fixed_engine().test(&request).unwrap();
    assert_eq!(c.kind, "stats.fisher");
    assert!(c.method.why.as_deref().unwrap().contains("< 5"));
    let ComputedBody::Test(body) = body else {
        panic!()
    };
    assert_eq!(body.test, TestKind::Fisher);
    assert_eq!(body.expected, Some(vec![vec![2.0, 2.0], vec![2.0, 2.0]]));
    let forced = TestRequest {
        test: TestChoice::ChiSquare,
        table: Some(table),
        ..TestRequest::default()
    };
    let (c, _) = fixed_engine().test(&forced).unwrap();
    let w = c
        .warnings
        .iter()
        .find(|w| w.code == "expected_count_lt_5")
        .unwrap();
    assert!(w.text.contains("fisher"));
    assert!(c.method.name.contains("Yates"));
}

#[test]
fn contingency_from_categorical_columns() {
    let a = ["x", "x", "y", "y", "x", "y", "x", "y"];
    let b = ["u", "v", "u", "v", "u", "v", "u", "v"];
    let request = TestRequest {
        data: DataSet::inline(vec![
            DataColumn::categorical("a", &a),
            DataColumn::categorical("b", &b),
        ]),
        y: Some("a".into()),
        group: Some("b".into()),
        test: TestChoice::ChiSquare,
        ..TestRequest::default()
    };
    let (c, body) = fixed_engine().test(&request).unwrap();
    let ComputedBody::Test(body) = body else {
        panic!()
    };
    assert_eq!(body.observed, Some(vec![vec![3.0, 1.0], vec![1.0, 3.0]]));
    assert!(c.value("cramers_v").is_some());
    assert!(c.reproduce.r.as_deref().unwrap().contains("chisq.test(t"));
}

#[test]
fn degenerate_data_gives_error_envelopes() {
    let c = two_sample(&[1.0, 1.0, 1.0], &[2.0, 2.0, 2.0], TestChoice::WelchT);
    assert_eq!(c.status, ComputedStatus::Error);
    assert!(c.error.as_deref().unwrap().contains("constant"));
    assert!(!c.values.contains_key("statistic"));

    let request = RegressRequest {
        data: DataSet::inline(vec![
            DataColumn::numeric("y", &[1.0, 2.0, 3.0, 5.0, 4.0]),
            DataColumn::numeric("x1", &[1.0, 2.0, 3.0, 4.0, 5.0]),
            DataColumn::numeric("x2", &[2.0, 4.0, 6.0, 8.0, 10.0]),
        ]),
        y: "y".into(),
        x: vec!["x1".into(), "x2".into()],
        conf_level: 0.95,
    };
    let (c, _) = fixed_engine().regress(&request).unwrap();
    assert_eq!(c.status, ComputedStatus::Error);
    assert!(
        c.error.as_deref().unwrap().contains("collinear"),
        "{:?}",
        c.error
    );

    let mut constant = request.clone();
    constant.x = vec!["x1".into()];
    constant.data.columns[0] = DataColumn::numeric("y", &[3.0; 5]);
    let (c, _) = fixed_engine().regress(&constant).unwrap();
    assert_eq!(c.status, ComputedStatus::Error);
    assert!(c.error.as_deref().unwrap().contains("constant"));
}

#[test]
fn bad_requests_are_errors_of_the_call() {
    let request = TestRequest {
        data: two_columns(&SLEEP_1, &SLEEP_2),
        y: Some("nope".into()),
        ..TestRequest::default()
    };
    assert!(matches!(
        fixed_engine().test(&request),
        Err(ComputeError::Invalid(text)) if text.contains("unknown column")
    ));
    let request = TestRequest {
        data: two_columns(&SLEEP_1, &SLEEP_2),
        test: TestChoice::Fisher,
        y: Some("a".into()),
        y2: Some("b".into()),
        ..TestRequest::default()
    };
    // Fisher on numeric columns treats them as categories: 2×2 is needed.
    assert!(fixed_engine().test(&request).is_err());
}

#[test]
fn correlations_without_adjustment_warn() {
    let x: Vec<f64> = (0..25).map(f64::from).collect();
    let y: Vec<f64> = x.iter().map(|v| (v * 0.7).sin() + v / 10.0).collect();
    let z: Vec<f64> = x.iter().map(|v| (v * 1.3).cos()).collect();
    let data = DataSet::inline(vec![
        DataColumn::numeric("x", &x),
        DataColumn::numeric("y", &y),
        DataColumn::numeric("z", &z),
    ]);
    let request = CorrelateRequest {
        data,
        variables: Vec::new(),
        method: CorrelationMethod::Spearman,
        adjustment: PAdjust::None,
        conf_level: 0.95,
    };
    let (c, body) = fixed_engine().correlate(&request).unwrap();
    assert!(c.warnings.iter().any(|w| w.code == "multiple_comparisons"));
    assert_eq!(body.validate(), Ok(()));
    assert!(c.value("r.x.y").is_some() && c.value("p.y.z").is_some());
    let adjusted = CorrelateRequest {
        adjustment: PAdjust::Holm,
        ..request
    };
    let (c2, _) = fixed_engine().correlate(&adjusted).unwrap();
    assert!(!c2.warnings.iter().any(|w| w.code == "multiple_comparisons"));
    assert!(c2.value("p.x.y").unwrap().v >= c.value("p.x.y").unwrap().v);
}

#[test]
fn describe_by_group_and_categorical_levels() {
    let values: Vec<f64> = SLEEP_1.iter().chain(&SLEEP_2).copied().collect();
    let group: Vec<&str> = (0..20).map(|i| if i < 10 { "a" } else { "b" }).collect();
    let request = DescribeRequest {
        data: DataSet::inline(vec![
            DataColumn::numeric("extra", &values),
            DataColumn::categorical("group", &group),
        ]),
        variables: Vec::new(),
        by: Some("group".into()),
    };
    let (c, body) = fixed_engine().describe(&request).unwrap();
    assert_eq!(c.value("extra.a.mean").unwrap().v, 0.75);
    assert!((c.value("extra.b.mean").unwrap().v - 2.33).abs() < 1e-12);
    assert_eq!(body.validate(), Ok(()));
    let request = DescribeRequest {
        data: DataSet::inline(vec![DataColumn::categorical("group", &group)]),
        variables: Vec::new(),
        by: None,
    };
    let (c, body) = fixed_engine().describe(&request).unwrap();
    assert_eq!(c.value("group.a.count").unwrap().v, 10.0);
    assert_eq!(c.value("group.b.percent").unwrap().display_text(), "50.0 %");
    let ComputedBody::Descriptive(body) = body else {
        panic!()
    };
    assert_eq!(body.variables[0].levels.len(), 2);
}

#[test]
fn envelopes_carry_engine_hashes_reproduce_and_provenance() {
    let c = two_sample(&SLEEP_1, &SLEEP_2, TestChoice::WelchT);
    assert_eq!(c.engine.name, "crest-stats");
    assert_eq!(c.engine.version, env!("CARGO_PKG_VERSION"));
    assert_eq!(c.engine.backend.as_deref(), Some("statrs 0.19.1"));
    assert!(c.engine.platform.is_some());
    assert!(c.id.starts_with("st_"));
    let input = &c.inputs[0];
    assert!(input.hash.as_deref().unwrap().starts_with("blake3:"));
    assert_eq!(input.columns, vec!["a", "b"]);
    assert_eq!(input.rows, Some(10));
    let other = two_sample(&SLEEP_2, &SLEEP_1, TestChoice::WelchT);
    assert_ne!(input.hash, other.inputs[0].hash);
    assert_ne!(c.id, other.id);
    assert_eq!(
        c.provenance.activity_started.as_deref(),
        Some("2026-10-24T16:40:00Z")
    );
    assert_eq!(c.provenance.duration_ms, Some(0));
    assert!(c.environment_hash.is_some());
    let lua = c.reproduce.lua.as_deref().unwrap();
    assert!(lua.starts_with("app.compute.test{"), "{lua}");
    assert!(c.reproduce.python.as_deref().unwrap().contains("ttest_ind"));
    assert!(
        c.reproduce
            .python
            .as_deref()
            .unwrap()
            .contains("equal_var=False")
    );
    assert!(
        c.reproduce
            .r
            .as_deref()
            .unwrap()
            .contains("t.test(df$a, df$b")
    );
    assert_eq!(c.value("p_value").unwrap().display.as_deref(), Some(".079"));
}

#[test]
fn recompute_matches_and_detects_tampering() {
    let engine = fixed_engine();
    let original = two_sample(&SLEEP_1, &SLEEP_2, TestChoice::Auto);
    let (again, _) = engine.recompute(&original).unwrap();
    assert_eq!(again.id, original.id);
    assert_eq!(again.verification.matched, Some(true));
    assert!(again.verification.diff.is_empty());
    assert_eq!(
        again.verification.recomputed_at.as_deref(),
        Some("2026-10-24T16:40:00Z")
    );
    let mut tampered = original.clone();
    tampered.values.get_mut("statistic").unwrap().v = -2.5;
    let (again, _) = engine.recompute(&tampered).unwrap();
    assert_eq!(again.verification.matched, Some(false));
    assert_eq!(again.verification.diff[0].key, "statistic");

    let mut foreign = original;
    foreign.method.params.clear();
    assert!(matches!(
        engine.recompute(&foreign),
        Err(ComputeError::NotRecomputable(_))
    ));
}

#[test]
fn regression_fills_the_footer() {
    let x: Vec<f64> = (0..30).map(f64::from).collect();
    let g: Vec<&str> = (0..30)
        .map(|i| if i % 3 == 0 { "c" } else { "t" })
        .collect();
    let y: Vec<f64> = x
        .iter()
        .zip(&g)
        .map(|(x, g)| 2.0 + 0.5 * x + if *g == "t" { 1.5 } else { 0.0 } + (x * 1.7).sin())
        .collect();
    let request = RegressRequest {
        data: DataSet::inline(vec![
            DataColumn::numeric("y", &y),
            DataColumn::numeric("x", &x),
            DataColumn::categorical("g", &g),
        ]),
        y: "y".into(),
        x: vec!["x".into(), "g".into()],
        conf_level: 0.95,
    };
    let (c, body) = fixed_engine().regress(&request).unwrap();
    assert_eq!(c.validate(), Ok(()));
    let ComputedBody::Regression(r) = body else {
        panic!()
    };
    let terms: Vec<&str> = r.coefficients.iter().map(|c| c.term.as_str()).collect();
    assert_eq!(terms, ["(Intercept)", "x", "g[T.t]"]);
    assert!((r.coefficients[1].estimate - 0.5).abs() < 0.05);
    let d = &r.diagnostics;
    assert!(d.durbin_watson.is_some() && d.jarque_bera_p.is_some() && d.breusch_pagan_p.is_some());
    assert!(d.max_vif.unwrap() >= 1.0 && d.max_cooks_d.unwrap() > 0.0);
    assert_eq!(r.fitted.len(), 30);
    assert!(c.value("coef.g_T_t_").is_some());
    assert!(
        c.reproduce
            .python
            .as_deref()
            .unwrap()
            .contains("smf.ols('y ~ x + C(g)'")
    );
    assert!(
        c.reproduce
            .r
            .as_deref()
            .unwrap()
            .contains("lm(y ~ x + g, data = df)")
    );
}

#[test]
fn one_sample_and_signed_rank() {
    let x = [5.1, 4.9, 5.6, 5.8, 6.0, 5.4, 5.3, 6.1, 5.7, 5.5];
    let request = TestRequest {
        data: DataSet::inline(vec![DataColumn::numeric("x", &x)]),
        y: Some("x".into()),
        mu: 5.0,
        test: TestChoice::OneSampleT,
        ..TestRequest::default()
    };
    let (c, _) = fixed_engine().test(&request).unwrap();
    assert_eq!(c.kind, "stats.ttest_one_sample");
    assert!(c.value("p_value").unwrap().v < 0.01);
    let wilcoxon = TestRequest {
        test: TestChoice::Wilcoxon,
        ..request
    };
    let (c, _) = fixed_engine().test(&wilcoxon).unwrap();
    assert_eq!(c.kind, "stats.wilcoxon");
    // |x − 5| ties at 0.1 (4.9 and 5.1): the negative difference has rank
    // 1.5, so V = 55 − 1.5, from the normal approximation.
    assert_eq!(c.value("statistic").unwrap().v, 53.5);
    assert_eq!(c.method.params["p_method"], "normal_approx");
}
