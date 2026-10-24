//! Assumption checks: Shapiro-Wilk normality and Brown-Forsythe equality
//! of variances.

use crate::describe::quantile;
use crate::math::{pf_upper, pnorm_upper, qnorm};

/// A check result: statistic and p-value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Check {
    /// Statistic (W, F).
    pub stat: f64,
    /// p-value.
    pub p: f64,
}

fn poly(coefficients: &[f64], x: f64) -> f64 {
    coefficients.iter().rev().fold(0.0, |acc, c| acc * x + c)
}

/// Shapiro-Wilk W and its p-value by Royston's algorithm (AS R94, 1995),
/// as R's `shapiro.test`; `None` outside 3 ≤ n ≤ 5000 or for constant data.
pub fn shapiro_wilk(xs: &[f64]) -> Option<Check> {
    const C1: [f64; 6] = [0.0, 0.221_157, -0.147_981, -2.071_19, 4.434_685, -2.706_056];
    const C2: [f64; 6] = [
        0.0, 0.042_981, -0.293_762, -1.752_461, 5.682_633, -3.582_633,
    ];
    const C3: [f64; 4] = [0.544, -0.399_78, 0.025_054, -6.714e-4];
    const C4: [f64; 4] = [1.3822, -0.778_57, 0.062_767, -0.002_032_2];
    const C5: [f64; 4] = [-1.5861, -0.310_82, -0.083_751, 0.003_891_5];
    const C6: [f64; 3] = [-0.4803, -0.082_676, 0.003_030_2];
    const G: [f64; 2] = [-2.273, 0.459];

    let n = xs.len();
    if !(3..=5000).contains(&n) {
        return None;
    }
    let mut x = xs.to_vec();
    x.sort_by(f64::total_cmp);
    let range = x[n - 1] - x[0];
    if range <= 1e-19 * x[n - 1].abs().max(1.0) {
        return None;
    }
    let an = n as f64;
    let half = n / 2;
    // Coefficients a[0..half] for the upper half of the order statistics.
    let mut a = vec![0.0; half];
    if n == 3 {
        a[0] = std::f64::consts::FRAC_1_SQRT_2;
    } else {
        let an25 = an + 0.25;
        let m: Vec<f64> = (1..=half)
            .map(|i| qnorm((i as f64 - 0.375) / an25))
            .collect();
        let summ2 = 2.0 * m.iter().map(|v| v * v).sum::<f64>();
        let ssumm2 = summ2.sqrt();
        let rsn = 1.0 / an.sqrt();
        let a1 = poly(&C1, rsn) - m[0] / ssumm2;
        let (first, fac) = if n > 5 {
            let a2 = -m[1] / ssumm2 + poly(&C2, rsn);
            let fac = ((summ2 - 2.0 * m[0] * m[0] - 2.0 * m[1] * m[1])
                / (1.0 - 2.0 * a1 * a1 - 2.0 * a2 * a2))
                .sqrt();
            a[1] = a2;
            (2, fac)
        } else {
            let fac = ((summ2 - 2.0 * m[0] * m[0]) / (1.0 - 2.0 * a1 * a1)).sqrt();
            (1, fac)
        };
        a[0] = a1;
        for i in first..half {
            a[i] = -m[i] / fac;
        }
    }
    // W: squared correlation of the data with the coefficients.
    let mean = x.iter().sum::<f64>() / an;
    let ssq: f64 = x.iter().map(|v| (v - mean).powi(2)).sum();
    let num: f64 = (0..half).map(|i| a[i] * (x[n - 1 - i] - x[i])).sum();
    let w = (num * num / ssq).min(1.0);

    if n == 3 {
        let pi6 = 6.0 / std::f64::consts::PI;
        let stqr = std::f64::consts::FRAC_PI_3;
        let p = (pi6 * (w.sqrt().asin() - stqr)).max(0.0);
        return Some(Check { stat: w, p });
    }
    let w1 = (1.0 - w).ln();
    let p = if n <= 11 {
        let gamma = poly(&G, an);
        if w1 >= gamma {
            1e-99
        } else {
            let y = -(gamma - w1).ln();
            let m = poly(&C3, an);
            let s = poly(&C4, an).exp();
            pnorm_upper((y - m) / s)
        }
    } else {
        let xx = an.ln();
        let m = poly(&C5, xx);
        let s = poly(&C6, xx).exp();
        pnorm_upper((w1 - m) / s)
    };
    Some(Check { stat: w, p })
}

/// Brown-Forsythe test (Levene's test on deviations from the median) for
/// equal variances across `groups`; `None` with fewer than two groups of
/// two or when every deviation is zero.
pub fn brown_forsythe(groups: &[&[f64]]) -> Option<Check> {
    let k = groups.len();
    if k < 2 || groups.iter().any(|g| g.len() < 2) {
        return None;
    }
    let deviations: Vec<Vec<f64>> = groups
        .iter()
        .map(|g| {
            let mut sorted = g.to_vec();
            sorted.sort_by(f64::total_cmp);
            let median = quantile(&sorted, 0.5);
            g.iter().map(|x| (x - median).abs()).collect()
        })
        .collect();
    let n: usize = deviations.iter().map(Vec::len).sum();
    let grand = deviations.iter().flatten().sum::<f64>() / n as f64;
    let means: Vec<f64> = deviations
        .iter()
        .map(|d| d.iter().sum::<f64>() / d.len() as f64)
        .collect();
    let between: f64 = deviations
        .iter()
        .zip(&means)
        .map(|(d, m)| d.len() as f64 * (m - grand).powi(2))
        .sum();
    let within: f64 = deviations
        .iter()
        .zip(&means)
        .map(|(d, m)| d.iter().map(|z| (z - m).powi(2)).sum::<f64>())
        .sum();
    if within <= 0.0 {
        return None;
    }
    let df1 = (k - 1) as f64;
    let df2 = (n - k) as f64;
    let f = (between / df1) / (within / df2);
    Some(Check {
        stat: f,
        p: pf_upper(f, df1, df2),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapiro_wilk_separates_normal_from_skewed_data() {
        // Normal scores: W close to 1, large p.
        let normal: Vec<f64> = (1..=30)
            .map(|i| qnorm((f64::from(i) - 0.375) / 30.25))
            .collect();
        let check = shapiro_wilk(&normal).unwrap_or_else(|| panic!());
        assert!(check.stat > 0.98 && check.p > 0.5, "{check:?}");
        // Exponential scores: clearly skewed.
        let skewed: Vec<f64> = (1..=30)
            .map(|i| -(1.0 - (f64::from(i) - 0.5) / 30.0).ln())
            .collect();
        let check = shapiro_wilk(&skewed).unwrap_or_else(|| panic!());
        assert!(check.p < 0.01, "{check:?}");
        // n = 3 has a closed form: equally spaced points give W = 1.
        let check = shapiro_wilk(&[1.0, 2.0, 3.0]).unwrap_or_else(|| panic!());
        assert!((check.stat - 1.0).abs() < 1e-12 && (check.p - 1.0).abs() < 1e-9);
        assert!(shapiro_wilk(&[1.0, 1.0, 1.0]).is_none());
    }

    /// Normal samples from a xorshift generator and Box-Muller.
    fn normal_samples(seed: u64) -> impl FnMut(usize) -> Vec<f64> {
        let mut state = seed;
        let mut uniform = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            ((state >> 11) as f64 + 0.5) / (1u64 << 53) as f64
        };
        move |n| {
            (0..n)
                .map(|_| {
                    let (u, v) = (uniform(), uniform());
                    (-2.0 * u.ln()).sqrt() * (2.0 * std::f64::consts::PI * v).cos()
                })
                .collect()
        }
    }

    #[test]
    fn shapiro_wilk_p_values_are_calibrated_under_normality() {
        // Under H0 about 5% of p-values fall below .05; 4000 replicates
        // give a standard error of 0.34 percentage points.
        for (n, seed) in [(8, 7), (20, 11), (60, 13)] {
            let mut sample = normal_samples(seed);
            let reps = 4000;
            let rejected = (0..reps)
                .filter(|_| shapiro_wilk(&sample(n)).is_some_and(|c| c.p < 0.05))
                .count();
            let rate = rejected as f64 / f64::from(reps);
            assert!((0.035..0.065).contains(&rate), "n = {n}: rate {rate}");
        }
    }

    #[test]
    fn brown_forsythe_flags_unequal_spread() {
        let a = [9.0, 10.0, 11.0, 10.0, 9.5, 10.5];
        let b = [0.0, 20.0, 5.0, 15.0, 2.0, 18.0];
        let check = brown_forsythe(&[&a, &b]).unwrap_or_else(|| panic!());
        assert!(check.p < 0.01, "{check:?}");
    }
}
