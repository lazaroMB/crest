//! The Bode / Nyquist card: the frequency response of
//! G(s) = N(s)/D(s)·e^(−sT) on a log-spaced sweep, its poles and zeros,
//! the gain and phase crossovers with their margins, and the closed-loop
//! poles under unity negative feedback.
//!
//! Magnitude and phase come from evaluating the polynomials at jω (Horner
//! in complex arithmetic). The phase is unwrapped along the sweep and
//! anchored at the first frequency to the sum of the factors' angles,
//! so an integrator starts at −90° rather than at an arbitrary multiple
//! of 360°. Roots are found with the Durand–Kerner iteration and polished
//! with Newton steps. Crossovers are bracketed on the sweep and refined
//! by bisection on the exact response: the gain crossover where |G| = 1
//! (phase margin = 180° + arg G there, wrapped to (−180°, 180°]), the
//! phase crossover where the unwrapped phase passes −180° + k·360° (gain
//! margin = −20·log10 |G| there). The margins reported are the smallest.

use application::ports::compute::{BodeRequest, ComputeError};
use domain::computed::{
    Assumption, AssumptionStatus, BodeBody, BodePoint, Complex, Computed, ComputedBody, Crossover,
    Narrative, WarningSeverity, apa,
};
use serde_json::Value;

use crate::envelope::{self, labelled, number, warning};

/// Highest polynomial degree accepted.
pub const MAX_DEGREE: usize = 30;
/// Fewest and most sweep points.
const POINTS: (u32, u32) = (50, 5000);

fn invalid(text: impl Into<String>) -> ComputeError {
    ComputeError::Invalid(text.into())
}

/// A complex number with the few operations the card needs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct C {
    /// Real part.
    pub re: f64,
    /// Imaginary part.
    pub im: f64,
}

impl C {
    /// `re + j·im`.
    #[must_use]
    pub const fn new(re: f64, im: f64) -> Self {
        Self { re, im }
    }
    fn add(self, o: Self) -> Self {
        Self::new(self.re + o.re, self.im + o.im)
    }
    fn sub(self, o: Self) -> Self {
        Self::new(self.re - o.re, self.im - o.im)
    }
    fn mul(self, o: Self) -> Self {
        Self::new(
            self.re * o.re - self.im * o.im,
            self.re * o.im + self.im * o.re,
        )
    }
    fn div(self, o: Self) -> Self {
        let d = o.re * o.re + o.im * o.im;
        Self::new(
            (self.re * o.re + self.im * o.im) / d,
            (self.im * o.re - self.re * o.im) / d,
        )
    }
    fn abs(self) -> f64 {
        self.re.hypot(self.im)
    }
    fn arg(self) -> f64 {
        self.im.atan2(self.re)
    }
}

/// `p` (highest power first, complex coefficients) at `x`.
fn horner(p: &[C], x: C) -> C {
    p.iter().fold(C::new(0.0, 0.0), |acc, c| acc.mul(x).add(*c))
}

fn real(p: &[f64]) -> Vec<C> {
    p.iter().map(|c| C::new(*c, 0.0)).collect()
}

/// The roots of `p` (real coefficients, highest power first, leading
/// coefficient non-zero), real ones with a zero imaginary part, sorted
/// by real then imaginary part.
#[must_use]
pub fn roots(p: &[f64]) -> Vec<Complex> {
    // Roots at the origin are exact: the trailing zeros.
    let zeros_at_origin = p.iter().rev().take_while(|c| **c == 0.0).count();
    let q = &p[..p.len() - zeros_at_origin];
    let n = q.len().saturating_sub(1);
    let mut out: Vec<C> = vec![C::new(0.0, 0.0); zeros_at_origin];
    if n > 0 {
        let lead = q[0];
        let monic: Vec<C> = q.iter().map(|c| C::new(c / lead, 0.0)).collect();
        let bound = 1.0 + monic[1..].iter().map(|c| c.abs()).fold(0.0, f64::max);
        let seed = C::new(0.4, 0.9);
        let mut z: Vec<C> = (0..n)
            .map(|k| {
                let mut w = C::new(1.0, 0.0);
                for _ in 0..=k {
                    w = w.mul(seed);
                }
                C::new(w.re * bound, w.im * bound)
            })
            .collect();
        for _ in 0..5000 {
            let mut change: f64 = 0.0;
            for i in 0..n {
                let mut den = C::new(1.0, 0.0);
                for j in 0..n {
                    if i != j {
                        den = den.mul(z[i].sub(z[j]));
                    }
                }
                if den.abs() == 0.0 {
                    den = C::new(1e-12, 0.0);
                }
                let step = horner(&monic, z[i]).div(den);
                z[i] = z[i].sub(step);
                change = change.max(step.abs() / (1.0 + z[i].abs()));
            }
            if change < 1e-15 {
                break;
            }
        }
        // Newton polish on the original polynomial.
        let derivative: Vec<C> = monic[..n]
            .iter()
            .enumerate()
            .map(|(i, c)| C::new(c.re * (n - i) as f64, c.im * (n - i) as f64))
            .collect();
        for r in &mut z {
            for _ in 0..3 {
                let d = horner(&derivative, *r);
                if d.abs() == 0.0 {
                    break;
                }
                let next = r.sub(horner(&monic, *r).div(d));
                if next.re.is_finite() && next.im.is_finite() {
                    *r = next;
                }
            }
        }
        out.extend(z);
    }
    let mut roots: Vec<Complex> = out
        .into_iter()
        .map(|c| {
            let scale = 1.0 + c.abs();
            Complex {
                re: if c.re.abs() < 1e-12 * scale {
                    0.0
                } else {
                    c.re
                },
                im: if c.im.abs() < 1e-8 * scale { 0.0 } else { c.im },
            }
        })
        .collect();
    roots.sort_by(|a, b| a.re.total_cmp(&b.re).then(b.im.total_cmp(&a.im)));
    roots
}

/// Π(s − r) times `gain`, highest power first; the coefficients must come
/// out real (complex roots in conjugate pairs).
fn expand(roots: &[Complex], gain: f64) -> Result<Vec<f64>, String> {
    let mut p = vec![C::new(1.0, 0.0)];
    for r in roots {
        let r = C::new(r.re, r.im);
        let mut next = vec![C::new(0.0, 0.0); p.len() + 1];
        for (i, c) in p.iter().enumerate() {
            next[i] = next[i].add(*c);
            next[i + 1] = next[i + 1].sub(c.mul(r));
        }
        p = next;
    }
    let scale = p.iter().map(|c| c.abs()).fold(1.0, f64::max);
    if p.iter().any(|c| c.im.abs() > 1e-9 * scale) {
        return Err("complex zeros and poles must come in conjugate pairs".into());
    }
    Ok(p.iter().map(|c| c.re * gain).collect())
}

/// The transfer function of the request: (numerator, denominator).
fn polynomials(request: &BodeRequest) -> Result<(Vec<f64>, Vec<f64>), String> {
    let by_roots = !request.poles.is_empty() || !request.zeros.is_empty();
    let by_polys = !request.denominator.is_empty() || !request.numerator.is_empty();
    let (num, den) = match (by_polys, by_roots) {
        (true, true) => {
            return Err(
                "give `numerator`/`denominator` or `zeros`/`poles`/`gain`, not both".into(),
            );
        }
        (false, false) => return Err("give `numerator` and `denominator`, or `poles`".into()),
        (true, false) => {
            let num = if request.numerator.is_empty() {
                vec![request.gain.unwrap_or(1.0)]
            } else {
                request
                    .numerator
                    .iter()
                    .map(|c| c * request.gain.unwrap_or(1.0))
                    .collect()
            };
            (num, request.denominator.clone())
        }
        (false, true) => (
            expand(&request.zeros, request.gain.unwrap_or(1.0))?,
            expand(&request.poles, 1.0)?,
        ),
    };
    let strip = |p: Vec<f64>| -> Vec<f64> {
        let first = p.iter().position(|c| *c != 0.0).unwrap_or(p.len());
        p[first..].to_vec()
    };
    let (num, den) = (strip(num), strip(den));
    if [&num, &den]
        .iter()
        .any(|p| p.iter().any(|c| !c.is_finite()))
    {
        return Err("coefficients must be finite numbers".into());
    }
    if den.is_empty() {
        return Err("the denominator is zero".into());
    }
    if num.is_empty() {
        return Err("the numerator is zero: |G| = 0 everywhere".into());
    }
    if num.len() > MAX_DEGREE + 1 || den.len() > MAX_DEGREE + 1 {
        return Err(format!("degrees above {MAX_DEGREE} are not supported"));
    }
    Ok((num, den))
}

/// The response at ω: G(jω), its magnitude in dB and its principal phase
/// in degrees.
struct Response<'a> {
    num: &'a [C],
    den: &'a [C],
    delay: f64,
}

impl Response<'_> {
    fn at(&self, w: f64) -> C {
        let s = C::new(0.0, w);
        let g = horner(self.num, s).div(horner(self.den, s));
        let (sin, cos) = (-w * self.delay).sin_cos();
        g.mul(C::new(cos, sin))
    }
    fn db(&self, w: f64) -> f64 {
        20.0 * self.at(w).abs().log10()
    }
    /// Principal phase in degrees.
    fn phase(&self, w: f64) -> f64 {
        self.at(w).arg().to_degrees()
    }
    /// The phase at `w` on the branch nearest `near`.
    fn phase_near(&self, w: f64, near: f64) -> f64 {
        let p = self.phase(w);
        p + 360.0 * ((near - p) / 360.0).round()
    }
}

/// Sum of the factors' angles at ω (the phase without wrapping), degrees.
fn factor_phase(
    num: &[f64],
    den: &[f64],
    zeros: &[Complex],
    poles: &[Complex],
    w: f64,
    t: f64,
) -> f64 {
    let angle = |r: &Complex| (w - r.im).atan2(-r.re);
    let lead = num[0] / den[0];
    let sign = if lead < 0.0 {
        std::f64::consts::PI
    } else {
        0.0
    };
    (zeros.iter().map(angle).sum::<f64>() - poles.iter().map(angle).sum::<f64>() + sign - w * t)
        .to_degrees()
}

/// Bisection on `f` over [a, b] in log ω, given a sign change.
fn bisect(mut a: f64, mut b: f64, f: impl Fn(f64) -> f64) -> f64 {
    let fa = f(a);
    for _ in 0..80 {
        let m = (a * b).sqrt();
        if (f(m) < 0.0) == (fa < 0.0) {
            a = m;
        } else {
            b = m;
        }
        if b / a - 1.0 < 1e-13 {
            break;
        }
    }
    (a * b).sqrt()
}

fn wrap180(deg: f64) -> f64 {
    let x = (deg + 180.0).rem_euclid(360.0) - 180.0;
    if x <= -180.0 { x + 360.0 } else { x }
}

/// Runs a Bode request.
pub fn run(request: &BodeRequest, id: String) -> Result<(Computed, ComputedBody), ComputeError> {
    let (num, den) = polynomials(request).map_err(invalid)?;
    if !(request.delay.is_finite() && request.delay >= 0.0) {
        return Err(invalid("`delay` must be a non-negative number of seconds"));
    }
    if !(POINTS.0..=POINTS.1).contains(&request.points) {
        return Err(invalid(format!(
            "`points` must be between {} and {}",
            POINTS.0, POINTS.1
        )));
    }
    let zeros = if num.len() > 1 {
        roots(&num)
    } else {
        Vec::new()
    };
    let poles = if den.len() > 1 {
        roots(&den)
    } else {
        Vec::new()
    };
    let unit = request.unit;
    // The sweep: two decades around the poles and zeros (and 1/T).
    let mut scales: Vec<f64> = zeros
        .iter()
        .chain(&poles)
        .map(|r| r.re.hypot(r.im))
        .filter(|m| *m > 0.0)
        .collect();
    if request.delay > 0.0 {
        scales.push(1.0 / request.delay);
    }
    let lo_auto = scales.iter().copied().fold(f64::INFINITY, f64::min);
    let hi_auto = scales.iter().copied().fold(0.0, f64::max);
    let (lo_auto, hi_auto) = if scales.is_empty() {
        (1e-2, 1e2)
    } else {
        (
            10f64.powf(lo_auto.log10().floor() - 2.0),
            10f64.powf(hi_auto.log10().ceil() + 2.0),
        )
    };
    let w_min = request.w_min.map_or(lo_auto, |f| unit.to_rad(f));
    let w_max = request.w_max.map_or(hi_auto, |f| unit.to_rad(f));
    if !(w_min > 0.0 && w_max > w_min && w_max.is_finite()) {
        return Err(invalid("give 0 < w_min < w_max"));
    }
    let (num_c, den_c) = (real(&num), real(&den));
    let g = Response {
        num: &num_c,
        den: &den_c,
        delay: request.delay,
    };
    let n = request.points as usize;
    let ws: Vec<f64> = (0..n)
        .map(|i| {
            let f = i as f64 / (n - 1) as f64;
            (w_min.ln() + f * (w_max.ln() - w_min.ln())).exp()
        })
        .collect();
    let mut points: Vec<BodePoint> = Vec::with_capacity(n);
    for w in &ws {
        let value = g.at(*w);
        let phase = match points.last() {
            None => {
                let anchor = factor_phase(&num, &den, &zeros, &poles, *w, request.delay);
                g.phase_near(*w, anchor)
            }
            Some(prev) => g.phase_near(*w, prev.phase_deg),
        };
        points.push(BodePoint {
            w: *w,
            magnitude_db: 20.0 * value.abs().log10(),
            phase_deg: phase,
            re: value.re,
            im: value.im,
        });
    }
    if points
        .iter()
        .any(|p| !(p.magnitude_db.is_finite() && p.phase_deg.is_finite()))
    {
        return Err(invalid(
            "the response is infinite on the sweep (a pole on the jω axis): move w_min/w_max",
        ));
    }
    // Gain crossovers: |G| = 1.
    let mut gain_crossovers: Vec<Crossover> = Vec::new();
    for pair in points.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        if (a.magnitude_db < 0.0) != (b.magnitude_db < 0.0) {
            let w = bisect(a.w, b.w, |w| g.db(w));
            let phase = g.phase_near(w, a.phase_deg);
            gain_crossovers.push(Crossover {
                w,
                margin: wrap180(phase + 180.0),
            });
        }
    }
    // Phase crossovers: −180° + k·360°.
    let level = |phase: f64| ((phase + 180.0) / 360.0).floor();
    let mut phase_crossovers: Vec<Crossover> = Vec::new();
    for pair in points.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        let (ka, kb) = (level(a.phase_deg), level(b.phase_deg));
        if ka != kb {
            let target = 360.0 * ka.max(kb) - 180.0;
            let w = bisect(a.w, b.w, |w| g.phase_near(w, a.phase_deg) - target);
            phase_crossovers.push(Crossover {
                w,
                margin: -g.db(w),
            });
        }
    }
    let pm = gain_crossovers
        .iter()
        .min_by(|x, y| x.margin.total_cmp(&y.margin))
        .copied();
    let gm = phase_crossovers
        .iter()
        .min_by(|x, y| x.margin.total_cmp(&y.margin))
        .copied();
    // Closed loop under unity negative feedback: roots of D + N.
    let closed_loop_poles = if request.delay == 0.0 {
        let len = num.len().max(den.len());
        let pad = |p: &[f64]| {
            let mut v = vec![0.0; len - p.len()];
            v.extend_from_slice(p);
            v
        };
        let sum: Vec<f64> = pad(&num)
            .iter()
            .zip(pad(&den))
            .map(|(a, b)| a + b)
            .collect();
        let first = sum.iter().position(|c| *c != 0.0).unwrap_or(sum.len());
        let sum = &sum[first..];
        if sum.len() > 1 {
            roots(sum)
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };
    let origin = |r: &&Complex| r.re == 0.0 && r.im == 0.0;
    let integrators = poles.iter().filter(origin).count();
    let differentiators = zeros.iter().filter(origin).count();
    let unstable_open = poles
        .iter()
        .filter(|p| p.re > 1e-9 * (1.0 + p.im.abs()))
        .count();
    // Envelope.
    let mut engine = envelope::solve_engine();
    engine.backend = Some("crest frequency response (Durand–Kerner roots)".into());
    let mut computed = Computed::new(id, "solve.bode", "Frequency response", engine);
    computed.inputs.push(domain::computed::InputRef {
        name: "transfer function".into(),
        role: Some("param".into()),
        path: None,
        hash: Some(envelope::hash(
            &serde_json::to_vec(request).unwrap_or_default(),
        )),
        rows: None,
        columns: Vec::new(),
        filter: None,
    });
    let f = |w: f64| unit.from_rad(w);
    let values = &mut computed.values;
    let freq = |w: f64| {
        let mut v = envelope::significant(f(w), 4);
        v.unit = Some(unit.symbol().into());
        v
    };
    if let Some(pm) = pm {
        let mut v = number(pm.margin, 2);
        v.unit = Some("°".into());
        values.insert("phase_margin".into(), v);
        values.insert("gain_crossover".into(), freq(pm.w));
    }
    if let Some(gm) = gm {
        let mut v = number(gm.margin, 2);
        v.unit = Some("dB".into());
        values.insert("gain_margin".into(), v);
        values.insert("phase_crossover".into(), freq(gm.w));
    }
    let dc =
        (integrators == 0 && differentiators == 0).then(|| num[num.len() - 1] / den[den.len() - 1]);
    if let Some(dc) = dc {
        let db = 20.0 * dc.abs().log10();
        let mut v = number(db, 2);
        v.unit = Some("dB".into());
        values.insert("dc_gain".into(), v);
        // Where |G| first falls 3 dB (half power) below the DC gain.
        let target = db - 10.0 * 2f64.log10();
        if let Some(pair) = points
            .windows(2)
            .find(|p| p[0].magnitude_db > target && p[1].magnitude_db <= target)
        {
            let w = bisect(pair[0].w, pair[1].w, |w| g.db(w) - target);
            values.insert("bandwidth".into(), freq(w));
        }
    }
    if !closed_loop_poles.is_empty() {
        let max_re = closed_loop_poles
            .iter()
            .map(|p| p.re)
            .fold(f64::NEG_INFINITY, f64::max);
        let stable = max_re < 0.0;
        values.insert(
            "closed_loop_max_re".into(),
            labelled(
                envelope::significant(max_re, 4),
                if stable { "stable" } else { "unstable" },
                "closed-loop poles of G/(1+G) all in the left half-plane",
            ),
        );
    }
    let params = &mut computed.method.params;
    let body = BodeBody {
        numerator: num.clone(),
        denominator: den.clone(),
        delay: request.delay,
        unit,
        points,
        zeros,
        poles,
        gain_crossovers,
        phase_crossovers,
        closed_loop_poles,
    };
    params.insert(
        "transfer_function".into(),
        Value::from(body.transfer_text()),
    );
    params.insert("unit".into(), Value::from(unit.symbol()));
    computed.method.why = Some(format!(
        "G(s) = {} evaluated at s = jω on {} log-spaced frequencies",
        body.transfer_text(),
        request.points
    ));
    computed.method.alternatives = Vec::new();
    if pm.is_none() {
        computed.warnings.push(warning(
            "no_gain_crossover",
            WarningSeverity::Info,
            "|G| does not cross 0 dB on the sweep: the phase margin is not defined there",
        ));
    }
    if gm.is_none() {
        computed.warnings.push(warning(
            "no_phase_crossover",
            WarningSeverity::Info,
            "the phase does not reach −180° on the sweep: the gain margin is infinite there",
        ));
    }
    if unstable_open > 0 {
        computed.warnings.push(warning(
            "unstable_open_loop",
            WarningSeverity::Warn,
            format!(
                "the open loop has {unstable_open} pole(s) in the right half-plane: positive \
                 margins do not by themselves mean a stable closed loop (count Nyquist \
                 encirclements of −1)"
            ),
        ));
    }
    if body.gain_crossovers.len() > 1 {
        computed.warnings.push(warning(
            "several_crossovers",
            WarningSeverity::Info,
            format!(
                "|G| crosses 0 dB {} times; the smallest phase margin is reported",
                body.gain_crossovers.len()
            ),
        ));
    }
    computed.assumptions.push(Assumption {
        id: "lti".into(),
        check: None,
        stat: None,
        p: None,
        status: AssumptionStatus::NotChecked,
        consequence: None,
        note: Some("linear time-invariant model; margins assume unity negative feedback".into()),
    });
    if request.delay > 0.0 {
        computed.assumptions.push(Assumption {
            id: "delay".into(),
            check: None,
            stat: None,
            p: None,
            status: AssumptionStatus::NotChecked,
            consequence: Some("closed-loop poles not computed".into()),
            note: Some(format!(
                "dead time e^(−{}s) applied exactly in the frequency response",
                apa::significant(request.delay, 4)
            )),
        });
    }
    let (n_text, d_text) = (format!("{num:?}"), format!("{den:?}"));
    computed.reproduce.python = Some(format!(
        "import scipy.signal as sg; w, mag, phase = sg.bode(sg.TransferFunction({n_text}, {d_text}))"
    ));
    computed.reproduce.r = Some(format!(
        "control::bode(control::tf({}, {}))",
        r_vector(&num),
        r_vector(&den)
    ));
    computed.narrative = Some(Narrative {
        template: "bode".into(),
        text_slots: vec![
            "phase_margin".into(),
            "gain_crossover".into(),
            "gain_margin".into(),
            "phase_crossover".into(),
        ],
    });
    Ok((computed, ComputedBody::Bode(body)))
}

fn r_vector(p: &[f64]) -> String {
    let parts: Vec<String> = p.iter().map(f64::to_string).collect();
    format!("c({})", parts.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roots_of_known_polynomials() {
        let r = roots(&[1.0, 3.0, 2.0]);
        assert!((r[0].re + 2.0).abs() < 1e-12 && (r[1].re + 1.0).abs() < 1e-12);
        let r = roots(&[1.0, 1.0, 1.0]);
        assert!((r[0].re + 0.5).abs() < 1e-12);
        assert!((r[0].im - 0.75f64.sqrt()).abs() < 1e-12);
        assert!((r[1].im + 0.75f64.sqrt()).abs() < 1e-12);
        let r = roots(&[1.0, 1.0, 0.0]);
        assert_eq!(r[1], Complex { re: 0.0, im: 0.0 });
        let p = expand(
            &[
                Complex { re: -1.0, im: 2.0 },
                Complex { re: -1.0, im: -2.0 },
            ],
            3.0,
        );
        assert_eq!(p, Ok(vec![3.0, 6.0, 15.0]));
        assert!(expand(&[Complex { re: -1.0, im: 2.0 }], 1.0).is_err());
        assert!((wrap180(-320.0) - 40.0).abs() < 1e-12);
        assert!((wrap180(180.0) - 180.0).abs() < 1e-12);
    }
}
