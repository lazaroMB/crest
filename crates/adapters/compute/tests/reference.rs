//! Reference values: NIST StRD certified results and examples from R's
//! documentation (printed digits). No scipy or R is needed: the data sets
//! and the published values are embedded below.
//!
//! - NIST StRD linear regression: Norris (lower difficulty) and Longley
//!   (higher difficulty, ill-conditioned), certified to 15 digits.
//! - NIST StRD univariate summary statistics: Michelson, NumAcc1–4.
//! - R: `t.test` on `sleep`, the `wilcox.test`, `fisher.test`,
//!   `chisq.test` and `cor.test` examples of their help pages.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::approx_constant)]

use adapter_compute::StatsEngine;
use application::ports::compute::{
    ComputePort, CorrelateRequest, DataColumn, DataSet, DescribeRequest, RegressRequest,
    TestChoice, TestRequest,
};
use domain::computed::{
    Alternative, Computed, ComputedBody, ComputedStatus, CorrelationMethod, PAdjust, RegressionBody,
};

/// Asserts `|actual − expected| ≤ rel · |expected|`.
#[track_caller]
fn rel(actual: f64, expected: f64, rel: f64) {
    let err = (actual - expected).abs() / expected.abs();
    assert!(
        err <= rel,
        "{actual} vs {expected}: relative error {err:e} > {rel:e}"
    );
}

/// Asserts `actual` rounds to `printed` at the printed digits: within
/// half a unit of its last digit.
#[track_caller]
fn printed(actual: f64, printed: &str) {
    let expected: f64 = printed.parse().unwrap();
    let mantissa = printed.split(['e', 'E']).next().unwrap_or(printed);
    let decimals = mantissa.split('.').nth(1).map_or(0, str::len);
    let exponent: i32 = printed
        .split(['e', 'E'])
        .nth(1)
        .map_or(0, |e| e.parse().unwrap());
    let half = 0.5 * 10f64.powi(exponent - i32::try_from(decimals).unwrap());
    assert!(
        (actual - expected).abs() <= half * (1.0 + 1e-9),
        "{actual} does not round to {printed}"
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

fn regression(body: &ComputedBody) -> &RegressionBody {
    match body {
        ComputedBody::Regression(r) => r,
        other => panic!("not a regression: {other:?}"),
    }
}

// --- NIST StRD linear regression -------------------------------------------

/// Norris: (y, x).
const NORRIS: [(f64, f64); 36] = [
    (0.1, 0.2),
    (338.8, 337.4),
    (118.1, 118.2),
    (888.0, 884.6),
    (9.2, 10.1),
    (228.1, 226.5),
    (668.5, 666.3),
    (998.5, 996.3),
    (449.1, 448.6),
    (778.9, 777.0),
    (559.2, 558.2),
    (0.3, 0.4),
    (0.1, 0.6),
    (778.1, 775.5),
    (668.8, 666.9),
    (339.3, 338.0),
    (448.9, 447.5),
    (10.8, 11.6),
    (557.7, 556.0),
    (228.3, 228.1),
    (998.0, 995.8),
    (888.8, 887.6),
    (119.6, 120.2),
    (0.3, 0.3),
    (0.6, 0.3),
    (557.6, 556.8),
    (339.3, 339.1),
    (888.0, 887.2),
    (998.5, 999.0),
    (778.9, 779.0),
    (10.2, 11.1),
    (117.6, 118.3),
    (228.9, 229.2),
    (668.4, 669.1),
    (449.2, 448.9),
    (0.2, 0.5),
];

#[test]
fn nist_norris() {
    let y: Vec<f64> = NORRIS.iter().map(|r| r.0).collect();
    let x: Vec<f64> = NORRIS.iter().map(|r| r.1).collect();
    let request = RegressRequest {
        data: DataSet::inline(vec![
            DataColumn::numeric("y", &y),
            DataColumn::numeric("x", &x),
        ]),
        y: "y".into(),
        x: vec!["x".into()],
        conf_level: 0.95,
    };
    let (c, body) = engine().regress(&request).unwrap();
    assert_eq!(c.validate(), Ok(()));
    let r = regression(&body);
    let tol = 1e-12;
    rel(r.coefficients[0].estimate, -0.262_323_073_774_029, tol);
    rel(r.coefficients[0].se, 0.232_818_234_301_152, tol);
    rel(r.coefficients[1].estimate, 1.002_116_818_020_45, tol);
    rel(r.coefficients[1].se, 0.429_796_848_199_937e-3, tol);
    rel(r.fit.sigma.unwrap(), 0.884_796_396_144_373, tol);
    rel(r.fit.r_squared, 0.999_993_745_883_712, tol);
    rel(v(&c, "coef.x"), 1.002_116_818_020_45, tol);
}

/// Longley: employment, then GNP deflator, GNP, unemployed, armed forces,
/// population, year.
const LONGLEY: [[f64; 7]; 16] = [
    [60323.0, 83.0, 234289.0, 2356.0, 1590.0, 107608.0, 1947.0],
    [61122.0, 88.5, 259426.0, 2325.0, 1456.0, 108632.0, 1948.0],
    [60171.0, 88.2, 258054.0, 3682.0, 1616.0, 109773.0, 1949.0],
    [61187.0, 89.5, 284599.0, 3351.0, 1650.0, 110929.0, 1950.0],
    [63221.0, 96.2, 328975.0, 2099.0, 3099.0, 112075.0, 1951.0],
    [63639.0, 98.1, 346999.0, 1932.0, 3594.0, 113270.0, 1952.0],
    [64989.0, 99.0, 365385.0, 1870.0, 3547.0, 115094.0, 1953.0],
    [63761.0, 100.0, 363112.0, 3578.0, 3350.0, 116219.0, 1954.0],
    [66019.0, 101.2, 397469.0, 2904.0, 3048.0, 117388.0, 1955.0],
    [67857.0, 104.6, 419180.0, 2822.0, 2857.0, 118734.0, 1956.0],
    [68169.0, 108.4, 442769.0, 2936.0, 2798.0, 120445.0, 1957.0],
    [66513.0, 110.8, 444546.0, 4681.0, 2637.0, 121950.0, 1958.0],
    [68655.0, 112.6, 482704.0, 3813.0, 2552.0, 123366.0, 1959.0],
    [69564.0, 114.2, 502601.0, 3931.0, 2514.0, 125368.0, 1960.0],
    [69331.0, 115.7, 518173.0, 4806.0, 2572.0, 127852.0, 1961.0],
    [70551.0, 116.9, 554894.0, 4007.0, 2827.0, 130081.0, 1962.0],
];

#[test]
fn nist_longley() {
    let names = ["y", "x1", "x2", "x3", "x4", "x5", "x6"];
    let columns = (0..7)
        .map(|j| {
            let values: Vec<f64> = LONGLEY.iter().map(|r| r[j]).collect();
            DataColumn::numeric(names[j], &values)
        })
        .collect();
    let request = RegressRequest {
        data: DataSet::inline(columns),
        y: "y".into(),
        x: names[1..].iter().map(|s| (*s).to_owned()).collect(),
        conf_level: 0.95,
    };
    let (c, body) = engine().regress(&request).unwrap();
    assert_eq!(c.validate(), Ok(()));
    let r = regression(&body);
    let certified = [
        (-3_482_258.634_595_82, 890_420.383_607_373),
        (15.061_872_271_373_3, 84.914_925_774_766_9),
        (-0.358_191_792_925_910e-1, 0.334_910_077_722_432e-1),
        (-2.020_229_803_816_83, 0.488_399_681_651_699),
        (-1.033_226_867_173_59, 0.214_274_163_161_675),
        (-0.511_041_056_535_807e-1, 0.226_073_200_069_370),
        (1_829.151_464_613_55, 455.478_499_142_212),
    ];
    // Longley is ill-conditioned; QR on centered predictors keeps 11+ digits.
    let tol = 1e-11;
    for (row, (b, se)) in r.coefficients.iter().zip(certified) {
        rel(row.estimate, b, tol);
        rel(row.se, se, tol);
    }
    rel(r.fit.sigma.unwrap(), 304.854_073_561_965, tol);
    rel(r.fit.r_squared, 0.995_479_004_577_296, tol);
    // statsmodels prints "Cond. No. 4.86e+09" for this design.
    rel(r.diagnostics.condition_number.unwrap(), 4.86e9, 0.01);
    assert!(c.warnings.iter().any(|w| w.code == "vif_gt_10"));
    assert!(
        c.warnings
            .iter()
            .any(|w| w.code == "large_condition_number")
    );
    assert_eq!(c.status, ComputedStatus::Warning);
}

// --- NIST StRD univariate --------------------------------------------------

fn describe_one(values: &[f64]) -> Computed {
    let request = DescribeRequest {
        data: DataSet::inline(vec![DataColumn::numeric("x", values)]),
        variables: Vec::new(),
        by: None,
    };
    engine().describe(&request).unwrap().0
}

/// Michelson's 1879 speed of light measurements (km/s − 299000), as
/// R's `morley$Speed`; NIST's data set is these / 1000 + 299.
const MORLEY: [f64; 100] = [
    850.0, 740.0, 900.0, 1070.0, 930.0, 850.0, 950.0, 980.0, 980.0, 880.0, 1000.0, 980.0, 930.0,
    650.0, 760.0, 810.0, 1000.0, 1000.0, 960.0, 960.0, 960.0, 940.0, 960.0, 940.0, 880.0, 800.0,
    850.0, 880.0, 900.0, 840.0, 830.0, 790.0, 810.0, 880.0, 880.0, 830.0, 800.0, 790.0, 760.0,
    800.0, 880.0, 880.0, 880.0, 860.0, 720.0, 720.0, 620.0, 860.0, 970.0, 950.0, 880.0, 910.0,
    850.0, 870.0, 840.0, 840.0, 850.0, 840.0, 840.0, 840.0, 890.0, 810.0, 810.0, 820.0, 800.0,
    770.0, 760.0, 740.0, 750.0, 760.0, 910.0, 920.0, 890.0, 860.0, 880.0, 720.0, 840.0, 850.0,
    850.0, 780.0, 890.0, 840.0, 780.0, 810.0, 760.0, 810.0, 790.0, 810.0, 820.0, 850.0, 870.0,
    870.0, 810.0, 740.0, 810.0, 940.0, 950.0, 800.0, 810.0, 870.0,
];

#[test]
fn nist_michelson() {
    let values: Vec<f64> = MORLEY.iter().map(|s| s / 1000.0 + 299.0).collect();
    let c = describe_one(&values);
    rel(v(&c, "x.mean"), 299.852_400_000_000, 1e-13);
    rel(v(&c, "x.sd"), 0.079_010_547_819_051_8, 1e-10);
    assert_eq!(v(&c, "x.n"), 100.0);
}

#[test]
fn nist_numacc() {
    let c = describe_one(&[10_000_001.0, 10_000_003.0, 10_000_002.0]);
    rel(v(&c, "x.mean"), 10_000_002.0, 1e-15);
    rel(v(&c, "x.sd"), 1.0, 1e-15);
    // NumAcc2–4: one centre value, then 500 pairs below and above.
    for (centre, tol) in [(1.2, 1e-13), (1_000_000.2, 1e-9), (10_000_000.2, 1e-8)] {
        let mut values = vec![centre];
        for _ in 0..500 {
            values.push(centre - 0.1);
            values.push(centre + 0.1);
        }
        let c = describe_one(&values);
        rel(v(&c, "x.mean"), centre, 1e-15);
        rel(v(&c, "x.sd"), 0.1, tol);
    }
}

// --- R documentation examples ----------------------------------------------

/// R's `sleep$extra` by group.
const SLEEP_1: [f64; 10] = [0.7, -1.6, -0.2, -1.2, -0.1, 3.4, 3.7, 0.8, 0.0, 2.0];
const SLEEP_2: [f64; 10] = [1.9, 0.8, 1.1, 0.1, -0.1, 4.4, 5.5, 1.6, 4.6, 3.4];

fn sleep_grouped() -> DataSet {
    let extra: Vec<f64> = SLEEP_1.iter().chain(&SLEEP_2).copied().collect();
    let group: Vec<&str> = (0..20).map(|i| if i < 10 { "1" } else { "2" }).collect();
    DataSet::inline(vec![
        DataColumn::numeric("extra", &extra),
        DataColumn::categorical("group", &group),
    ])
}

fn sleep_test(test: TestChoice) -> Computed {
    let request = TestRequest {
        data: sleep_grouped(),
        test,
        y: Some("extra".into()),
        group: Some("group".into()),
        ..TestRequest::default()
    };
    let (c, _) = engine().test(&request).unwrap();
    assert_eq!(c.validate(), Ok(()));
    c
}

#[test]
fn r_sleep_welch() {
    // t.test(extra ~ group, data = sleep)
    let c = sleep_test(TestChoice::WelchT);
    assert_eq!(c.kind, "stats.ttest_welch");
    printed(v(&c, "statistic"), "-1.8608");
    printed(v(&c, "df"), "17.776");
    printed(v(&c, "p_value"), "0.07939");
    let ci = c.value("mean_diff").unwrap().ci.clone().unwrap();
    printed(ci.lo, "-3.3654832");
    printed(ci.hi, "0.2054832");
}

#[test]
fn r_sleep_student() {
    // t.test(extra ~ group, data = sleep, var.equal = TRUE)
    let c = sleep_test(TestChoice::StudentT);
    printed(v(&c, "statistic"), "-1.8608");
    assert_eq!(v(&c, "df"), 18.0);
    printed(v(&c, "p_value"), "0.07919");
}

#[test]
fn r_sleep_paired() {
    // t.test(sleep$extra[1:10], sleep$extra[11:20], paired = TRUE)
    let request = TestRequest {
        data: DataSet::inline(vec![
            DataColumn::numeric("g1", &SLEEP_1),
            DataColumn::numeric("g2", &SLEEP_2),
        ]),
        test: TestChoice::PairedT,
        y: Some("g1".into()),
        y2: Some("g2".into()),
        ..TestRequest::default()
    };
    let (c, _) = engine().test(&request).unwrap();
    printed(v(&c, "statistic"), "-4.0621");
    assert_eq!(v(&c, "df"), 9.0);
    printed(v(&c, "p_value"), "0.002833");
    let ci = c.value("mean_diff").unwrap().ci.clone().unwrap();
    printed(ci.lo, "-2.4598858");
    printed(ci.hi, "-0.7001142");
}

#[test]
fn r_wilcox_paired_depression() {
    // ?wilcox.test: wilcox.test(x, y, paired = TRUE, alternative = "greater")
    // gives V = 40, p-value = 0.01953.
    let x = [1.83, 0.50, 1.62, 2.48, 1.68, 1.88, 1.55, 3.06, 1.30];
    let y = [0.878, 0.647, 0.598, 2.05, 1.06, 1.29, 1.06, 3.14, 1.29];
    let request = TestRequest {
        data: DataSet::inline(vec![
            DataColumn::numeric("x", &x),
            DataColumn::numeric("y", &y),
        ]),
        test: TestChoice::Wilcoxon,
        y: Some("x".into()),
        y2: Some("y".into()),
        paired: true,
        alternative: Alternative::Greater,
        ..TestRequest::default()
    };
    let (c, _) = engine().test(&request).unwrap();
    assert_eq!(v(&c, "statistic"), 40.0);
    printed(v(&c, "p_value"), "0.01953");
}

#[test]
fn r_wilcox_two_sample() {
    // ?wilcox.test: wilcox.test(x, y, alternative = "g") gives W = 35,
    // p-value = 0.1272.
    let x = [0.80, 0.83, 1.89, 1.04, 1.45, 1.38, 1.91, 1.64, 0.73, 1.46];
    let y = [1.15, 0.88, 0.90, 0.74, 1.21];
    let mut columns = vec![
        DataColumn::numeric("x", &x),
        DataColumn::numeric("y", &[0.0; 10]),
    ];
    // Columns must have equal length: pad y with missing values.
    columns[1].values = application::ports::compute::ColumnValues::Numeric(
        y.iter()
            .map(|v| Some(*v))
            .chain(std::iter::repeat_n(None, 5))
            .collect(),
    );
    let request = TestRequest {
        data: DataSet::inline(columns),
        test: TestChoice::MannWhitney,
        y: Some("x".into()),
        y2: Some("y".into()),
        alternative: Alternative::Greater,
        ..TestRequest::default()
    };
    let (c, _) = engine().test(&request).unwrap();
    assert_eq!(v(&c, "statistic"), 35.0);
    printed(v(&c, "p_value"), "0.1272");
    assert_eq!(c.method.params["p_method"], "exact");
}

fn table_test(table: Vec<Vec<f64>>, test: TestChoice, alternative: Alternative) -> Computed {
    let request = TestRequest {
        test,
        table: Some(table),
        alternative,
        ..TestRequest::default()
    };
    let (c, _) = engine().test(&request).unwrap();
    assert_eq!(c.validate(), Ok(()));
    c
}

#[test]
fn r_fisher_tea_tasting_and_convictions() {
    // ?fisher.test: TeaTasting, alternative = "greater": p-value = 0.2429;
    // two-sided: p-value = 0.4857.
    let tea = vec![vec![3.0, 1.0], vec![1.0, 3.0]];
    let c = table_test(tea.clone(), TestChoice::Fisher, Alternative::Greater);
    printed(v(&c, "p_value"), "0.2429");
    let c = table_test(tea, TestChoice::Fisher, Alternative::TwoSided);
    printed(v(&c, "p_value"), "0.4857");
    // Convictions, alternative = "less": p-value = 0.0004652.
    let convictions = vec![vec![2.0, 15.0], vec![10.0, 3.0]];
    let c = table_test(convictions, TestChoice::Fisher, Alternative::Less);
    printed(v(&c, "p_value"), "0.0004652");
}

#[test]
fn r_chisq_party_by_gender() {
    // ?chisq.test: M <- as.table(rbind(c(762, 327, 468), c(484, 239, 477)))
    // X-squared = 30.07, df = 2, p-value = 2.954e-07.
    let m = vec![vec![762.0, 327.0, 468.0], vec![484.0, 239.0, 477.0]];
    let c = table_test(m, TestChoice::Auto, Alternative::TwoSided);
    assert_eq!(c.kind, "stats.chi_square");
    printed(v(&c, "statistic"), "30.07");
    assert_eq!(v(&c, "df"), 2.0);
    printed(v(&c, "p_value"), "2.954e-07");
}

/// The Hollander & Wolfe example of `?cor.test`.
fn cor_data() -> DataSet {
    let x = [44.4, 45.9, 41.9, 53.3, 44.7, 44.1, 50.7, 45.2, 60.1];
    let y = [2.6, 3.1, 2.5, 5.0, 3.6, 4.0, 5.2, 2.8, 3.8];
    DataSet::inline(vec![
        DataColumn::numeric("x", &x),
        DataColumn::numeric("y", &y),
    ])
}

fn cor(method: CorrelationMethod) -> Computed {
    let request = CorrelateRequest {
        data: cor_data(),
        variables: Vec::new(),
        method,
        adjustment: PAdjust::Holm,
        conf_level: 0.95,
    };
    let (c, _) = engine().correlate(&request).unwrap();
    assert_eq!(c.validate(), Ok(()));
    c
}

#[test]
fn r_cor_test_examples() {
    // Pearson: t = 1.8411, df = 7, p-value = 0.1082, cor 0.5711816,
    // 95% CI [-0.1497426, 0.8955795].
    let c = cor(CorrelationMethod::Pearson);
    printed(v(&c, "r"), "0.5711816");
    assert_eq!(v(&c, "df"), 7.0);
    printed(v(&c, "p_value"), "0.1082");
    let ci = c.value("r").unwrap().ci.clone().unwrap();
    printed(ci.lo, "-0.1497426");
    printed(ci.hi, "0.8955795");
    // Kendall, alternative = "greater": T = 26, p-value = 0.05972, tau
    // 0.4444444. The exact null distribution is symmetric, so the
    // two-sided p the matrix reports is twice that.
    let c = cor(CorrelationMethod::Kendall);
    printed(v(&c, "r"), "0.4444444");
    printed(v(&c, "p_value") / 2.0, "0.05972");
    // Spearman, alternative = "greater": S = 48, p-value = 0.0484, rho 0.6.
    let c = cor(CorrelationMethod::Spearman);
    rel(v(&c, "r"), 0.6, 1e-12);
    printed(v(&c, "p_value") / 2.0, "0.0484");
}
