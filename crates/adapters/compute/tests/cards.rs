//! Reference values of the phase-2 cards, from R's documentation and
//! textbook examples (printed digits), and the properties each card
//! promises (shared histogram edges, seeded reproducibility, plug-back).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use adapter_compute::StatsEngine;
use application::ports::compute::*;
use domain::computed::*;

/// Asserts `actual` rounds to `printed` at the printed digits.
#[track_caller]
fn printed(actual: f64, printed: &str) {
    let expected: f64 = printed.parse().unwrap();
    let decimals = printed.split('.').nth(1).map_or(0, str::len);
    let half = 0.5 * 10f64.powi(-i32::try_from(decimals).unwrap());
    assert!(
        (actual - expected).abs() <= half * (1.0 + 1e-9),
        "{actual} does not round to {printed}"
    );
}

/// Asserts `|actual − expected| ≤ tol`.
#[track_caller]
fn near(actual: f64, expected: f64, tol: f64) {
    assert!(
        (actual - expected).abs() <= tol,
        "{actual} vs {expected} (tolerance {tol})"
    );
}

fn v(c: &Computed, key: &str) -> f64 {
    c.value(key)
        .unwrap_or_else(|| panic!("no value {key:?} in {:?}", c.values.keys()))
        .v
}

fn engine() -> StatsEngine {
    StatsEngine::new()
}

/// `mtcars$mpg`.
const MPG: [f64; 32] = [
    21.0, 21.0, 22.8, 21.4, 18.7, 18.1, 14.3, 24.4, 22.8, 19.2, 17.8, 16.4, 17.3, 15.2, 10.4, 10.4,
    14.7, 32.4, 30.4, 33.9, 21.5, 15.5, 15.2, 13.3, 19.2, 27.3, 26.0, 30.4, 15.8, 19.7, 15.0, 21.4,
];

/// `mtcars$am`.
const AM: [f64; 32] = [
    1.0, 1.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0,
    1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0,
];

fn distribution(by: bool) -> DistributionRequest {
    let mut columns = vec![DataColumn::numeric("mpg", &MPG)];
    if by {
        columns.push(DataColumn::numeric("am", &AM));
    }
    DistributionRequest {
        data: DataSet::inline(columns),
        variable: "mpg".into(),
        by: by.then(|| "am".into()),
        bin_rule: BinRule::FreedmanDiaconis,
        bins: None,
        bandwidth_rule: BandwidthRule::Silverman,
        bandwidth: None,
        conf_level: 0.95,
    }
}

// --- Distribution ------------------------------------------------------------

#[test]
fn distribution_matches_r_density_boxplot_and_nclass() {
    let (c, body) = engine().distribution(&distribution(false)).unwrap();
    assert_eq!(c.status, ComputedStatus::Ok, "{:?}", c.warnings);
    // density(mtcars$mpg): "Bandwidth 'bw' = 2.477".
    printed(v(&c, "bandwidth"), "2.477");
    // boxplot.stats(mtcars$mpg)$stats: 10.40 15.35 19.20 22.80 33.90, no $out.
    let ComputedBody::Distribution(d) = &body else {
        panic!("not a distribution: {body:?}");
    };
    let b = &d.groups[0].boxplot;
    assert_eq!(
        [
            b.whisker_lo,
            b.lower_hinge,
            b.median,
            b.upper_hinge,
            b.whisker_hi
        ],
        [10.4, 15.35, 19.2, 22.8, 33.9]
    );
    assert!(b.outliers.is_empty());
    assert_eq!(v(&c, "outliers"), 0.0);
    // nclass.FD(mtcars$mpg) = 6; IQR(mtcars$mpg) = 7.375.
    assert_eq!(v(&c, "bins"), 6.0);
    near(v(&c, "iqr"), 7.375, 1e-12);
    near(d.bin_width, 23.5 / 6.0, 1e-12);
    assert_eq!(d.groups[0].histogram.counts.iter().sum::<u64>(), 32);
    // shapiro.test(mtcars$mpg): W = 0.94756, p-value = 0.1229.
    printed(v(&c, "shapiro_w"), "0.94756");
    printed(v(&c, "shapiro_p"), "0.1229");
    // The ECDF ends at 1 and the DKW band is sqrt(log(2/0.05) / 64).
    assert_eq!(d.groups[0].ecdf.last().map(|s| s.p), Some(1.0));
    near(v(&c, "dkw_epsilon"), (40f64.ln() / 64.0).sqrt(), 1e-12);
    let sentence = domain::computed::narrative::sentence(&c, &body).unwrap();
    assert!(
        sentence.starts_with("mpg (n = 32) has median 19.20"),
        "{sentence}"
    );
    assert!(
        c.reproduce
            .r
            .as_deref()
            .unwrap()
            .contains("hist(x, breaks = \"FD\")")
    );
}

#[test]
fn distribution_groups_share_edges_and_prefix_their_keys() {
    let (c, body) = engine().distribution(&distribution(true)).unwrap();
    let ComputedBody::Distribution(d) = &body else {
        panic!("not a distribution");
    };
    assert_eq!(d.groups.len(), 2);
    assert_eq!(d.groups[0].histogram.edges, d.groups[1].histogram.edges);
    assert_eq!(body.validate(), Ok(()));
    // tapply(mtcars$mpg, mtcars$am, median): 17.3 and 22.8.
    near(v(&c, "0.median"), 17.3, 1e-12);
    near(v(&c, "1.median"), 22.8, 1e-12);
    assert_eq!(v(&c, "0.n") + v(&c, "1.n"), 32.0);
    assert_eq!(c.assumptions.len(), 2);

    let mut fixed = distribution(false);
    fixed.bins = Some(10);
    fixed.bandwidth = Some(1.5);
    let (c, body) = engine().distribution(&fixed).unwrap();
    let ComputedBody::Distribution(d) = &body else {
        panic!("not a distribution");
    };
    assert_eq!(d.bin_rule, BinRule::Fixed);
    assert_eq!(d.bandwidth_rule, BandwidthRule::Fixed);
    assert_eq!(v(&c, "bins"), 10.0);
    assert_eq!(v(&c, "bandwidth"), 1.5);

    let mut bad = distribution(false);
    bad.bins = Some(0);
    assert!(engine().distribution(&bad).is_err());
    bad.bins = None;
    bad.variable = "nope".into();
    assert!(engine().distribution(&bad).is_err());
}

// --- Contingency ---------------------------------------------------------------

fn table(cells: &[&[f64]]) -> ContingencyRequest {
    ContingencyRequest {
        data: DataSet::default(),
        row: None,
        column: None,
        table: Some(cells.iter().map(|r| r.to_vec()).collect()),
        row_labels: Vec::new(),
        column_labels: Vec::new(),
        test: ContingencyChoice::Auto,
        correct: true,
        alternative: Alternative::TwoSided,
        conf_level: 0.95,
        seed: DEFAULT_SEED,
        simulations: 10_000,
    }
}

fn contingency(body: &ComputedBody) -> &ContingencyBody {
    match body {
        ComputedBody::Contingency(c) => c,
        other => panic!("not a contingency table: {other:?}"),
    }
}

#[test]
fn contingency_matches_chisq_test_and_its_stdres() {
    // ?chisq.test, Agresti (2007): party identification by gender.
    let mut request = table(&[&[762.0, 327.0, 468.0], &[484.0, 239.0, 477.0]]);
    request.row_labels = vec!["F".into(), "M".into()];
    request.column_labels = vec!["Democrat".into(), "Independent".into(), "Republican".into()];
    let (c, body) = engine().contingency(&request).unwrap();
    // X-squared = 30.07, df = 2, p-value = 2.954e-07.
    printed(v(&c, "statistic"), "30.07");
    assert_eq!(v(&c, "df"), 2.0);
    near(v(&c, "p_value"), 2.954e-07, 5e-11);
    let t = contingency(&body);
    assert_eq!(t.test, ContingencyTest::ChiSquare);
    // chisq.test(M)$stdres.
    for (actual, expected) in
        t.std_residuals[0]
            .iter()
            .zip(["4.5020535", "0.6994517", "-5.3159455"])
    {
        printed(*actual, expected);
    }
    printed(t.std_residuals[1][2], "5.3159455");
    printed(v(&c, "max_residual"), "-5.32");
    assert_eq!(
        c.method.why.as_deref(),
        Some("every expected count is at least 5")
    );
    let sentence = domain::computed::narrative::sentence(&c, &body).unwrap();
    assert!(sentence.contains("F × Republican"), "{sentence}");
}

#[test]
fn contingency_switches_to_fisher_and_matches_fisher_test() {
    // ?fisher.test: TeaTasting, alternative = "greater": p-value = 0.2429,
    // odds ratio 6.408309, 95 percent CI 0.3135693 to Inf.
    let mut request = table(&[&[3.0, 1.0], &[1.0, 3.0]]);
    request.alternative = Alternative::Greater;
    let (c, body) = engine().contingency(&request).unwrap();
    assert_eq!(contingency(&body).test, ContingencyTest::FisherExact);
    assert!(c.method.why.as_deref().unwrap().contains("< 5"));
    printed(v(&c, "p_value"), "0.2429");
    near(v(&c, "odds_ratio"), 6.408_309, 1e-4);
    assert!(
        c.value("odds_ratio").unwrap().ci.is_none(),
        "an infinite bound has no interval"
    );
    let bounds = c.method.params["odds_ratio_interval"].as_str().unwrap();
    assert!(bounds.starts_with("[0.313"), "{bounds}");
    // Two-sided: p-value = 0.4857.
    request.alternative = Alternative::TwoSided;
    let (c, _) = engine().contingency(&request).unwrap();
    printed(v(&c, "p_value"), "0.4857");

    // Convictions, alternative = "less": p-value = 0.0004652, odds ratio
    // 0.04693661, CI 0 to 0.2849601.
    let mut request = table(&[&[2.0, 15.0], &[10.0, 3.0]]);
    request.alternative = Alternative::Less;
    request.test = ContingencyChoice::Fisher;
    let (c, _) = engine().contingency(&request).unwrap();
    printed(v(&c, "p_value"), "0.0004652");
    near(v(&c, "odds_ratio"), 0.046_936_61, 1e-5);
    near(
        c.value("odds_ratio")
            .unwrap()
            .ci
            .as_ref()
            .map_or(0.0, |ci| ci.hi),
        0.284_960_1,
        1e-4,
    );

    // Job satisfaction by income, 4×4 (n = 96): p-value = 0.7827.
    let job = table(&[
        &[1.0, 3.0, 10.0, 6.0],
        &[2.0, 3.0, 10.0, 7.0],
        &[1.0, 6.0, 14.0, 12.0],
        &[0.0, 1.0, 9.0, 11.0],
    ]);
    let (c, body) = engine().contingency(&job).unwrap();
    assert_eq!(contingency(&body).test, ContingencyTest::FisherExact);
    printed(v(&c, "p_value"), "0.7827");
    assert_eq!(c.seed, None);
}

// --- Power -----------------------------------------------------------------------

fn power(test: PowerTest) -> PowerRequest {
    PowerRequest {
        test,
        n: None,
        effect: None,
        p1: None,
        p2: None,
        alpha: None,
        power: None,
        groups: None,
        alternative: Alternative::TwoSided,
        solve_for: None,
    }
}

#[test]
fn power_matches_pwr() {
    let solve = |request: PowerRequest, key: &str| {
        let (c, body) = engine().power(&request).unwrap();
        assert_eq!(body.validate(), Ok(()));
        assert_ne!(c.status, ComputedStatus::Error, "{:?}", c.error);
        v(&c, key)
    };
    // pwr.t.test(d = 0.8, power = 0.8): n = 25.52458 per group.
    let mut r = power(PowerTest::TwoSampleT);
    r.effect = Some(0.8);
    r.power = Some(0.8);
    near(solve(r.clone(), "n"), 25.524_571_8, 1e-6);
    assert_eq!(solve(r.clone(), "n_required"), 26.0);
    assert_eq!(solve(r.clone(), "n_total"), 52.0);
    // d = 0.5: n = 63.76561; d = 0.3: n = 175.38 (pwr prints 175.3851 at
    // uniroot's tolerance).
    r.effect = Some(0.5);
    near(solve(r.clone(), "n"), 63.765_610_2, 1e-6);
    r.effect = Some(0.3);
    near(solve(r.clone(), "n"), 175.384_669_5, 1e-5);
    // pwr.t.test(n = 20, d = 0.75): power = 0.6374.
    let mut r = power(PowerTest::TwoSampleT);
    r.n = Some(20.0);
    r.effect = Some(0.75);
    near(solve(r.clone(), "power"), 0.637_401_721, 1e-8);
    // n = 30, power = 0.8: d = 0.7356.
    r.n = Some(30.0);
    r.effect = None;
    r.power = Some(0.8);
    near(solve(r.clone(), "effect"), 0.735_621_07, 1e-7);
    // n = 25, d = 0.8, power = 0.8: alpha = 0.05343.
    r.n = Some(25.0);
    r.effect = Some(0.8);
    near(solve(r, "alpha"), 0.053_428_059, 1e-7);
    // pwr.t.test(d = 0.2, n = 60, sig.level = 0.10, type = "one.sample"):
    // power = 0.4555818.
    let mut r = power(PowerTest::OneSampleT);
    r.n = Some(60.0);
    r.effect = Some(0.2);
    r.alpha = Some(0.1);
    near(solve(r, "power"), 0.455_581_764, 1e-8);
    // Paired, greater, n = 30, d = 0.5: power = 0.8483.
    let mut r = power(PowerTest::PairedT);
    r.n = Some(30.0);
    r.effect = Some(0.5);
    r.alternative = Alternative::Greater;
    near(solve(r, "power"), 0.848_254_191, 1e-8);
}

#[test]
fn power_of_anova_and_proportions_matches_pwr() {
    let solve = |request: PowerRequest, key: &str| {
        let (c, _) = engine().power(&request).unwrap();
        v(&c, key)
    };
    // pwr.anova.test(f = 0.28, k = 4, n = 20): power = 0.5149793.
    let mut r = power(PowerTest::Anova);
    r.groups = Some(4);
    r.effect = Some(0.28);
    r.n = Some(20.0);
    near(solve(r.clone(), "power"), 0.514_979_292, 1e-8);
    // ... power = 0.80: n = 35.75789 per group.
    r.n = None;
    r.power = Some(0.8);
    near(solve(r, "n"), 35.757_894_5, 1e-6);
    // pwr.anova.test(k = 5, f = 0.25, power = 0.8): n = 39.1534.
    let mut r = power(PowerTest::Anova);
    r.groups = Some(5);
    r.effect = Some(0.25);
    r.power = Some(0.8);
    near(solve(r, "n"), 39.153_399_3, 1e-6);
    // pwr.2p.test(h = 0.3, n = 80): power = 0.4751.
    let mut r = power(PowerTest::TwoProportions);
    r.effect = Some(0.3);
    r.n = Some(80.0);
    near(solve(r, "power"), 0.475_100_871, 1e-8);
    // pwr.2p.test(h = ES.h(0.65, 0.45), power = 0.9, alternative =
    // "greater"): n = 104.4935.
    let mut r = power(PowerTest::TwoProportions);
    r.p1 = Some(0.65);
    r.p2 = Some(0.45);
    r.power = Some(0.9);
    r.alternative = Alternative::Greater;
    near(solve(r, "n"), 104.493_436, 1e-5);
    // pwr.p.test(h = 0.2, n = 60): power = 0.3408.
    let mut r = power(PowerTest::OneProportion);
    r.effect = Some(0.2);
    r.n = Some(60.0);
    near(solve(r, "power"), 0.340_845_124, 1e-8);

    // Two missing quantities, or none, are refused.
    let mut r = power(PowerTest::TwoSampleT);
    r.effect = Some(0.5);
    assert!(engine().power(&r).is_err());
    r.n = Some(10.0);
    r.power = Some(0.8);
    r.alpha = Some(0.05);
    assert!(engine().power(&r).is_err());
    let anova = power(PowerTest::Anova);
    assert!(engine().power(&anova).is_err());
}

// --- A/B test ------------------------------------------------------------------

fn ab(xa: u64, na: u64, xb: u64, nb: u64) -> AbTestRequest {
    let arm = |conversions, visitors| ArmCounts {
        name: None,
        visitors,
        conversions,
    };
    AbTestRequest {
        control: arm(xa, na),
        treatment: arm(xb, nb),
        expected_split: 0.5,
        conf_level: 0.95,
        alternative: Alternative::TwoSided,
        looks: 1,
        planned_visitors: None,
        minimum_lift: None,
    }
}

#[test]
fn ab_test_matches_prop_test_newcombe_and_srm() {
    // Newcombe (1998) example (a): 56/70 vs 48/80.
    let (c, body) = engine().ab_test(&ab(48, 80, 56, 70)).unwrap();
    let ComputedBody::AbTest(t) = &body else {
        panic!("not an A/B test");
    };
    assert_eq!(t.method, AbMethod::ZTest);
    let lift = c.value("abs_lift").unwrap();
    near(lift.v, 0.2, 1e-12);
    let ci = lift.ci.as_ref().unwrap();
    near(ci.lo, 0.0524, 5e-5);
    near(ci.hi, 0.3339, 5e-5);
    // prop.test(c(56, 48), c(70, 80), correct = FALSE): X-squared = z², the
    // uncorrected χ² of the 2×2 table.
    let mut chi = table(&[&[56.0, 14.0], &[48.0, 32.0]]);
    chi.correct = false;
    chi.test = ContingencyChoice::ChiSquare;
    let (x, _) = engine().contingency(&chi).unwrap();
    near(v(&c, "statistic").powi(2), v(&x, "statistic"), 1e-9);
    near(v(&c, "p_value"), v(&x, "p_value"), 1e-12);
    near(v(&c, "prob_b_better"), 0.995_929_539, 1e-8);
    assert!(!t.sample_ratio_mismatch);

    // chisq.test(c(50000, 51000)): X-squared = 9.901, p-value = 0.001652:
    // not a mismatch at .001; 50000 vs 52000 is.
    let (c, body) = engine().ab_test(&ab(5000, 50_000, 5100, 51_000)).unwrap();
    printed(v(&c, "srm_chi2"), "9.90");
    near(v(&c, "srm_p"), 0.001_651_898_7, 1e-9);
    assert!(matches!(&body, ComputedBody::AbTest(t) if !t.sample_ratio_mismatch));
    let (c, body) = engine().ab_test(&ab(5000, 50_000, 5200, 52_000)).unwrap();
    assert!(matches!(&body, ComputedBody::AbTest(t) if t.sample_ratio_mismatch));
    assert!(c.warnings.iter().any(|w| w.code == "sample_ratio_mismatch"));
    assert_eq!(c.status, ComputedStatus::Warning);
}

#[test]
fn ab_test_uses_fisher_for_small_counts_and_warns_about_peeking() {
    let mut request = ab(1, 20, 6, 20);
    request.looks = 5;
    request.planned_visitors = Some(100);
    request.minimum_lift = Some(0.1);
    let (c, body) = engine().ab_test(&request).unwrap();
    assert!(matches!(&body, ComputedBody::AbTest(t) if t.method == AbMethod::FisherExact));
    // fisher.test(matrix(c(6, 1, 14, 19), 2)): p-value = 0.09148 (scipy
    // fisher_exact agrees).
    printed(v(&c, "p_value"), "0.09148");
    assert!(c.value("statistic").is_none());
    let peeking = c.warnings.iter().find(|w| w.code == "peeking").unwrap();
    assert!(peeking.text.contains(".14"), "{}", peeking.text);
    assert!(c.warnings.iter().any(|w| w.code == "stopped_early"));
    assert!(v(&c, "power") > 0.0 && v(&c, "mde") > 0.0);
    let sentence = domain::computed::narrative::sentence(&c, &body).unwrap();
    assert!(sentence.contains("Fisher"), "{sentence}");

    let mut bad = ab(5, 4, 1, 10);
    assert!(engine().ab_test(&bad).is_err());
    bad.control.conversions = 1;
    bad.looks = 0;
    assert!(engine().ab_test(&bad).is_err());
}

// --- Bootstrap -------------------------------------------------------------------

/// Efron and Tibshirani (1993), table 3.1: the law school sample (n = 15).
const LSAT: [f64; 15] = [
    576.0, 635.0, 558.0, 578.0, 666.0, 580.0, 555.0, 661.0, 651.0, 605.0, 653.0, 575.0, 545.0,
    572.0, 594.0,
];
const GPA: [f64; 15] = [
    3.39, 3.30, 2.81, 3.03, 3.44, 3.07, 3.00, 3.43, 3.36, 3.13, 3.12, 2.74, 2.76, 2.88, 2.96,
];

fn bootstrap(statistic: BootStatistic, resamples: u32) -> BootstrapRequest {
    BootstrapRequest {
        data: DataSet::inline(vec![
            DataColumn::numeric("lsat", &LSAT),
            DataColumn::numeric("gpa", &GPA),
        ]),
        y: "lsat".into(),
        statistic,
        group: None,
        y2: Some("gpa".into()),
        levels: Vec::new(),
        trim: 0.1,
        resamples,
        seed: 1,
        conf_level: 0.95,
    }
}

#[test]
fn bootstrap_of_the_law_school_correlation_matches_efron_and_tibshirani() {
    let (c, body) = engine()
        .bootstrap(&bootstrap(BootStatistic::Correlation, 4000))
        .unwrap();
    assert_eq!(c.seed, Some(1));
    assert_eq!(c.rng.as_deref(), Some("chacha20"));
    // r = .776; the bootstrap SE settles near .132 for large B (E&T table
    // 6.1: .133 at B = 1600, .132 at B = 3200).
    printed(v(&c, "estimate"), "0.776");
    near(v(&c, "se"), 0.132, 0.008);
    let ComputedBody::Bootstrap(b) = &body else {
        panic!("not a bootstrap");
    };
    let interval = |m: BootMethod| b.intervals.iter().find(|i| i.method == m).unwrap();
    let (perc, bca) = (interval(BootMethod::Percentile), interval(BootMethod::Bca));
    // The sampling distribution is skewed to the left: BCa moves down.
    assert!(bca.lo < perc.lo && bca.hi < perc.hi, "{bca:?} vs {perc:?}");
    assert!(perc.lo > 0.4 && perc.hi < 1.0, "{perc:?}");
    assert!(v(&c, "lo_mcse") > 0.0 && v(&c, "se_mcse") > 0.0);
    assert_eq!(b.histogram.counts.iter().sum::<u64>(), 4000);
    assert!(b.acceleration.is_some() && b.z0.is_some());
}

#[test]
fn bootstrap_is_seeded_and_recomputes_exactly() {
    let request = bootstrap(BootStatistic::Mean, 20_000);
    let (c, _) = engine().bootstrap(&request).unwrap();
    // The bootstrap SE of the mean tends to the plug-in SD / √n.
    let m = LSAT.iter().sum::<f64>() / 15.0;
    let plug_in = (LSAT.iter().map(|x| (x - m).powi(2)).sum::<f64>() / 15.0).sqrt() / 15f64.sqrt();
    near(v(&c, "se"), plug_in, 4.0 * v(&c, "se_mcse"));
    let (again, _) = engine().bootstrap(&request).unwrap();
    assert_eq!(c.values, again.values);
    let (recomputed, _) = engine().recompute(&c).unwrap();
    assert_eq!(recomputed.verification.matched, Some(true));
    let mut other = request.clone();
    other.seed = 2;
    let (c2, _) = engine().bootstrap(&other).unwrap();
    assert_ne!(v(&c, "se"), v(&c2, "se"));

    let mut groups = bootstrap(BootStatistic::MeanDifference, 500);
    groups.data.columns.push(DataColumn::categorical(
        "g",
        &[
            "a", "a", "a", "a", "a", "a", "a", "b", "b", "b", "b", "b", "b", "b", "b",
        ],
    ));
    groups.group = Some("g".into());
    let (c, body) = engine().bootstrap(&groups).unwrap();
    let a_mean = LSAT[..7].iter().sum::<f64>() / 7.0;
    let b_mean = LSAT[7..].iter().sum::<f64>() / 8.0;
    near(v(&c, "estimate"), a_mean - b_mean, 1e-9);
    assert!(c.warnings.iter().any(|w| w.code == "few_resamples"));
    assert!(matches!(&body, ComputedBody::Bootstrap(b) if b.versus.as_deref() == Some("g: a − b")));
    groups.group = None;
    assert!(engine().bootstrap(&groups).is_err());
}

#[test]
fn bootstrap_sentence_names_the_interval_and_the_seed() {
    let (c, body) = engine()
        .bootstrap(&bootstrap(BootStatistic::Median, 1000))
        .unwrap();
    let sentence = domain::computed::narrative::sentence(&c, &body).unwrap();
    assert!(sentence.starts_with("The median of lsat is"), "{sentence}");
    assert!(
        sentence.contains("1000 resamples with seed 1"),
        "{sentence}"
    );
    let line = domain::computed::narrative::apa_line(&c, &body)
        .unwrap()
        .plain();
    assert!(line.starts_with("Mdn = "), "{line}");
}

// --- Forecast ---------------------------------------------------------------------

/// A monthly series: level 10, slope 0.5, a seasonal swing of 3 and a small
/// irregular part.
fn monthly(n: usize) -> Vec<f64> {
    (0..n)
        .map(|t| {
            let t = t as f64;
            10.0 + 0.5 * t
                + 3.0 * (2.0 * std::f64::consts::PI * t / 12.0).sin()
                + 0.2 * (1.7 * t).sin()
        })
        .collect()
}

fn forecast_request(y: &[f64], periods: Vec<usize>) -> ForecastRequest {
    ForecastRequest {
        data: DataSet::inline(vec![DataColumn::numeric("sales", y)]),
        y: "sales".into(),
        time: None,
        periods,
        horizon: 12,
        holdout: None,
    }
}

#[test]
fn forecast_decomposes_and_forecasts_a_seasonal_series() {
    let y = monthly(96);
    let (c, body) = engine().forecast(&forecast_request(&y, vec![12])).unwrap();
    assert_ne!(c.status, ComputedStatus::Error, "{:?}", c.error);
    let ComputedBody::Forecast(f) = &body else {
        panic!("not a forecast");
    };
    assert_eq!(body.validate(), Ok(()));
    // The components add up to the series (single precision).
    for (t, observed) in y.iter().enumerate() {
        let sum = f.trend[t] + f.seasonal[0].values[t] + f.remainder[t];
        near(sum, *observed, 1e-3);
    }
    assert!(v(&c, "seasonal_strength_12") > 0.9);
    assert!(v(&c, "trend_strength") > 0.9);
    // Held out 12 months, forecast from the first 84: accurate.
    assert_eq!(v(&c, "holdout"), 12.0);
    assert!(v(&c, "mape") < 3.0, "MAPE {}", v(&c, "mape"));
    // The intervals nest and widen with the horizon.
    for i in 0..12 {
        assert!(f.lo95[i] <= f.lo80[i] && f.lo80[i] <= f.forecast[i]);
        assert!(f.forecast[i] <= f.hi80[i] && f.hi80[i] <= f.hi95[i]);
    }
    assert!(f.hi95[11] - f.lo95[11] >= f.hi95[0] - f.lo95[0]);
    // The forecast continues the trend: next year is above this one.
    assert!(f.forecast[11] > y[95] - 6.0);
    assert!(c.warnings.iter().any(|w| w.code == "extrapolation"));
    let sentence = domain::computed::narrative::sentence(&c, &body).unwrap();
    assert!(sentence.contains("MAPE"), "{sentence}");
}

#[test]
fn forecast_without_periods_and_too_short_series() {
    let y: Vec<f64> = (0..40)
        .map(|t| 5.0 + 0.3 * f64::from(t) + 0.1 * (f64::from(t) * 2.3).sin())
        .collect();
    let (c, body) = engine()
        .forecast(&forecast_request(&y, Vec::new()))
        .unwrap();
    assert_ne!(c.status, ComputedStatus::Error, "{:?}", c.error);
    let ComputedBody::Forecast(f) = &body else {
        panic!("not a forecast");
    };
    assert!(f.trend.is_empty() && f.seasonal.is_empty());
    assert_eq!(f.forecast.len(), 12);
    let (c, _) = engine()
        .forecast(&forecast_request(&monthly(20), vec![12]))
        .unwrap();
    assert_eq!(c.status, ComputedStatus::Error);
    let mut bad = forecast_request(&y, vec![1]);
    assert!(engine().forecast(&bad).is_err());
    bad.periods = Vec::new();
    bad.horizon = 0;
    assert!(engine().forecast(&bad).is_err());
}

// --- LP / MILP -----------------------------------------------------------------------

fn lp(
    sense: Sense,
    objective: &str,
    vars: &[&str],
    constraints: &[(&str, &str)],
) -> OptimizeRequest {
    OptimizeRequest {
        sense,
        objective: objective.into(),
        variables: vars
            .iter()
            .map(|n| LpVariable {
                name: (*n).into(),
                lower: Some(0.0),
                upper: None,
                integer: false,
                binary: false,
            })
            .collect(),
        constraints: constraints
            .iter()
            .map(|(name, expr)| LpConstraint {
                name: Some((*name).into()),
                expr: (*expr).into(),
            })
            .collect(),
    }
}

fn optimization(body: &ComputedBody) -> &OptimizationBody {
    match body {
        ComputedBody::Optimization(o) => o,
        other => panic!("not an optimisation: {other:?}"),
    }
}

#[test]
fn lp_matches_textbook_solutions_and_shadow_prices() {
    // Hillier & Lieberman, Wyndor Glass: x1 = 2, x2 = 6, Z = 36; shadow
    // prices 0, 1.5, 1.
    let request = lp(
        Sense::Maximize,
        "3*x1 + 5*x2",
        &["x1", "x2"],
        &[
            ("plant1", "x1 <= 4"),
            ("plant2", "2*x2 <= 12"),
            ("plant3", "3*x1 + 2*x2 <= 18"),
        ],
    );
    let (c, body) = engine().optimize(&request).unwrap();
    assert!(c.id.starts_with("so_"), "{}", c.id);
    assert_eq!(c.engine.name, "crest-solve");
    let o = optimization(&body);
    assert_eq!(o.status, OptimizationStatus::Optimal);
    near(v(&c, "objective"), 36.0, 1e-9);
    near(v(&c, "value.x1"), 2.0, 1e-9);
    near(v(&c, "value.x2"), 6.0, 1e-9);
    for (name, price) in [("plant1", 0.0), ("plant2", 1.5), ("plant3", 1.0)] {
        near(v(&c, &format!("dual.{name}")), price, 1e-9);
    }
    assert_eq!(
        o.constraints.iter().map(|k| k.binding).collect::<Vec<_>>(),
        [false, true, true]
    );
    near(v(&c, "slack.plant1"), 2.0, 1e-9);
    let sentence = domain::computed::narrative::sentence(&c, &body).unwrap();
    assert!(sentence.contains("plant2"), "{sentence}");

    // Winston, Dorian Auto (minimise, ≥ rows): x1 = 3.6, x2 = 1.4, z = 320;
    // shadow prices 5 and 7.5.
    let request = lp(
        Sense::Minimize,
        "50 x1 + 100 x2",
        &["x1", "x2"],
        &[
            ("women", "7 x1 + 2 x2 >= 28"),
            ("men", "2 x1 + 12 x2 >= 24"),
        ],
    );
    let (c, _) = engine().optimize(&request).unwrap();
    near(v(&c, "objective"), 320.0, 1e-9);
    near(v(&c, "value.x1"), 3.6, 1e-9);
    near(v(&c, "dual.women"), 5.0, 1e-9);
    near(v(&c, "dual.men"), 7.5, 1e-9);

    // Winston, Giapetto: the demand bound has slack 20 and price 0; the
    // reduced costs of the basic variables are 0.
    let request = lp(
        Sense::Maximize,
        "3 soldiers + 2 trains",
        &["soldiers", "trains"],
        &[
            ("finishing", "2 soldiers + trains <= 100"),
            ("carpentry", "soldiers + trains <= 80"),
            ("demand", "soldiers <= 40"),
        ],
    );
    let (c, _) = engine().optimize(&request).unwrap();
    near(v(&c, "objective"), 180.0, 1e-9);
    near(v(&c, "slack.demand"), 20.0, 1e-9);
    near(v(&c, "dual.finishing"), 1.0, 1e-9);
    near(v(&c, "dual.carpentry"), 1.0, 1e-9);
    near(v(&c, "reduced_cost.soldiers"), 0.0, 1e-9);
}

#[test]
fn milp_infeasible_unbounded_and_nonlinear() {
    // 0/1 knapsack: values 8, 11, 6, 4, weights 5, 7, 4, 3, capacity 14:
    // optimum 21 with b, c and d.
    let mut request = lp(
        Sense::Maximize,
        "8a + 11b + 6c + 4d",
        &["a", "b", "c", "d"],
        &[("capacity", "5a + 7b + 4c + 3d <= 14")],
    );
    for v in &mut request.variables {
        v.binary = true;
    }
    let (c, body) = engine().optimize(&request).unwrap();
    let o = optimization(&body);
    assert!(o.integer);
    assert_eq!(o.status, OptimizationStatus::Optimal);
    near(v(&c, "objective"), 21.0, 1e-9);
    assert_eq!(
        ["a", "b", "c", "d"].map(|n| v(&c, &format!("value.{n}"))),
        [0.0, 1.0, 1.0, 1.0]
    );
    assert!(c.method.params["duals"].as_str().unwrap().contains("fixed"));
    let (again, _) = engine().recompute(&c).unwrap();
    assert_eq!(again.verification.matched, Some(true));

    let request = lp(
        Sense::Maximize,
        "x",
        &["x"],
        &[("low", "x >= 5"), ("high", "x <= 3")],
    );
    let (c, body) = engine().optimize(&request).unwrap();
    assert_eq!(optimization(&body).status, OptimizationStatus::Infeasible);
    assert!(c.warnings.iter().any(|w| w.code == "infeasible"));
    let request = lp(
        Sense::Maximize,
        "x + y",
        &["x", "y"],
        &[("c", "x - y <= 1")],
    );
    let (_, body) = engine().optimize(&request).unwrap();
    assert_eq!(optimization(&body).status, OptimizationStatus::Unbounded);
    let request = lp(Sense::Maximize, "x*y", &["x", "y"], &[("c", "x + y <= 1")]);
    assert!(engine().optimize(&request).is_err());
    let request = lp(Sense::Maximize, "x", &["x"], &[("c", "x^2 <= 1")]);
    assert!(engine().optimize(&request).is_err());
}

// --- Equation solver ------------------------------------------------------------------

fn equations(eqs: &[&str], unknowns: &[(&str, f64)], range: Option<[f64; 2]>) -> SolveRequest {
    SolveRequest {
        equations: eqs.iter().map(|e| (*e).to_owned()).collect(),
        unknowns: unknowns
            .iter()
            .map(|(name, guess)| Unknown {
                name: (*name).into(),
                guess: Some(*guess),
            })
            .collect(),
        range,
        tolerance: 1e-12,
        max_iterations: 100,
    }
}

fn solved(body: &ComputedBody) -> &EquationsBody {
    match body {
        ComputedBody::Equations(e) => e,
        other => panic!("not a solution: {other:?}"),
    }
}

#[test]
fn equation_solver_finds_known_roots_and_plugs_them_back() {
    // Every root of x² = 2 in [−3, 3]: ±√2.
    let (c, body) = engine()
        .solve(&equations(&["x^2 = 2"], &[("x", 1.0)], Some([-3.0, 3.0])))
        .unwrap();
    let e = solved(&body);
    assert_eq!(e.method, SolveMethod::BracketScan);
    assert_eq!(e.roots.len(), 2);
    near(e.roots[0], -std::f64::consts::SQRT_2, 1e-12);
    near(v(&c, "root_2"), std::f64::consts::SQRT_2, 1e-12);
    assert!(e.plug_back[0].residual.abs() < 1e-12);
    // The Dottie number: cos x = x at 0.7390851332151607.
    let (c, _) = engine()
        .solve(&equations(&["cos(x) = x"], &[("x", 1.0)], None))
        .unwrap();
    near(v(&c, "x"), 0.739_085_133_215_160_7, 1e-12);
    // Kepler's equation E − 0.5 sin E = 1: E = 1.498701133517848.
    let (c, _) = engine()
        .solve(&equations(&["E - 0.5 sin(E) = 1"], &[("E", 1.0)], None))
        .unwrap();
    near(v(&c, "E"), 1.498_701_133_517_848, 1e-11);
    // x² + y² = 4, x − y = 1 from (1, 1): x = (1 + √7)/2, y = x − 1.
    let (c, body) = engine()
        .solve(&equations(
            &["x^2 + y^2 = 4", "x - y = 1"],
            &[("x", 1.0), ("y", 1.0)],
            None,
        ))
        .unwrap();
    let x = (1.0 + 7f64.sqrt()) / 2.0;
    near(v(&c, "x"), x, 1e-12);
    near(v(&c, "y"), x - 1.0, 1e-12);
    let e = solved(&body);
    assert!(e.converged && e.residual_norm < 1e-12);
    assert!(v(&c, "condition_number") >= 1.0);
    assert_eq!(e.plug_back.len(), 2);
    let sentence = domain::computed::narrative::sentence(&c, &body).unwrap();
    assert!(sentence.starts_with("Newton converged"), "{sentence}");
}

#[test]
fn equation_solver_reports_failure_and_bad_systems() {
    // x² + 1 = 0 has no real root.
    let (c, body) = engine()
        .solve(&equations(
            &["x^2 + 1 = 0"],
            &[("x", 1.0)],
            Some([-5.0, 5.0]),
        ))
        .unwrap();
    assert!(!solved(&body).converged);
    assert!(c.warnings.iter().any(|w| w.code == "not_converged"));
    // A pole is not a root: tan x has roots at 0 and ±π in [−4, 4] but
    // sign flips at ±π/2 too.
    let (_, body) = engine()
        .solve(&equations(&["tan(x)"], &[("x", 1.0)], Some([-4.0, 4.0])))
        .unwrap();
    assert_eq!(solved(&body).roots.len(), 3);
    assert!(
        engine()
            .solve(&equations(&["x + y = 1"], &[("x", 0.0), ("y", 0.0)], None))
            .is_err()
    );
    assert!(
        engine()
            .solve(&equations(&["x = = 1"], &[("x", 0.0)], None))
            .is_err()
    );
    assert!(
        engine()
            .solve(&equations(&["pi = 1"], &[("pi", 0.0)], None))
            .is_err()
    );
}

// --- Monte Carlo ------------------------------------------------------------------------

fn sim_input(name: &str, distribution: SimDistribution) -> SimInput {
    SimInput {
        name: name.into(),
        distribution,
        source: None,
        unit: None,
    }
}

fn simulate(inputs: Vec<SimInput>, model: Option<&str>, samples: u32) -> SimulateRequest {
    SimulateRequest {
        inputs,
        model: model.map(str::to_owned),
        output: None,
        unit: None,
        samples,
        seed: 7,
    }
}

fn simulation(body: &ComputedBody) -> &SimulationBody {
    match body {
        ComputedBody::Simulation(s) => s,
        other => panic!("not a simulation: {other:?}"),
    }
}

#[test]
fn monte_carlo_matches_closed_forms_within_its_mcse() {
    // X ~ N(10, 2), Y ~ N(5, 1): X + Y ~ N(15, √5).
    let request = simulate(
        vec![
            sim_input(
                "x",
                SimDistribution::Normal {
                    mean: 10.0,
                    sd: 2.0,
                },
            ),
            sim_input("y", SimDistribution::Normal { mean: 5.0, sd: 1.0 }),
        ],
        Some("x + y"),
        50_000,
    );
    let (c, body) = engine().simulate(&request).unwrap();
    let mcse = v(&c, "mcse");
    near(mcse, 5f64.sqrt() / 50_000f64.sqrt(), 1e-3);
    near(v(&c, "mean"), 15.0, 4.0 * mcse);
    near(v(&c, "sd"), 5f64.sqrt(), 0.03);
    near(
        v(&c, "p95") - v(&c, "p5"),
        2.0 * 1.644_853_6 * 5f64.sqrt(),
        0.1,
    );
    let m = simulation(&body);
    // The tornado: x swings 2 × 1.2816 × 2, y half that; x first.
    assert_eq!(m.tornado[0].input, "x");
    near(m.tornado[0].swing, 2.0 * 1.281_551_565_5 * 2.0, 1e-9);
    near(m.tornado[1].swing, 2.0 * 1.281_551_565_5, 1e-9);
    near(m.base, 15.0, 1e-12);
    assert_eq!(m.convergence.x.len(), m.convergence_mcse.len());
    assert_eq!(c.seed, Some(7));
    let (again, _) = engine().recompute(&c).unwrap();
    assert_eq!(again.verification.matched, Some(true));

    // Triangular [0, 3, 6] has mean 3; PERT [0, 2, 10] has mean (0 + 8 + 10)/6 = 3.
    let request = simulate(
        vec![
            sim_input(
                "t",
                SimDistribution::Triangular {
                    min: 0.0,
                    mode: 3.0,
                    max: 6.0,
                },
            ),
            sim_input(
                "p",
                SimDistribution::Pert {
                    min: 0.0,
                    mode: 2.0,
                    max: 10.0,
                },
            ),
        ],
        Some("t - p"),
        40_000,
    );
    let (c, _) = engine().simulate(&request).unwrap();
    near(v(&c, "mean"), 0.0, 4.0 * v(&c, "mcse"));
}

#[test]
fn fermi_estimate_is_exact_for_lognormal_factors() {
    // Piano tuners in Chicago (Fermi's classic): people, people per
    // household, share with a piano, tunings per piano per year, per
    // tuner per year (as 1/x).
    let ln = |low, high| SimDistribution::Lognormal { low, high };
    let mut inputs = vec![
        sim_input("people", ln(2.5e6, 3.5e6)),
        sim_input("pianos_per_person", ln(0.01, 0.05)),
        sim_input("tunings_per_piano", ln(0.5, 2.0)),
        sim_input("tuner_per_tuning", ln(1.0 / 1500.0, 1.0 / 500.0)),
    ];
    inputs[0].source = Some("census".into());
    let (c, body) = engine().simulate(&simulate(inputs, None, 100_000)).unwrap();
    let m = simulation(&body);
    assert!(m.fermi);
    // The exact median is the product of the factor medians.
    let median: f64 = [
        (2.5e6, 3.5e6),
        (0.01, 0.05),
        (0.5, 2.0),
        (1.0 / 1500.0, 1.0 / 500.0),
    ]
    .iter()
    .map(|(lo, hi): &(f64, f64)| (lo * hi).sqrt())
    .product();
    near(v(&c, "analytic_p50"), median, 1e-9 * median);
    near(v(&c, "p50") / median, 1.0, 0.02);
    near(v(&c, "p95") / v(&c, "analytic_p95"), 1.0, 0.03);
    assert!(c.warnings.iter().any(|w| w.code == "unsourced_estimates"));
    let sentence = domain::computed::narrative::sentence(&c, &body).unwrap();
    assert!(sentence.contains("lognormal"), "{sentence}");

    let bad = simulate(
        vec![sim_input(
            "x",
            SimDistribution::Uniform { min: 2.0, max: 1.0 },
        )],
        None,
        1000,
    );
    assert!(engine().simulate(&bad).is_err());
}

// --- Cash flow ---------------------------------------------------------------------------

fn cash(flows: &[f64], rate: f64) -> CashFlowRequest {
    CashFlowRequest {
        flows: flows.to_vec(),
        rate,
        finance_rate: None,
        reinvest_rate: None,
        currency: Some("USD".into()),
        period: Some("year".into()),
    }
}

#[test]
fn cash_flow_matches_excel_npv_irr_and_mirr() {
    // Excel NPV(10%, −10000, 3000, 4200, 6800) = 1188.44 discounts the
    // first flow; with it at period 0 the NPV is 1188.44 × 1.1 = 1307.29.
    let (c, body) = engine()
        .cash_flow(&cash(&[-10_000.0, 3000.0, 4200.0, 6800.0], 0.1))
        .unwrap();
    near(v(&c, "npv"), 1_307.287_754, 1e-6);
    assert_eq!(c.value("npv").unwrap().display_text(), "1307.29 USD");
    near(v(&c, "npv") / 1.1, 1_188.443_412, 1e-6);
    let ComputedBody::CashFlow(f) = &body else {
        panic!("not a cash flow");
    };
    assert_eq!(f.rows.len(), 4);
    near(f.rows[3].discount_factor, 1.0 / 1.331, 1e-15);
    // Excel IRR: −70000, 12000, 15000, 18000, 21000 → −2.1%; with 26000
    // more → 8.66% (both to the 12th digit by scipy brentq).
    let (c, _) = engine()
        .cash_flow(&cash(
            &[-70_000.0, 12_000.0, 15_000.0, 18_000.0, 21_000.0],
            0.1,
        ))
        .unwrap();
    near(v(&c, "irr"), -0.021_244_848_273, 1e-12);
    let (c, body) = engine()
        .cash_flow(&cash(
            &[-70_000.0, 12_000.0, 15_000.0, 18_000.0, 21_000.0, 26_000.0],
            0.1,
        ))
        .unwrap();
    near(v(&c, "irr"), 0.086_630_948_037, 1e-12);
    assert_eq!(v(&c, "sign_changes"), 1.0);
    // Payback: −70000 + 12000 + 15000 + 18000 + 21000 = −4000 after year 4,
    // so 4 + 4000/26000.
    near(v(&c, "payback"), 4.0 + 4000.0 / 26_000.0, 1e-12);
    let sentence = domain::computed::narrative::sentence(&c, &body).unwrap();
    assert!(sentence.contains("The IRR is 8.66%"), "{sentence}");
    // Excel MIRR: −120000, 39000, 30000, 21000, 37000, 46000 at finance 10%,
    // reinvestment 12% → 12.61%; 14% → 13.48%; the first three years → −4.80%.
    let flows = [-120_000.0, 39_000.0, 30_000.0, 21_000.0, 37_000.0, 46_000.0];
    let mut request = cash(&flows, 0.1);
    request.reinvest_rate = Some(0.12);
    let (c, _) = engine().cash_flow(&request).unwrap();
    near(v(&c, "mirr"), 0.126_094_130_366, 1e-12);
    request.reinvest_rate = Some(0.14);
    let (c, _) = engine().cash_flow(&request).unwrap();
    near(v(&c, "mirr"), 0.134_759_110_828, 1e-12);
    request.flows.truncate(4);
    request.reinvest_rate = Some(0.12);
    let (c, _) = engine().cash_flow(&request).unwrap();
    near(v(&c, "mirr"), -0.048_044_655_250, 1e-12);
}

#[test]
fn cash_flow_with_two_sign_changes_has_two_irrs() {
    // −100 + 230/(1 + r) − 132/(1 + r)² = 0 at r = 10% and r = 20%.
    let (c, body) = engine()
        .cash_flow(&cash(&[-100.0, 230.0, -132.0], 0.15))
        .unwrap();
    assert!(c.value("irr").is_none());
    near(v(&c, "irr_1"), 0.1, 1e-15);
    near(v(&c, "irr_2"), 0.2, 1e-15);
    assert!(c.warnings.iter().any(|w| w.code == "multiple_irr"));
    let sentence = domain::computed::narrative::sentence(&c, &body).unwrap();
    assert!(sentence.contains("no single IRR"), "{sentence}");
    assert!(engine().cash_flow(&cash(&[1.0], 0.1)).is_err());
    assert!(engine().cash_flow(&cash(&[-1.0, 2.0], -1.0)).is_err());
}
