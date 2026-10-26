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

// --- Decision matrix ------------------------------------------------------------------------

#[test]
fn decision_matrix_totals_and_weight_thresholds_by_hand() {
    let criterion = |name: &str, weight, direction| CriterionInput {
        name: name.into(),
        weight,
        direction,
    };
    let option = |name: &str, scores: [f64; 3]| OptionInput {
        name: name.into(),
        scores: scores.to_vec(),
    };
    let request = DecideRequest {
        criteria: vec![
            criterion("price", 50.0, CriterionDirection::Cost),
            criterion("quality", 30.0, CriterionDirection::Benefit),
            criterion("support", 20.0, CriterionDirection::Benefit),
        ],
        options: vec![
            option("A", [100.0, 7.0, 5.0]),
            option("B", [80.0, 6.0, 8.0]),
            option("C", [120.0, 9.0, 6.0]),
        ],
        normalization: Normalization::MinMax,
    };
    let (c, body) = engine().decide(&request).unwrap();
    // Min-max scores: price A .5, B 1, C 0; quality A 1/3, B 0, C 1;
    // support A 0, B 1, C 1/3. Totals .35, .70, .3667.
    near(v(&c, "total.A"), 0.35, 1e-12);
    near(v(&c, "total.B"), 0.7, 1e-12);
    near(v(&c, "total.C"), 0.3 + 0.2 / 3.0, 1e-12);
    near(v(&c, "margin"), 0.7 - (0.3 + 0.2 / 3.0), 1e-12);
    let ComputedBody::DecisionMatrix(d) = &body else {
        panic!("not a decision matrix");
    };
    assert_eq!(
        d.options.iter().map(|o| o.rank).collect::<Vec<_>>(),
        [3, 1, 2]
    );
    // Price at .25 instead of .50 (B: .4 + .6w, C: .7333(1 − w)) hands
    // the lead to C; quality at .475 does too.
    let price = &d.sensitivity[0];
    let lower = price.lower.as_ref().unwrap();
    near(lower.weight, 0.25, 1e-12);
    assert_eq!(lower.new_leader, "C");
    assert!(price.upper.is_none());
    let quality = d.sensitivity[1].upper.as_ref().unwrap();
    near(
        quality.weight,
        0.904_761_904_761_904_7 / 1.904_761_904_761_904_7,
        1e-12,
    );
    assert_eq!(quality.new_leader, "C");
    // The nearest threshold is quality's, .475 − .300 = .175 away.
    near(v(&c, "min_weight_change"), 0.475 - 0.3, 1e-12);
    let sentence = domain::computed::narrative::sentence(&c, &body).unwrap();
    assert!(
        sentence.starts_with("B ranks first with 0.700"),
        "{sentence}"
    );
    assert!(sentence.contains("most sensitive to quality"), "{sentence}");

    let mut tie = request.clone();
    tie.options[0].scores = vec![80.0, 6.0, 8.0];
    let (c, _) = engine().decide(&tie).unwrap();
    assert!(c.warnings.iter().any(|w| w.code == "tie"));
    let mut bad = request;
    bad.options[0].scores.pop();
    assert!(engine().decide(&bad).is_err());
}

// --- Bode / Nyquist -----------------------------------------------------------

fn bode(numerator: &[f64], denominator: &[f64], delay: f64) -> BodeRequest {
    BodeRequest {
        numerator: numerator.to_vec(),
        denominator: denominator.to_vec(),
        zeros: Vec::new(),
        poles: Vec::new(),
        gain: None,
        delay,
        w_min: None,
        w_max: None,
        points: 400,
        unit: FrequencyUnit::RadPerS,
    }
}

/// The sweep point nearest `w`.
fn at(body: &BodeBody, w: f64) -> BodePoint {
    *body
        .points
        .iter()
        .min_by(|a, b| {
            (a.w.ln() - w.ln())
                .abs()
                .total_cmp(&(b.w.ln() - w.ln()).abs())
        })
        .unwrap()
}

#[test]
fn bode_first_order_lag_is_minus_3_db_and_45_degrees_at_the_corner() {
    let mut request = bode(&[1.0], &[1.0, 1.0], 0.0);
    request.w_min = Some(0.01);
    request.w_max = Some(100.0);
    request.points = 401;
    let (c, body) = engine().bode(&request).unwrap();
    let ComputedBody::Bode(b) = body else {
        panic!()
    };
    // 401 log-spaced points over four decades put ω = 1 on the grid.
    let corner = at(&b, 1.0);
    near(corner.w, 1.0, 1e-12);
    near(corner.magnitude_db, -10.0 * 2f64.log10(), 1e-9);
    near(corner.phase_deg, -45.0, 1e-9);
    near(b.points[0].phase_deg, -(0.01f64).atan().to_degrees(), 1e-9);
    near(v(&c, "dc_gain"), 0.0, 1e-12);
    near(v(&c, "bandwidth"), 1.0, 1e-9);
    assert!(c.value("phase_margin").is_none());
    assert!(c.value("gain_margin").is_none());
    assert_eq!(b.poles, [Complex { re: -1.0, im: 0.0 }]);
    // 1/(s+1) + 1 = 0 → s = −2.
    near(b.closed_loop_poles[0].re, -2.0, 1e-12);
    assert_eq!(
        c.value("closed_loop_max_re")
            .and_then(|x| x.label.clone())
            .as_deref(),
        Some("stable")
    );
}

#[test]
fn bode_second_order_peaks_at_one_over_two_zeta() {
    // ωn = 10, ζ = 0.1: |G(jωn)| = 1/(2ζ) = 5, phase −90°.
    let (wn, zeta) = (10.0, 0.1);
    let mut request = bode(&[wn * wn], &[1.0, 2.0 * zeta * wn, wn * wn], 0.0);
    request.w_min = Some(0.1);
    request.w_max = Some(1000.0);
    request.points = 401;
    let (_, body) = engine().bode(&request).unwrap();
    let ComputedBody::Bode(b) = body else {
        panic!()
    };
    let p = at(&b, wn);
    near(p.w, wn, 1e-9);
    near(p.magnitude_db, 20.0 * 5f64.log10(), 1e-9);
    near(p.phase_deg, -90.0, 1e-9);
    near(b.points.last().unwrap().phase_deg, -180.0, 0.2);
    near(b.poles[0].re, -1.0, 1e-12);
    near(b.poles[0].im.abs(), 99f64.sqrt(), 1e-12);
}

#[test]
fn bode_integrator_lag_has_the_closed_form_phase_margin() {
    // G = 1/(s(s+1)): ωc² = (√5 − 1)/2, PM = 90° − atan(ωc).
    let (c, body) = engine().bode(&bode(&[1.0], &[1.0, 1.0, 0.0], 0.0)).unwrap();
    let wc = ((5f64.sqrt() - 1.0) / 2.0).sqrt();
    near(v(&c, "gain_crossover"), wc, 1e-9);
    near(v(&c, "phase_margin"), 90.0 - wc.atan().to_degrees(), 1e-7);
    printed(v(&c, "phase_margin"), "51.83");
    assert!(c.value("gain_margin").is_none());
    assert!(c.value("dc_gain").is_none());
    let ComputedBody::Bode(b) = body else {
        panic!()
    };
    // An integrator starts near −90°, not −90° ± 360°.
    near(b.points[0].phase_deg, -90.0, 1.0);
    // s² + s + 1: −0.5 ± 0.866j.
    near(b.closed_loop_poles[0].re, -0.5, 1e-12);
}

#[test]
fn bode_third_order_gain_margin_is_20_log_6() {
    // G = 1/(s(s+1)(s+2)): phase −180° at ω = √2, |G| = 1/6 there.
    let (c, _) = engine()
        .bode(&bode(&[1.0], &[1.0, 3.0, 2.0, 0.0], 0.0))
        .unwrap();
    near(v(&c, "phase_crossover"), 2f64.sqrt(), 1e-9);
    near(v(&c, "gain_margin"), 20.0 * 6f64.log10(), 1e-8);
    printed(v(&c, "gain_margin"), "15.56");
    let wc = v(&c, "gain_crossover");
    near(
        wc * (wc * wc + 1.0).sqrt() * (wc * wc + 4.0).sqrt(),
        1.0,
        1e-9,
    );
    let pm = 90.0 - wc.atan().to_degrees() - (wc / 2.0).atan().to_degrees();
    near(v(&c, "phase_margin"), pm, 1e-7);
    // The same plant given as poles and a gain.
    let mut by_poles = bode(&[], &[], 0.0);
    by_poles.poles = vec![
        Complex { re: 0.0, im: 0.0 },
        Complex { re: -1.0, im: 0.0 },
        Complex { re: -2.0, im: 0.0 },
    ];
    let (c2, _) = engine().bode(&by_poles).unwrap();
    near(v(&c2, "gain_margin"), v(&c, "gain_margin"), 1e-9);
}

#[test]
fn bode_dead_time_adds_minus_omega_t_to_the_phase() {
    let mut request = bode(&[1.0], &[1.0, 1.0], 0.1);
    request.w_min = Some(0.01);
    request.w_max = Some(100.0);
    request.points = 401;
    let (c, body) = engine().bode(&request).unwrap();
    let ComputedBody::Bode(b) = body else {
        panic!()
    };
    let p = at(&b, 1.0);
    near(p.phase_deg, -45.0 - 0.1f64.to_degrees(), 1e-9);
    near(p.magnitude_db, -10.0 * 2f64.log10(), 1e-9);
    assert!(b.closed_loop_poles.is_empty());
    // The delay brings the phase to −180° even for a first-order lag.
    assert!(c.value("phase_crossover").is_some());
    assert!(engine().bode(&bode(&[1.0], &[0.0], 0.0)).is_err());
    let mut lone = bode(&[], &[], 0.0);
    lone.poles = vec![Complex { re: -1.0, im: 1.0 }];
    assert!(engine().bode(&lone).is_err());
}

// --- Karnaugh map --------------------------------------------------------------

fn kmap(variables: &[&str], minterms: &[u32], dont_cares: &[u32]) -> KarnaughRequest {
    KarnaughRequest {
        variables: variables.iter().map(|s| (*s).to_owned()).collect(),
        minterms: minterms.to_vec(),
        maxterms: Vec::new(),
        dont_cares: dont_cares.to_vec(),
        expression: None,
    }
}

fn terms(implicants: &[Implicant]) -> std::collections::BTreeSet<String> {
    implicants.iter().map(|i| i.term.clone()).collect()
}

#[test]
fn karnaugh_four_variables_match_the_textbook_sop_and_pos() {
    // Mano: F(w,x,y,z) = Σm(0,1,2,4,5,6,8,9,12,13,14) = y' + w'z' + xz'.
    let request = kmap(
        &["w", "x", "y", "z"],
        &[0, 1, 2, 4, 5, 6, 8, 9, 12, 13, 14],
        &[],
    );
    let (c, body) = engine().karnaugh(&request).unwrap();
    let ComputedBody::Karnaugh(k) = body else {
        panic!()
    };
    assert_eq!(
        terms(&k.sop),
        ["w'z'", "xz'", "y'"].map(String::from).into()
    );
    // Zeros 3, 7, 10, 11, 15: (y' + z')(w' + x + y').
    assert_eq!(
        terms(&k.pos),
        ["(w' + x + y')", "(y' + z')"].map(String::from).into()
    );
    assert_eq!(v(&c, "sop_terms"), 3.0);
    assert_eq!(v(&c, "sop_literals"), 5.0);
    assert_eq!(k.method, CoverMethod::Exact);
    assert_eq!(c.assumptions[0].status, AssumptionStatus::Pass);
}

#[test]
fn karnaugh_dont_cares_and_five_variables() {
    // Σm(1,3,7,11,15) + d(0,2,5): two terms of two literals (yz + w'x').
    let (c, _) = engine()
        .karnaugh(&kmap(&["w", "x", "y", "z"], &[1, 3, 7, 11, 15], &[0, 2, 5]))
        .unwrap();
    assert_eq!((v(&c, "sop_terms"), v(&c, "sop_literals")), (2.0, 4.0));
    // Mano: F(A..E) = Σm(0,2,4,6,9,13,21,23,25,29,31) = A'B'E' + BD'E + ACE.
    let (_, body) = engine()
        .karnaugh(&kmap(
            &["A", "B", "C", "D", "E"],
            &[0, 2, 4, 6, 9, 13, 21, 23, 25, 29, 31],
            &[],
        ))
        .unwrap();
    let ComputedBody::Karnaugh(k) = body else {
        panic!()
    };
    assert_eq!(
        terms(&k.sop),
        ["A'B'E'", "ACE", "BD'E"].map(String::from).into()
    );
    assert_eq!(k.maps().len(), 2);
}

#[test]
fn karnaugh_expressions_constants_and_errors() {
    let mut request = kmap(&[], &[], &[]);
    request.expression = Some("A ^ B ^ C".into());
    let (c, body) = engine().karnaugh(&request).unwrap();
    let ComputedBody::Karnaugh(k) = body else {
        panic!()
    };
    assert_eq!(k.variables, ["A", "B", "C"]);
    assert_eq!(k.minterms, [1, 2, 4, 7]);
    assert_eq!((v(&c, "sop_terms"), v(&c, "sop_literals")), (4.0, 12.0));
    let (_, body) = engine()
        .karnaugh(&kmap(&["A", "B"], &[0, 1, 2, 3], &[]))
        .unwrap();
    let ComputedBody::Karnaugh(k) = body else {
        panic!()
    };
    assert_eq!(
        (k.sop_expression.as_str(), k.pos_expression.as_str()),
        ("1", "1")
    );
    let mut zeros = kmap(&["A", "B"], &[], &[]);
    zeros.maxterms = vec![0, 1, 2, 3];
    let (_, body) = engine().karnaugh(&zeros).unwrap();
    let ComputedBody::Karnaugh(k) = body else {
        panic!()
    };
    assert_eq!(
        (k.sop_expression.as_str(), k.pos_expression.as_str()),
        ("0", "0")
    );
    assert!(engine().karnaugh(&kmap(&["A"], &[2], &[])).is_err());
    assert!(engine().karnaugh(&kmap(&["A", "B"], &[1], &[1])).is_err());
    let seven = kmap(&["A", "B", "C", "D", "E", "F", "G"], &[1], &[]);
    assert!(engine().karnaugh(&seven).is_err());
}

#[test]
fn karnaugh_six_variables_stay_fast_and_correct() {
    // Every term with an odd number of ones: no two ones are adjacent, so
    // 32 primes of six literals and no merging.
    let odd: Vec<u32> = (0..64u32).filter(|m| m.count_ones() % 2 == 1).collect();
    let (c, _) = engine().karnaugh(&kmap(&[], &odd, &[])).unwrap();
    assert_eq!((v(&c, "sop_terms"), v(&c, "sop_literals")), (32.0, 192.0));
    // A dense function with many primes.
    let dense: Vec<u32> = (0..64u32).filter(|m| m % 7 != 0).collect();
    let (c, _) = engine().karnaugh(&kmap(&[], &dense, &[0, 7])).unwrap();
    assert_eq!(c.assumptions[0].status, AssumptionStatus::Pass);
}

// --- Beam ------------------------------------------------------------------------

fn support(at: f64, kind: SupportKind) -> BeamSupport {
    BeamSupport { at, kind }
}

fn point(at: f64, value: f64) -> BeamLoad {
    BeamLoad {
        kind: BeamLoadKind::Point,
        at,
        end: None,
        value,
        end_value: None,
    }
}

fn udl(at: f64, end: f64, value: f64) -> BeamLoad {
    BeamLoad {
        kind: BeamLoadKind::Distributed,
        at,
        end: Some(end),
        value,
        end_value: None,
    }
}

fn beam(length: f64, supports: Vec<BeamSupport>, loads: Vec<BeamLoad>) -> BeamRequest {
    BeamRequest {
        length,
        length_unit: LengthUnit::M,
        force_unit: ForceUnit::KN,
        supports,
        loads,
        e: Some(200.0),
        e_unit: ModulusUnit::GPa,
        i: Some(8.0e7),
        i_unit: InertiaUnit::Mm4,
        segments: 200,
    }
}

/// E·I of [`beam`] in kN·m²: 200 GPa × 8e7 mm⁴ = 16 000 kN·m².
const EI: f64 = 16_000.0;

fn rel(actual: f64, expected: f64, tol: f64) {
    near(actual / expected, 1.0, tol);
}

#[test]
fn beam_simply_supported_udl_matches_the_closed_forms() {
    // w = 10 kN/m on 6 m: R = wL/2, Mmax = wL²/8 at midspan, δ = 5wL⁴/384EI.
    let (w, l) = (10.0, 6.0);
    let request = beam(
        l,
        vec![
            support(0.0, SupportKind::Pin),
            support(l, SupportKind::Roller),
        ],
        vec![udl(0.0, l, w)],
    );
    let (c, body) = engine().beam(&request).unwrap();
    assert_eq!(c.status, ComputedStatus::Ok, "{:?}", c.warnings);
    rel(v(&c, "reaction.A"), w * l / 2.0, 1e-9);
    rel(v(&c, "reaction.B"), w * l / 2.0, 1e-9);
    rel(v(&c, "max_moment"), w * l * l / 8.0, 1e-9);
    near(v(&c, "max_moment_at"), l / 2.0, 1e-9);
    rel(v(&c, "max_shear").abs(), w * l / 2.0, 1e-9);
    // Downward: negative; in mm.
    rel(
        -v(&c, "max_deflection"),
        5.0 * w * l.powi(4) / (384.0 * EI) * 1000.0,
        1e-9,
    );
    near(v(&c, "indeterminacy"), 0.0, 0.0);
    let ComputedBody::Beam(b) = body else {
        panic!()
    };
    assert_eq!(c.assumptions[1].status, AssumptionStatus::Pass);
    assert_eq!(b.x.len(), b.moment.len());
    assert_eq!(b.deflection.len(), b.x.len());
}

#[test]
fn beam_cantilever_tip_load_and_fixed_end_moment() {
    // P = 5 kN at the tip of a 2 m cantilever: M_A = PL (ccw), M(0) = −PL,
    // δ = PL³/3EI.
    let (p, l) = (5.0, 2.0);
    let request = beam(l, vec![support(0.0, SupportKind::Fixed)], vec![point(l, p)]);
    let (c, _) = engine().beam(&request).unwrap();
    rel(v(&c, "reaction.A"), p, 1e-9);
    rel(v(&c, "moment.A"), p * l, 1e-9);
    rel(v(&c, "min_moment"), -p * l, 1e-9);
    near(v(&c, "min_moment_at"), 0.0, 1e-12);
    assert!(c.value("max_moment").is_none());
    rel(
        -v(&c, "max_deflection"),
        p * l.powi(3) / (3.0 * EI) * 1000.0,
        1e-9,
    );
    near(v(&c, "max_deflection_at"), l, 1e-12);
}

#[test]
fn beam_indeterminate_cases_match_the_textbook_reactions() {
    let (w, l) = (12.0, 5.0);
    // Propped cantilever, UDL: R_B = 3wL/8, M_A = wL²/8.
    let (c, _) = engine()
        .beam(&beam(
            l,
            vec![
                support(0.0, SupportKind::Fixed),
                support(l, SupportKind::Roller),
            ],
            vec![udl(0.0, l, w)],
        ))
        .unwrap();
    rel(v(&c, "reaction.B"), 3.0 * w * l / 8.0, 1e-9);
    rel(v(&c, "moment.A"), w * l * l / 8.0, 1e-9);
    near(v(&c, "indeterminacy"), 1.0, 0.0);
    // Fixed both ends, UDL: end moments wL²/12, midspan wL²/24, δ = wL⁴/384EI.
    let (c, _) = engine()
        .beam(&beam(
            l,
            vec![
                support(0.0, SupportKind::Fixed),
                support(l, SupportKind::Fixed),
            ],
            vec![udl(0.0, l, w)],
        ))
        .unwrap();
    rel(v(&c, "min_moment"), -w * l * l / 12.0, 1e-9);
    rel(v(&c, "max_moment"), w * l * l / 24.0, 1e-9);
    rel(
        -v(&c, "max_deflection"),
        w * l.powi(4) / (384.0 * EI) * 1000.0,
        1e-9,
    );
    // Two equal spans, UDL: middle reaction 1.25wL, ends 0.375wL, M_B = −wL²/8.
    let (c, _) = engine()
        .beam(&beam(
            2.0 * l,
            vec![
                support(0.0, SupportKind::Pin),
                support(l, SupportKind::Roller),
                support(2.0 * l, SupportKind::Roller),
            ],
            vec![udl(0.0, 2.0 * l, w)],
        ))
        .unwrap();
    rel(v(&c, "reaction.B"), 1.25 * w * l, 1e-9);
    rel(v(&c, "reaction.A"), 0.375 * w * l, 1e-9);
    rel(v(&c, "min_moment"), -w * l * l / 8.0, 1e-9);
    near(v(&c, "min_moment_at"), l, 1e-9);
}

#[test]
fn beam_overhang_point_moment_and_triangular_loads() {
    // Supports at 0 and 4 m, P = 10 kN at the tip of a 6 m beam:
    // R_B = P·6/4 = 15, R_A = −5, M_B = −P·2 = −20.
    let (c, _) = engine()
        .beam(&beam(
            6.0,
            vec![
                support(0.0, SupportKind::Pin),
                support(4.0, SupportKind::Roller),
            ],
            vec![point(6.0, 10.0)],
        ))
        .unwrap();
    rel(v(&c, "reaction.B"), 15.0, 1e-9);
    rel(v(&c, "reaction.A"), -5.0, 1e-9);
    rel(v(&c, "min_moment"), -20.0, 1e-9);
    // A 30 kN·m counterclockwise moment at midspan of a 6 m simple beam:
    // R_A = M/L up, R_B = M/L down; the moment jumps from +15 to −15.
    let moment = BeamLoad {
        kind: BeamLoadKind::Moment,
        at: 3.0,
        end: None,
        value: 30.0,
        end_value: None,
    };
    let (c, _) = engine()
        .beam(&beam(
            6.0,
            vec![
                support(0.0, SupportKind::Pin),
                support(6.0, SupportKind::Roller),
            ],
            vec![moment],
        ))
        .unwrap();
    rel(v(&c, "reaction.A"), 5.0, 1e-9);
    rel(v(&c, "reaction.B"), -5.0, 1e-9);
    rel(v(&c, "min_moment"), -15.0, 1e-9);
    rel(v(&c, "max_moment"), 15.0, 1e-9);
    // Triangular load 0 → w on a simple beam: R_B = wL/3, Mmax = wL²/(9√3)
    // at L/√3.
    let (w, l) = (9.0, 6.0);
    let triangle = BeamLoad {
        kind: BeamLoadKind::Distributed,
        at: 0.0,
        end: Some(l),
        value: 0.0,
        end_value: Some(w),
    };
    let (c, _) = engine()
        .beam(&beam(
            l,
            vec![
                support(0.0, SupportKind::Pin),
                support(l, SupportKind::Roller),
            ],
            vec![triangle],
        ))
        .unwrap();
    rel(v(&c, "reaction.B"), w * l / 3.0, 1e-9);
    rel(v(&c, "max_moment"), w * l * l / (9.0 * 3f64.sqrt()), 1e-9);
    near(v(&c, "max_moment_at"), l / 3f64.sqrt(), 1e-9);
}

#[test]
fn beam_rejects_mechanisms_and_bad_positions() {
    let one_pin = beam(
        4.0,
        vec![support(0.0, SupportKind::Pin)],
        vec![point(2.0, 1.0)],
    );
    assert!(engine().beam(&one_pin).is_err());
    let outside = beam(
        4.0,
        vec![
            support(0.0, SupportKind::Pin),
            support(5.0, SupportKind::Roller),
        ],
        vec![],
    );
    assert!(engine().beam(&outside).is_err());
    let mut imperial = beam(
        10.0,
        vec![
            support(0.0, SupportKind::Pin),
            support(10.0, SupportKind::Roller),
        ],
        vec![point(5.0, 1.0)],
    );
    imperial.length_unit = LengthUnit::Ft;
    imperial.force_unit = ForceUnit::Kip;
    imperial.e = Some(29_000.0);
    imperial.e_unit = ModulusUnit::Ksi;
    imperial.i = Some(100.0);
    imperial.i_unit = InertiaUnit::In4;
    // PL³/48EI with L = 120 in, P = 1 kip, E = 29 000 ksi: 0.012414 in.
    let (c, _) = engine().beam(&imperial).unwrap();
    rel(
        -v(&c, "max_deflection"),
        120f64.powi(3) / (48.0 * 29_000.0 * 100.0),
        1e-9,
    );
    assert_eq!(
        c.value("max_deflection")
            .and_then(|x| x.unit.clone())
            .as_deref(),
        Some("in")
    );
}

// --- Tolerance stack-up -----------------------------------------------------------

fn dim(name: &str, nominal: f64, tolerance: f64, direction: StackDirection) -> StackDimensionInput {
    StackDimensionInput {
        name: name.into(),
        nominal,
        tolerance: Some(tolerance),
        plus: None,
        minus: None,
        direction,
        distribution: ToleranceDistribution::Normal,
    }
}

fn gap_stack() -> ToleranceRequest {
    // A housing of 50 ± 0.1 holding parts of 30 ± 0.05 and 19.8 ± 0.05:
    // gap 0.2 nominal.
    ToleranceRequest {
        dimensions: vec![
            dim("housing", 50.0, 0.1, StackDirection::Add),
            dim("part 1", 30.0, 0.05, StackDirection::Subtract),
            dim("part 2", 19.8, 0.05, StackDirection::Subtract),
        ],
        lower_limit: Some(0.05),
        upper_limit: None,
        unit: Some("mm".into()),
        sigmas: 3.0,
        samples: 200_000,
        seed: 7,
    }
}

#[test]
fn tolerance_worst_case_and_rss_are_closed_form() {
    let (c, body) = engine().tolerance(&gap_stack()).unwrap();
    near(v(&c, "nominal"), 0.2, 1e-12);
    near(v(&c, "worst_case_min"), 0.0, 1e-12);
    near(v(&c, "worst_case_max"), 0.4, 1e-12);
    let rss = (0.1f64.powi(2) + 2.0 * 0.05f64.powi(2)).sqrt();
    near(v(&c, "rss_min"), 0.2 - rss, 1e-12);
    near(v(&c, "rss_max"), 0.2 + rss, 1e-12);
    let ComputedBody::Tolerance(t) = body else {
        panic!()
    };
    // Shares: 0.1/0.2 of the worst case, 0.01/0.015 of the RSS variance.
    near(t.dimensions[0].worst_case_share, 0.5, 1e-12);
    near(t.dimensions[0].rss_share, 2.0 / 3.0, 1e-12);
    let total: f64 = t.dimensions.iter().map(|d| d.variance_share).sum();
    near(total, 1.0, 1e-12);
    assert_eq!(t.histogram.counts.iter().sum::<u64>(), 200_000);
    assert_eq!(c.method.params["worst_case_fits"], serde_json::json!(false));
    assert_eq!(c.method.params["rss_fits"], serde_json::json!(true));
}

#[test]
fn tolerance_monte_carlo_matches_the_normal_stack_within_its_error() {
    let (c, _) = engine().tolerance(&gap_stack()).unwrap();
    let sigma = (0.1f64.powi(2) + 2.0 * 0.05f64.powi(2)).sqrt() / 3.0;
    let n = 200_000.0;
    near(v(&c, "mc_mean"), 0.2, 4.0 * sigma / f64::sqrt(n));
    near(v(&c, "mc_sd") / sigma, 1.0, 0.01);
    // P(gap < 0.05) = Φ((0.05 − 0.2)/σ).
    let p = statrs::distribution::ContinuousCDF::cdf(
        &statrs::distribution::Normal::new(0.2, sigma).unwrap(),
        0.05,
    );
    near(
        v(&c, "out_of_spec"),
        p,
        4.0 * (p * (1.0 - p) / n).sqrt() + 1e-6,
    );
    // Seeded: the same request gives the same draws, and recompute matches.
    let (again, _) = engine().tolerance(&gap_stack()).unwrap();
    assert_eq!(v(&again, "mc_sd"), v(&c, "mc_sd"));
    let (re, _) = engine().recompute(&c).unwrap();
    assert_eq!(re.verification.matched, Some(true));
    assert_eq!(c.seed, Some(7));
}

#[test]
fn tolerance_asymmetric_and_uniform_dimensions() {
    let mut request = gap_stack();
    request.dimensions = vec![
        StackDimensionInput {
            name: "shaft".into(),
            nominal: 10.0,
            tolerance: None,
            plus: Some(0.2),
            minus: Some(0.0),
            direction: StackDirection::Add,
            distribution: ToleranceDistribution::Uniform,
        },
        dim("spacer", 4.0, 0.06, StackDirection::Subtract),
    ];
    request.lower_limit = None;
    let (c, body) = engine().tolerance(&request).unwrap();
    near(v(&c, "nominal"), 6.0, 1e-12);
    near(v(&c, "center"), 6.1, 1e-12);
    near(v(&c, "worst_case_max"), 6.1 + 0.16, 1e-12);
    let ComputedBody::Tolerance(t) = body else {
        panic!()
    };
    near(t.dimensions[0].sd, 0.1 / 3f64.sqrt(), 1e-15);
    let sigma = ((0.1f64 / 3f64.sqrt()).powi(2) + 0.02f64.powi(2)).sqrt();
    near(v(&c, "mc_sd") / sigma, 1.0, 0.01);
    assert!(c.value("out_of_spec").is_none());
    let mut bad = gap_stack();
    bad.dimensions[0].plus = Some(0.1);
    assert!(engine().tolerance(&bad).is_err());
    bad = gap_stack();
    bad.dimensions[1].tolerance = Some(-0.1);
    assert!(engine().tolerance(&bad).is_err());
}

// --- BOM costing ------------------------------------------------------------------

fn bom_file(builds: f64) -> BomRequest {
    let text = std::fs::read_to_string(format!(
        "{}/tests/fixtures/board_bom.csv",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    let data = application::csv::data_set(&text, ',', inline_source()).unwrap();
    BomRequest {
        data,
        items: Vec::new(),
        part: None,
        quantity: None,
        unit_cost: None,
        category: None,
        reference: None,
        mpn: None,
        builds,
        currency: Some("USD".into()),
    }
}

#[test]
fn bom_costs_lines_exactly_and_flags_gaps() {
    let (c, body) = engine().bom(&bom_file(100.0)).unwrap();
    let ComputedBody::Bom(b) = body else { panic!() };
    // R1, R2, R5–R8: six resistors counted from the references.
    assert_eq!(b.lines[0].quantity, "6");
    assert!(b.lines[0].quantity_from_references);
    assert_eq!(b.lines[0].extended.as_deref(), Some("0.0126"));
    // "$3,45" is 3.45 (a decimal comma).
    assert_eq!(b.lines[2].unit_cost.as_deref(), Some("3.45"));
    // 0.0126 + 0.014 + 3.45 + 0.42 = 3.8966; the switch has no price.
    assert_eq!(b.total, "3.8966");
    assert_eq!(b.total_builds, "389.66");
    assert_eq!(
        c.value("total").and_then(|v| v.display.clone()).as_deref(),
        Some("3.8966")
    );
    assert_eq!(c.value("total_builds").map(|v| v.v), Some(389.66));
    assert_eq!(v(&c, "missing_prices"), 1.0);
    assert_eq!(v(&c, "missing_mpns"), 1.0);
    assert_eq!(b.categories[0].name, "ICs");
    assert_eq!(
        b.categories
            .iter()
            .find(|k| k.name == "passives")
            .map(|k| k.total.as_str()),
        Some("0.0266")
    );
    let codes: Vec<&str> = c.warnings.iter().map(|w| w.code.as_str()).collect();
    // J1's file total 0.50 disagrees with 1 × 0.42.
    assert!(codes.contains(&"extended_mismatch"), "{codes:?}");
    assert!(codes.contains(&"missing_prices"));
    assert!(c.warnings.iter().any(|w| w.text.contains("rows 4")));
}

#[test]
fn bom_inline_items_and_errors() {
    let mut request = bom_file(1.0);
    request.data = DataSet::default();
    request.items = vec![
        BomItem {
            part: "M3 screw".into(),
            quantity: Some(4.0),
            unit_cost: Some(0.1),
            category: Some("mechanical".into()),
            reference: None,
            mpn: None,
            description: None,
        },
        BomItem {
            part: "case".into(),
            quantity: Some(1.0),
            unit_cost: Some(2.2),
            category: None,
            reference: None,
            mpn: None,
            description: None,
        },
    ];
    let (c, body) = engine().bom(&request).unwrap();
    let ComputedBody::Bom(b) = body else { panic!() };
    // 0.1 × 4 + 2.2 = 2.6 exactly (2.6000000000000005 in binary).
    assert_eq!(b.total, "2.6");
    assert!(c.value("total_builds").is_none());
    let mut negative = request.clone();
    negative.items[0].quantity = Some(-1.0);
    assert!(engine().bom(&negative).is_err());
    let mut wrong = bom_file(1.0);
    wrong.unit_cost = Some("Nope".into());
    assert!(engine().bom(&wrong).is_err());
}
