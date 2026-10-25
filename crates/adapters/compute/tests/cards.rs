//! Reference values of the phase-2 cards, from R's documentation and
//! textbook examples (printed digits), and the properties each card
//! promises (shared histogram edges, seeded reproducibility, plug-back).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use adapter_compute::StatsEngine;
use application::ports::compute::{
    ComputePort, ContingencyChoice, ContingencyRequest, DEFAULT_SEED, DataColumn, DataSet,
    DistributionRequest, PowerRequest,
};
use domain::computed::{
    Alternative, BandwidthRule, BinRule, Computed, ComputedBody, ComputedStatus, ContingencyBody,
    ContingencyTest, PowerTest,
};

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
