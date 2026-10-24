//! Distribution functions on top of statrs' special functions.
//!
//! Upper tails are computed directly (regularized incomplete beta and
//! gamma) rather than as `1 − cdf`, so small p-values keep their precision.
//! Invalid arguments give NaN instead of panicking; the envelope's
//! validation turns a NaN value into an error.

use std::f64::consts::{FRAC_1_SQRT_2, PI, SQRT_2};

use domain::computed::Alternative;
use statrs::function::beta::{checked_beta_reg, inv_beta_reg};
use statrs::function::erf::{erfc, erfc_inv};
use statrs::function::gamma::{checked_gamma_ur, ln_gamma};

/// P(Z ≤ z) for a standard normal Z.
pub fn pnorm(z: f64) -> f64 {
    0.5 * erfc(-z * FRAC_1_SQRT_2)
}

/// P(Z > z) for a standard normal Z.
pub fn pnorm_upper(z: f64) -> f64 {
    0.5 * erfc(z * FRAC_1_SQRT_2)
}

/// The standard normal quantile of `p`; NaN outside (0, 1).
pub fn qnorm(p: f64) -> f64 {
    if p > 0.0 && p < 1.0 {
        -SQRT_2 * erfc_inv(2.0 * p)
    } else {
        f64::NAN
    }
}

/// Regularized incomplete beta I_x(a, b); NaN on invalid arguments.
pub fn beta_reg(a: f64, b: f64, x: f64) -> f64 {
    checked_beta_reg(a, b, x).unwrap_or(f64::NAN)
}

/// P(T > t) for Student's t with `df` degrees of freedom.
pub fn pt_upper(t: f64, df: f64) -> f64 {
    if df.is_nan() || df <= 0.0 || t.is_nan() {
        return f64::NAN;
    }
    if t.is_infinite() {
        return if t > 0.0 { 0.0 } else { 1.0 };
    }
    let tail = 0.5 * beta_reg(df / 2.0, 0.5, df / (df + t * t));
    if t >= 0.0 { tail } else { 1.0 - tail }
}

/// P(T ≤ t) for Student's t.
pub fn pt(t: f64, df: f64) -> f64 {
    pt_upper(-t, df)
}

/// The Student t density.
fn dt(t: f64, df: f64) -> f64 {
    let ln = ln_gamma((df + 1.0) / 2.0)
        - ln_gamma(df / 2.0)
        - 0.5 * (df * PI).ln()
        - (df + 1.0) / 2.0 * (1.0 + t * t / df).ln();
    ln.exp()
}

/// The Student t quantile of `p`, refined by Newton steps on [`pt`].
pub fn qt(p: f64, df: f64) -> f64 {
    if !(p > 0.0 && p < 1.0) || df.is_nan() || df <= 0.0 {
        return f64::NAN;
    }
    if (p - 0.5).abs() < f64::EPSILON {
        return 0.0;
    }
    let lower = p.min(1.0 - p);
    let y = inv_beta_reg(df / 2.0, 0.5, 2.0 * lower);
    let mut t = -((df * (1.0 - y) / y).sqrt());
    for _ in 0..8 {
        let step = (pt(t, df) - lower) / dt(t, df);
        if !step.is_finite() {
            break;
        }
        t -= step;
        if step.abs() <= 1e-14 * t.abs().max(1.0) {
            break;
        }
    }
    if p > 0.5 { -t } else { t }
}

/// The p-value of a t statistic for `alternative`.
pub fn p_from_t(t: f64, df: f64, alternative: Alternative) -> f64 {
    match alternative {
        Alternative::TwoSided => {
            if df.is_nan() || df <= 0.0 || t.is_nan() {
                f64::NAN
            } else {
                beta_reg(df / 2.0, 0.5, df / (df + t * t))
            }
        }
        Alternative::Greater => pt_upper(t, df),
        Alternative::Less => pt_upper(-t, df),
    }
}

/// The p-value of a z statistic for `alternative`.
pub fn p_from_z(z: f64, alternative: Alternative) -> f64 {
    match alternative {
        Alternative::TwoSided => (2.0 * pnorm_upper(z.abs())).min(1.0),
        Alternative::Greater => pnorm_upper(z),
        Alternative::Less => pnorm(z),
    }
}

/// P(X > x) for χ² with `k` degrees of freedom.
pub fn pchisq_upper(x: f64, k: f64) -> f64 {
    if x <= 0.0 {
        return if k > 0.0 { 1.0 } else { f64::NAN };
    }
    checked_gamma_ur(k / 2.0, x / 2.0).unwrap_or(f64::NAN)
}

/// P(F > f) for Fisher's F with `d1` and `d2` degrees of freedom.
pub fn pf_upper(f: f64, d1: f64, d2: f64) -> f64 {
    if f <= 0.0 {
        return 1.0;
    }
    beta_reg(d2 / 2.0, d1 / 2.0, d2 / (d2 + d1 * f))
}

/// P(T ≤ t) for the noncentral t with `df` degrees of freedom and
/// noncentrality `delta`: algorithm AS 243 (Lenth 1989), as R's `pnt`.
/// `None` when the series does not converge or `delta` is beyond the
/// range the algorithm handles (|δ| > 37.62).
pub fn pnt(t: f64, df: f64, delta: f64) -> Option<f64> {
    const ITRMAX: u32 = 1000;
    const ERRMAX: f64 = 1e-12;
    if df.is_nan() || df <= 0.0 || !t.is_finite() || !delta.is_finite() || delta.abs() > 37.62 {
        return None;
    }
    let (tt, del, negdel) = if t < 0.0 {
        (-t, -delta, true)
    } else {
        (t, delta, false)
    };
    let x = tt * tt / (tt * tt + df);
    let mut tnc = 0.0;
    if x > 0.0 {
        let lambda = del * del;
        let mut p = 0.5 * (-0.5 * lambda).exp();
        let mut q = (2.0 / PI).sqrt() * p * del;
        let mut s = 0.5 - p;
        if s < 1e-7 {
            s = -0.5 * (-0.5 * lambda).exp_m1();
        }
        let mut a = 0.5;
        let b = 0.5 * df;
        let rxb = (1.0 - x).powf(b);
        let albeta = 0.5 * PI.ln() + ln_gamma(b) - ln_gamma(0.5 + b);
        let mut xodd = beta_reg(a, b, x);
        let mut godd = 2.0 * rxb * (a * x.ln() - albeta).exp();
        tnc = b * x;
        let mut xeven = if tnc < f64::EPSILON { tnc } else { 1.0 - rxb };
        let mut geven = tnc * rxb;
        tnc = p * xodd + q * xeven;
        let mut converged = false;
        for it in 1..=ITRMAX {
            let en = f64::from(it);
            a += 1.0;
            xodd -= godd;
            xeven -= geven;
            godd *= x * (a + b - 1.0) / a;
            geven *= x * (a + b - 0.5) / (a + 0.5);
            p *= lambda / (2.0 * en);
            q *= lambda / (2.0 * en + 1.0);
            s -= p;
            tnc += p * xodd + q * xeven;
            let errbd = 2.0 * s * (xodd - godd);
            if errbd.abs() < ERRMAX {
                converged = true;
                break;
            }
        }
        if !converged {
            return None;
        }
    }
    tnc += pnorm(-del);
    let tnc = tnc.clamp(0.0, 1.0);
    Some(if negdel { 1.0 - tnc } else { tnc })
}

/// The noncentrality `δ` with `P(T ≤ t; df, δ) = target`, by bisection
/// (the CDF falls as δ grows). `None` when it cannot be bracketed.
pub fn pnt_inverse_delta(t: f64, df: f64, target: f64) -> Option<f64> {
    let f = |delta: f64| pnt(t, df, delta).map(|p| p - target);
    let (mut lo, mut hi) = (t - 10.0, t + 10.0);
    let (mut flo, fhi) = (f(lo)?, f(hi)?);
    if flo < 0.0 || fhi > 0.0 {
        return None;
    }
    for _ in 0..200 {
        let mid = 0.5 * (lo + hi);
        let fmid = f(mid)?;
        if (fmid > 0.0) == (flo > 0.0) {
            lo = mid;
            flo = fmid;
        } else {
            hi = mid;
        }
        if hi - lo < 1e-12 * (1.0 + mid.abs()) {
            break;
        }
    }
    Some(0.5 * (lo + hi))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol * b.abs().max(1e-300)
    }

    #[test]
    fn central_distributions_match_tables() {
        assert!(close(qnorm(0.975), 1.959_963_984_540_054, 1e-12));
        assert!(close(qt(0.975, 9.0), 2.262_157_162_798_205, 1e-10));
        assert!(close(qt(0.025, 30.0), -2.042_272_456_301_238, 1e-10));
        assert!(close(pt(2.0, 5.0), 0.949_030_260_585_070_7, 1e-10));
        assert!(close(pchisq_upper(3.841_458_820_694_124, 1.0), 0.05, 1e-9));
        assert!(close(pf_upper(1.0, 3.0, 3.0), 0.5, 1e-12));
    }

    #[test]
    fn noncentral_t_reduces_to_central_and_inverts() {
        let p = pnt(1.5, 10.0, 0.0).unwrap_or(f64::NAN);
        assert!(close(p, pt(1.5, 10.0), 1e-10));
        let delta = pnt_inverse_delta(2.0, 18.0, 0.975).unwrap_or(f64::NAN);
        let back = pnt(2.0, 18.0, delta).unwrap_or(f64::NAN);
        assert!(close(back, 0.975, 1e-9));
        assert!(delta < 2.0);
    }
}
