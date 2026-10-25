//! Reference values of the phase-2 cards, from R's documentation and
//! textbook examples (printed digits), and the properties each card
//! promises (shared histogram edges, seeded reproducibility, plug-back).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use adapter_compute::StatsEngine;
use application::ports::compute::{ComputePort, DataColumn, DataSet, DistributionRequest};
use domain::computed::{BandwidthRule, BinRule, Computed, ComputedBody, ComputedStatus};

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
