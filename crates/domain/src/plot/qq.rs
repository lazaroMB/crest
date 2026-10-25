//! Normal quantiles for QQ plots of residuals.

/// The standard normal quantile of `p` (the inverse of its CDF), by
/// Acklam's rational approximation (relative error below 1.2e-9). NaN
/// outside `0 < p < 1`.
#[must_use]
pub fn normal_quantile(p: f64) -> f64 {
    const A: [f64; 6] = [
        -3.969_683_028_665_376e1,
        2.209_460_984_245_205e2,
        -2.759_285_104_469_687e2,
        1.383_577_518_672_69e2,
        -3.066_479_806_614_716e1,
        2.506_628_277_459_239,
    ];
    const B: [f64; 5] = [
        -5.447_609_879_822_406e1,
        1.615_858_368_580_409e2,
        -1.556_989_798_598_866e2,
        6.680_131_188_771_972e1,
        -1.328_068_155_288_572e1,
    ];
    const C: [f64; 6] = [
        -7.784_894_002_430_293e-3,
        -3.223_964_580_411_365e-1,
        -2.400_758_277_161_838,
        -2.549_732_539_343_734,
        4.374_664_141_464_968,
        2.938_163_982_698_783,
    ];
    const D: [f64; 4] = [
        7.784_695_709_041_462e-3,
        3.224_671_290_700_398e-1,
        2.445_134_137_142_996,
        3.754_408_661_907_416,
    ];
    const LOW: f64 = 0.024_25;
    if !(p > 0.0 && p < 1.0) {
        return f64::NAN;
    }
    let tail = |q: f64| {
        (((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5])
            / ((((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0)
    };
    if p < LOW {
        tail((-2.0 * p.ln()).sqrt())
    } else if p > 1.0 - LOW {
        -tail((-2.0 * (1.0 - p).ln()).sqrt())
    } else {
        let q = p - 0.5;
        let r = q * q;
        (((((A[0] * r + A[1]) * r + A[2]) * r + A[3]) * r + A[4]) * r + A[5]) * q
            / (((((B[0] * r + B[1]) * r + B[2]) * r + B[3]) * r + B[4]) * r + 1.0)
    }
}

/// The points of a normal QQ plot of `sample`: (theoretical quantile,
/// sample value) in ascending order, with Blom's plotting positions
/// `(i − 3/8) / (n + 1/4)`. Non-finite values are left out.
#[must_use]
pub fn normal_qq(sample: &[f64]) -> Vec<(f64, f64)> {
    let mut sorted: Vec<f64> = sample.iter().copied().filter(|v| v.is_finite()).collect();
    sorted.sort_by(f64::total_cmp);
    let n = sorted.len() as f64;
    sorted
        .into_iter()
        .enumerate()
        .map(|(i, v)| (normal_quantile((i as f64 + 1.0 - 0.375) / (n + 0.25)), v))
        .collect()
}

/// The `p` quantile of ascending `sorted` by linear interpolation (R's
/// type 7), or `None` when it is empty.
fn quantile(sorted: &[f64], p: f64) -> Option<f64> {
    let last = sorted.len().checked_sub(1)?;
    let h = last as f64 * p;
    let lo = h.floor() as usize;
    let hi = (lo + 1).min(last);
    let (a, b) = (*sorted.get(lo)?, *sorted.get(hi)?);
    Some(a + (h - lo as f64) * (b - a))
}

/// The reference line of a QQ plot as `(intercept, slope)`: through the
/// first and third quartiles, as R's `qqline` draws it. `None` with fewer
/// than two distinct points.
#[must_use]
pub fn qq_line(points: &[(f64, f64)]) -> Option<(f64, f64)> {
    let sample: Vec<f64> = points.iter().map(|p| p.1).collect();
    let (y1, y3) = (quantile(&sample, 0.25)?, quantile(&sample, 0.75)?);
    let (x1, x3) = (normal_quantile(0.25), normal_quantile(0.75));
    let slope = (y3 - y1) / (x3 - x1);
    (points.len() >= 2 && slope.is_finite()).then_some((y1 - slope * x1, slope))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantiles_match_published_values() {
        // qnorm() in R.
        for (p, z) in [
            (0.5, 0.0),
            (0.975, 1.959_963_984_540_054),
            (0.025, -1.959_963_984_540_054),
            (0.75, 0.674_489_750_196_081_7),
            (0.001, -3.090_232_306_167_814),
            (0.999_99, 4.264_890_793_922_602),
        ] {
            let got = normal_quantile(p);
            assert!((got - z).abs() < 1e-8, "{p}: {got} vs {z}");
        }
        assert!(normal_quantile(0.0).is_nan());
        assert!(normal_quantile(1.0).is_nan());
        assert!(normal_quantile(f64::NAN).is_nan());
    }

    #[test]
    fn qq_points_are_sorted_and_symmetric() {
        let points = normal_qq(&[3.0, -1.0, f64::NAN, 0.0, 2.0, 1.0]);
        assert_eq!(points.len(), 5);
        let ys: Vec<f64> = points.iter().map(|p| p.1).collect();
        assert_eq!(ys, [-1.0, 0.0, 1.0, 2.0, 3.0]);
        assert!(points[2].0.abs() < 1e-12);
        assert!((points[0].0 + points[4].0).abs() < 1e-12);
        // qqnorm(1:5) in R uses ppoints(5) = (i - 3/8) / (5 + 1/4).
        assert!((points[4].0 - 1.179_761_118_074_06).abs() < 1e-8);
    }

    #[test]
    fn qq_line_goes_through_the_quartiles() {
        let points = normal_qq(&[1.0, 2.0, 3.0, 4.0, 5.0]);
        let (intercept, slope) = qq_line(&points).unwrap();
        // Quartiles 2 and 4 against qnorm(0.25) and qnorm(0.75).
        assert!((intercept - 3.0).abs() < 1e-12);
        assert!((slope - 2.0 / (2.0 * 0.674_489_750_196_081_7)).abs() < 1e-8);
        assert_eq!(qq_line(&normal_qq(&[1.0])), None);
    }
}
