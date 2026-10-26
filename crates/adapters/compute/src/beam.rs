//! The beam card: a straight Euler–Bernoulli beam on pins, rollers and
//! fixed supports, solved by the direct stiffness method.
//!
//! Nodes sit at the ends, every support, point load, moment and the ends
//! of every distributed load. Each element has the cubic (Hermite) stiffness of a
//! prismatic beam and the consistent nodal loads of a linearly varying
//! distributed load, so the nodal deflections and rotations are exact
//! for Euler–Bernoulli theory. The reduced system is symmetric positive
//! definite with a half-bandwidth of three and is solved by a banded
//! Cholesky factorisation; a zero pivot means the beam is not held (a
//! mechanism). Reactions are K·u − F at the restrained degrees of
//! freedom. Shear and moment then follow from statics on the left part
//! of the beam, exactly, with both sides of every jump sampled and the
//! moment's extremes refined where the shear crosses zero; sections lie
//! about `length / segments` apart. With E·I given, the deflection
//! between nodes is w_a + θ_a·(x − a) + ∫(x − ξ)·M(ξ)/EI dξ from the
//! element's left node, exact by Gauss–Legendre quadrature since M is at
//! most cubic there (uniform E·I).

use application::ports::compute::{BeamRequest, ComputeError};
use domain::computed::{
    Assumption, AssumptionStatus, BeamBody, BeamLoad, BeamLoadKind, BeamReaction, BeamSupport,
    Computed, ComputedBody, ComputedValue, Narrative, SupportKind, WarningSeverity, apa,
};
use serde_json::Value;

use crate::envelope::{self, significant, warning};

/// Fewest and most elements between key points.
const SEGMENTS: (u32, u32) = (10, 1000);
/// Most supports and loads.
pub const MAX_ITEMS: usize = 200;

fn invalid(text: impl Into<String>) -> ComputeError {
    ComputeError::Invalid(text.into())
}

/// Statics of the left part of the beam, from the reactions and loads.
struct Statics<'a> {
    reactions: &'a [BeamReaction],
    loads: &'a [BeamLoad],
    length: f64,
}

/// Start, end and the two intensities of a distributed load.
fn extent(load: &BeamLoad, length: f64) -> (f64, f64, f64, f64) {
    let end = load.end.unwrap_or(length);
    (
        load.at,
        end,
        load.value,
        load.end_value.unwrap_or(load.value),
    )
}

impl Statics<'_> {
    /// Shear and moment at `x`, counting what sits exactly at `x` when
    /// `inclusive` (the right side of a jump).
    fn at(&self, x: f64, inclusive: bool) -> (f64, f64) {
        let left = |a: f64| if inclusive { a <= x } else { a < x };
        let (mut v, mut m) = (0.0, 0.0);
        for r in self.reactions.iter().filter(|r| left(r.at)) {
            v += r.force;
            m += r.force * (x - r.at) - r.moment.unwrap_or(0.0);
        }
        for load in self.loads {
            match load.kind {
                BeamLoadKind::Point if left(load.at) => {
                    v -= load.value;
                    m -= load.value * (x - load.at);
                }
                BeamLoadKind::Moment if left(load.at) => m -= load.value,
                BeamLoadKind::Distributed if load.at < x => {
                    let (s, e, qs, qe) = extent(load, self.length);
                    let slope = (qe - qs) / (e - s);
                    let l = x.min(e) - s;
                    let d = x - s;
                    v -= qs * l + slope * l * l / 2.0;
                    m -= qs * (d * l - l * l / 2.0) + slope * (d * l * l / 2.0 - l * l * l / 3.0);
                }
                _ => {}
            }
        }
        (v, m)
    }
}

/// Solves the symmetric positive definite band system `a·x = b` with
/// half-bandwidth `w`, in place. `None` on a non-positive pivot.
fn band_cholesky(a: &mut [Vec<f64>], b: &mut [f64], w: usize) -> Option<()> {
    let n = b.len();
    let scale = (0..n)
        .map(|i| a[i][i].abs())
        .fold(0.0, f64::max)
        .max(1e-300);
    for j in 0..n {
        let lo = j.saturating_sub(w);
        let d = a[j][j] - a[j][lo..j].iter().map(|x| x * x).sum::<f64>();
        if d <= 1e-10 * scale {
            return None;
        }
        let d = d.sqrt();
        a[j][j] = d;
        for i in j + 1..(j + w + 1).min(n) {
            let from = i.saturating_sub(w).max(lo);
            let dot: f64 = a[i][from..j]
                .iter()
                .zip(&a[j][from..j])
                .map(|(x, y)| x * y)
                .sum();
            a[i][j] = (a[i][j] - dot) / d;
        }
    }
    for i in 0..n {
        let mut s = b[i];
        for k in i.saturating_sub(w)..i {
            s -= a[i][k] * b[k];
        }
        b[i] = s / a[i][i];
    }
    for i in (0..n).rev() {
        let mut s = b[i];
        for k in i + 1..(i + w + 1).min(n) {
            s -= a[k][i] * b[k];
        }
        b[i] = s / a[i][i];
    }
    Some(())
}

/// A value with a unit.
fn quantity(v: f64, unit: &str) -> ComputedValue {
    let mut value = significant(v, 4);
    value.unit = Some(unit.to_owned());
    value
}

fn check(request: &BeamRequest) -> Result<(), String> {
    let l = request.length;
    if !(l.is_finite() && l > 0.0) {
        return Err("`length` must be a positive number".into());
    }
    if request.supports.is_empty() {
        return Err("give at least one support".into());
    }
    if request.supports.len() + request.loads.len() > MAX_ITEMS {
        return Err(format!("at most {MAX_ITEMS} supports and loads"));
    }
    if !(SEGMENTS.0..=SEGMENTS.1).contains(&request.segments) {
        return Err(format!(
            "`segments` must be between {} and {}",
            SEGMENTS.0, SEGMENTS.1
        ));
    }
    let inside = |x: f64| x.is_finite() && (-1e-12 * l..=l * (1.0 + 1e-12)).contains(&x);
    for s in &request.supports {
        if !inside(s.at) {
            return Err(format!("a support at {} is outside the span", s.at));
        }
    }
    let mut at: Vec<f64> = request.supports.iter().map(|s| s.at).collect();
    at.sort_by(f64::total_cmp);
    if at.windows(2).any(|w| (w[1] - w[0]).abs() <= 1e-9 * l) {
        return Err("two supports at the same position: merge them into one".into());
    }
    let reactions: u32 = request.supports.iter().map(|s| s.kind.reactions()).sum();
    if reactions < 2 {
        return Err("the beam is not held: give two pins/rollers or one fixed support".into());
    }
    for load in &request.loads {
        if !(inside(load.at) && load.value.is_finite()) {
            return Err(format!("a load at {} is outside the span", load.at));
        }
        if load.kind == BeamLoadKind::Distributed {
            let (s, e, _, qe) = extent(load, l);
            if !(inside(e) && e > s && qe.is_finite()) {
                return Err(format!(
                    "the distributed load from {s} needs an `end` after it, within the span"
                ));
            }
        }
    }
    for (name, v) in [("e", request.e), ("i", request.i)] {
        if v.is_some_and(|v| !(v.is_finite() && v > 0.0)) {
            return Err(format!("`{name}` must be a positive number"));
        }
    }
    Ok(())
}

/// Runs a beam request.
#[allow(clippy::too_many_lines)]
pub fn run(request: &BeamRequest, id: String) -> Result<(Computed, ComputedBody), ComputeError> {
    check(request).map_err(invalid)?;
    let l = request.length;
    let snap = |x: f64| x.clamp(0.0, l);
    let mut supports: Vec<BeamSupport> = request
        .supports
        .iter()
        .map(|s| BeamSupport {
            at: snap(s.at),
            kind: s.kind,
        })
        .collect();
    supports.sort_by(|a, b| a.at.total_cmp(&b.at));
    let loads: Vec<BeamLoad> = request
        .loads
        .iter()
        .map(|load| BeamLoad {
            at: snap(load.at),
            end: load.end.map(snap),
            ..load.clone()
        })
        .collect();
    // Key points, then the mesh.
    let mut keys: Vec<f64> = vec![0.0, l];
    keys.extend(supports.iter().map(|s| s.at));
    for load in &loads {
        keys.push(load.at);
        if load.kind == BeamLoadKind::Distributed {
            keys.push(extent(load, l).1);
        }
    }
    keys.sort_by(f64::total_cmp);
    keys.dedup_by(|a, b| (*a - *b).abs() <= 1e-12 * l);
    // The stiffness model: one element between consecutive key points.
    // With consistent loads its nodal values are exact, so a finer mesh
    // would only cost digits.
    let nodes = keys.clone();
    let n = nodes.len();
    let node_of = |x: f64| {
        nodes
            .iter()
            .position(|p| (p - x).abs() <= 1e-9 * l)
            .unwrap_or(0)
    };
    let dofs = 2 * n;
    // Stiffness with E·I = 1 (reactions do not depend on a uniform E·I).
    let mut k = vec![vec![0.0; dofs]; dofs];
    let mut f = vec![0.0; dofs];
    for e in 0..n - 1 {
        let le = nodes[e + 1] - nodes[e];
        let (l2, l3) = (le * le, le * le * le);
        let ke = [
            [12.0 / l3, 6.0 / l2, -12.0 / l3, 6.0 / l2],
            [6.0 / l2, 4.0 / le, -6.0 / l2, 2.0 / le],
            [-12.0 / l3, -6.0 / l2, 12.0 / l3, -6.0 / l2],
            [6.0 / l2, 2.0 / le, -6.0 / l2, 4.0 / le],
        ];
        let map = [2 * e, 2 * e + 1, 2 * e + 2, 2 * e + 3];
        for (a, ra) in map.iter().enumerate() {
            for (b, rb) in map.iter().enumerate() {
                k[*ra][*rb] += ke[a][b];
            }
        }
        // Distributed loads on this element (it lies wholly inside or
        // outside each, since load ends are nodes).
        for load in loads.iter().filter(|l| l.kind == BeamLoadKind::Distributed) {
            let (s, end, qs, qe) = extent(load, l);
            let mid = (nodes[e] + nodes[e + 1]) / 2.0;
            if mid <= s || mid >= end {
                continue;
            }
            let q = |x: f64| qs + (qe - qs) * (x - s) / (end - s);
            let (q1, q2) = (q(nodes[e]), q(nodes[e + 1]));
            f[map[0]] -= (7.0 * q1 + 3.0 * q2) * le / 20.0;
            f[map[1]] -= (3.0 * q1 + 2.0 * q2) * l2 / 60.0;
            f[map[2]] -= (3.0 * q1 + 7.0 * q2) * le / 20.0;
            f[map[3]] += (2.0 * q1 + 3.0 * q2) * l2 / 60.0;
        }
    }
    for load in &loads {
        let node = node_of(load.at);
        match load.kind {
            BeamLoadKind::Point => f[2 * node] -= load.value,
            BeamLoadKind::Moment => f[2 * node + 1] += load.value,
            BeamLoadKind::Distributed => {}
        }
    }
    let mut fixed = vec![false; dofs];
    for s in &supports {
        let node = node_of(s.at);
        fixed[2 * node] = true;
        if s.kind == SupportKind::Fixed {
            fixed[2 * node + 1] = true;
        }
    }
    let free: Vec<usize> = (0..dofs).filter(|d| !fixed[*d]).collect();
    let mut a: Vec<Vec<f64>> = free
        .iter()
        .map(|i| free.iter().map(|j| k[*i][*j]).collect())
        .collect();
    let mut b: Vec<f64> = free.iter().map(|i| f[*i]).collect();
    // Jacobi scaling: translations and rotations differ by 1/h² in the
    // stiffness, which would cost digits.
    let scale: Vec<f64> = (0..free.len())
        .map(|i| {
            let d = a[i][i];
            if d > 0.0 { 1.0 / d.sqrt() } else { 1.0 }
        })
        .collect();
    for i in 0..free.len() {
        for j in i.saturating_sub(3)..(i + 4).min(free.len()) {
            a[i][j] *= scale[i] * scale[j];
        }
        b[i] *= scale[i];
    }
    band_cholesky(&mut a, &mut b, 3)
        .ok_or_else(|| invalid("the beam is not held (a mechanism): add or change a support"))?;
    let mut u = vec![0.0; dofs];
    for (ix, d) in free.iter().enumerate() {
        u[*d] = b[ix] * scale[ix];
    }
    let reaction = |d: usize| -> f64 { (0..dofs).map(|j| k[d][j] * u[j]).sum::<f64>() - f[d] };
    let names: Vec<String> = (0..supports.len())
        .map(|i| {
            let letter = char::from(b'A' + u8::try_from(i % 26).unwrap_or(0));
            if i < 26 {
                letter.to_string()
            } else {
                format!("{letter}{}", i / 26)
            }
        })
        .collect();
    let reactions: Vec<BeamReaction> = supports
        .iter()
        .zip(&names)
        .map(|(s, name)| {
            let node = node_of(s.at);
            BeamReaction {
                name: name.clone(),
                at: s.at,
                kind: s.kind,
                force: reaction(2 * node),
                moment: (s.kind == SupportKind::Fixed).then(|| reaction(2 * node + 1)),
            }
        })
        .collect();
    let statics = Statics {
        reactions: &reactions,
        loads: &loads,
        length: l,
    };
    // Sections: the key points (both sides where a force or moment sits)
    // and about `segments` between them.
    let jumps: Vec<f64> = supports
        .iter()
        .map(|s| s.at)
        .chain(
            loads
                .iter()
                .filter(|l| l.kind != BeamLoadKind::Distributed)
                .map(|l| l.at),
        )
        .collect();
    let h = l / f64::from(request.segments);
    let mut samples: Vec<(f64, f64, f64)> = Vec::new();
    for pair in nodes.windows(2) {
        let x0 = pair[0];
        if jumps.iter().any(|j| (j - x0).abs() <= 1e-9 * l) {
            let (v, m) = statics.at(x0, false);
            samples.push((x0, v, m));
        }
        let parts = ((pair[1] - x0) / h).ceil().max(1.0) as usize;
        for p in 0..parts {
            let x = x0 + (pair[1] - x0) * p as f64 / parts as f64;
            let (v, m) = statics.at(x, true);
            samples.push((x, v, m));
        }
        // Where the shear changes sign inside the element, the moment
        // peaks.
        let (va, _) = statics.at(x0, true);
        let (vb, _) = statics.at(pair[1], false);
        if va * vb < 0.0 {
            let (mut lo, mut hi) = (x0, pair[1]);
            for _ in 0..100 {
                let mid = (lo + hi) / 2.0;
                if (statics.at(mid, true).0 < 0.0) == (va < 0.0) {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            let x = (lo + hi) / 2.0;
            let (v, m) = statics.at(x, true);
            samples.push((x, v, m));
        }
    }
    if jumps.iter().any(|j| (j - l).abs() <= 1e-9 * l) {
        let (v, m) = statics.at(l, false);
        samples.push((l, v, m));
    }
    let (v, m) = statics.at(l, true);
    samples.push((l, v, m));
    samples.sort_by(|p, q| p.0.total_cmp(&q.0));
    // E·I in force × length² of the request's units.
    let ei_si = match (request.e, request.i) {
        (Some(e), Some(i)) => Some(e * request.e_unit.pascals() * i * request.i_unit.metres4()),
        _ => None,
    };
    let (len_m, force_n) = (request.length_unit.metres(), request.force_unit.newtons());
    let shown = request.length_unit.deflection_unit();
    let deflection: Vec<f64> = match ei_si {
        Some(ei_si) => {
            let ei = ei_si / (force_n * len_m * len_m);
            samples
                .iter()
                .map(|s| {
                    // From the element's left node: w(x) = w_a + θ_a·(x − a)
                    // + ∫_a^x (x − ξ)·M(ξ) dξ (times 1/EI), with M a cubic at
                    // most, so 4-point Gauss–Legendre is exact.
                    let e = nodes
                        .partition_point(|p| *p <= s.0)
                        .saturating_sub(1)
                        .min(n - 2);
                    let a = nodes[e];
                    let d = s.0 - a;
                    let mut w = u[2 * e] + u[2 * e + 1] * d;
                    if d > 0.0 {
                        const GAUSS: [(f64, f64); 4] = [
                            (-0.861_136_311_594_052_6, 0.347_854_845_137_453_9),
                            (-0.339_981_043_584_856_3, 0.652_145_154_862_546_1),
                            (0.339_981_043_584_856_3, 0.652_145_154_862_546_1),
                            (0.861_136_311_594_052_6, 0.347_854_845_137_453_9),
                        ];
                        let integral: f64 = GAUSS
                            .iter()
                            .map(|(t, wt)| {
                                let xi = a + d * (t + 1.0) / 2.0;
                                wt * (s.0 - xi) * statics.at(xi, true).1
                            })
                            .sum::<f64>()
                            * d
                            / 2.0;
                        w += integral;
                    }
                    w / ei * len_m / shown.metres()
                })
                .collect()
        }
        None => Vec::new(),
    };
    let x: Vec<f64> = samples.iter().map(|s| s.0).collect();
    let shear: Vec<f64> = samples.iter().map(|s| s.1).collect();
    let moment: Vec<f64> = samples.iter().map(|s| s.2).collect();
    // Equilibrium: forces and moments about the left end.
    let applied_force: f64 = loads
        .iter()
        .map(|load| match load.kind {
            BeamLoadKind::Point => load.value,
            BeamLoadKind::Distributed => {
                let (s, e, qs, qe) = extent(load, l);
                (qs + qe) / 2.0 * (e - s)
            }
            BeamLoadKind::Moment => 0.0,
        })
        .sum();
    let reaction_force: f64 = reactions.iter().map(|r| r.force).sum();
    let force_scale = applied_force.abs().max(reaction_force.abs()).max(1e-300);
    let residual = (reaction_force - applied_force).abs() / force_scale;
    let (_, end_moment) = statics.at(l, true);
    let (end_shear, _) = statics.at(l, true);
    let moment_scale = moment
        .iter()
        .map(|m| m.abs())
        .fold(force_scale * l, f64::max);
    let moment_residual = end_moment.abs() / moment_scale;
    // Envelope.
    let mut engine = envelope::solve_engine();
    engine.backend = Some("crest direct stiffness (Euler–Bernoulli)".into());
    let mut computed = Computed::new(id, "solve.beam", "Beam analysis", engine);
    computed.inputs.push(domain::computed::InputRef {
        name: "beam".into(),
        role: Some("param".into()),
        path: None,
        hash: Some(envelope::hash(
            &serde_json::to_vec(request).unwrap_or_default(),
        )),
        rows: None,
        columns: Vec::new(),
        filter: None,
    });
    let (lu, fu) = (request.length_unit.symbol(), request.force_unit.symbol());
    let mu = format!("{fu}·{lu}");
    let values = &mut computed.values;
    for r in &reactions {
        values.insert(format!("reaction.{}", r.name), quantity(r.force, fu));
        if let Some(m) = r.moment {
            values.insert(format!("moment.{}", r.name), quantity(m, &mu));
        }
    }
    let tiny = 1e-9 * moment_scale;
    let arg = |pick: &dyn Fn(f64, f64) -> bool, ys: &[f64]| -> Option<usize> {
        (0..ys.len()).fold(None, |best: Option<usize>, i| match best {
            Some(b) if !pick(ys[i], ys[b]) => Some(b),
            _ => Some(i),
        })
    };
    if let Some(i) = arg(&|a, b| a > b, &moment).filter(|i| moment[*i] > tiny) {
        values.insert("max_moment".into(), quantity(moment[i], &mu));
        values.insert("max_moment_at".into(), quantity(x[i], lu));
    }
    if let Some(i) = arg(&|a, b| a < b, &moment).filter(|i| moment[*i] < -tiny) {
        values.insert("min_moment".into(), quantity(moment[i], &mu));
        values.insert("min_moment_at".into(), quantity(x[i], lu));
    }
    if let Some(i) = arg(&|a, b| a.abs() > b.abs(), &shear) {
        values.insert("max_shear".into(), quantity(shear[i], fu));
        values.insert("max_shear_at".into(), quantity(x[i], lu));
    }
    if let Some(i) = arg(&|a, b| a.abs() > b.abs(), &deflection) {
        values.insert(
            "max_deflection".into(),
            quantity(deflection[i], shown.symbol()),
        );
        values.insert("max_deflection_at".into(), quantity(x[i], lu));
    }
    if let Some(ei) = ei_si {
        values.insert("ei".into(), quantity(ei, "N·m²"));
    }
    let indeterminacy = reactions
        .iter()
        .map(|r| r.kind.reactions())
        .sum::<u32>()
        .saturating_sub(2);
    values.insert(
        "indeterminacy".into(),
        envelope::count(u64::from(indeterminacy)),
    );
    let params = &mut computed.method.params;
    params.insert("nodes".into(), Value::from(n));
    params.insert("length_unit".into(), Value::from(lu));
    params.insert("force_unit".into(), Value::from(fu));
    computed.method.why = Some(if indeterminacy == 0 {
        "statically determinate; reactions from the stiffness model agree with statics".into()
    } else {
        format!(
            "statically indeterminate to degree {indeterminacy}: reactions from the stiffness \
             method with uniform E·I"
        )
    });
    computed.method.alternatives = Vec::new();
    computed.assumptions.push(Assumption {
        id: "euler_bernoulli".into(),
        check: None,
        stat: None,
        p: None,
        status: AssumptionStatus::NotChecked,
        consequence: None,
        note: Some(
            "Euler–Bernoulli beam: plane sections, no shear deformation (slender beams), \
             small deflections, linear elastic material, uniform E·I over the span"
                .into(),
        ),
    });
    computed.assumptions.push(Assumption {
        id: "equilibrium".into(),
        check: Some("ΣF = 0 and M(L) = 0".into()),
        stat: Some(residual.max(moment_residual)),
        p: None,
        status: if residual < 1e-9 && moment_residual < 1e-9 && end_shear.abs() < 1e-9 * force_scale
        {
            AssumptionStatus::Pass
        } else {
            AssumptionStatus::Warn
        },
        consequence: None,
        note: Some("reactions balance the loads; relative residual shown".into()),
    });
    if indeterminacy > 0 {
        computed.assumptions.push(Assumption {
            id: "uniform_ei".into(),
            check: None,
            stat: None,
            p: None,
            status: AssumptionStatus::NotChecked,
            consequence: Some("reactions depend on how stiffness varies along the beam".into()),
            note: Some(
                "indeterminate beam: a non-uniform section would change the reactions".into(),
            ),
        });
    }
    if ei_si.is_none() {
        computed.warnings.push(warning(
            "no_stiffness",
            WarningSeverity::Info,
            "no E and I given: deflection not computed",
        ));
    } else if let Some(d) = computed.value("max_deflection").map(|v| v.v) {
        // Small-deflection check: span/250 is a common serviceability
        // limit; far beyond it the linear theory itself is doubtful.
        let span = l * len_m / shown.metres();
        if d.abs() > span / 50.0 {
            computed.warnings.push(warning(
                "large_deflection",
                WarningSeverity::Warn,
                format!(
                    "the deflection is over span/50 ({} {}): small-deflection theory may not hold",
                    apa::significant(d.abs(), 3),
                    shown.symbol()
                ),
            ));
        }
    }
    computed.narrative = Some(Narrative {
        template: "beam".into(),
        text_slots: vec![
            "max_moment".into(),
            "min_moment".into(),
            "max_shear".into(),
            "max_deflection".into(),
        ],
    });
    let body = BeamBody {
        length: l,
        length_unit: request.length_unit,
        force_unit: request.force_unit,
        supports,
        loads,
        reactions,
        x,
        shear,
        moment,
        deflection,
        ei: ei_si,
        indeterminacy,
    };
    Ok((computed, ComputedBody::Beam(body)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn band_cholesky_solves_a_tridiagonal_system() {
        let mut a = vec![
            vec![4.0, 1.0, 0.0],
            vec![1.0, 4.0, 1.0],
            vec![0.0, 1.0, 4.0],
        ];
        let mut b = vec![5.0, 6.0, 5.0];
        assert!(band_cholesky(&mut a, &mut b, 1).is_some());
        for x in b {
            assert!((x - 1.0).abs() < 1e-12);
        }
        let mut singular = vec![vec![0.0]];
        assert!(band_cholesky(&mut singular, &mut [1.0], 1).is_none());
    }
}
