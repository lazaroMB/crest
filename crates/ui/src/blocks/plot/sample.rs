//! Turning a [`PlotSpec`] into geometry, without drawing: the view (given,
//! or fitted robustly so an asymptote does not flatten the curve), curves
//! sampled adaptively with gaps at discontinuities, implicit curves traced
//! by marching squares, and the clipping that keeps far-off points out of
//! the paths. Everything is in data coordinates; [`super::axes`] maps it
//! to pixels.

use std::cell::RefCell;

use domain::plot::expr::Expr;
use domain::plot::{CompiledFunction, FunctionKind, PlotRange, PlotSpec, SeriesStyle};

use super::axes::{View, nice_range, widen};

/// A point in data coordinates.
pub type Pt = (f64, f64);

/// Intervals a curve starts with before refining.
pub const INITIAL_INTERVALS: usize = 256;
/// Times an interval may be halved.
pub const MAX_DEPTH: u32 = 10;
/// Largest distance, as a fraction of the view, between a midpoint and
/// the chord it would replace (about a pixel on a typical plot).
pub const TOLERANCE: f64 = 0.0015;
/// A chord still this long (as a fraction of the view, about ten pixels)
/// after the last halving is a jump: the curve breaks there.
pub const JUMP: f64 = 0.02;
/// Most points one curve may have.
pub const MAX_POINTS: usize = 50_000;
/// Cells across the view for implicit curves.
pub const GRID_X: usize = 160;
/// Cells up the view for implicit curves.
pub const GRID_Y: usize = 120;
/// Bisection steps that place an implicit crossing on a cell edge.
pub const BISECTIONS: u32 = 10;
/// Samples per function used to fit the view.
pub const FIT_SAMPLES: usize = 400;
/// The x range when neither the spec nor the data give one.
pub const DEFAULT_X: (f64, f64) = (-10.0, 10.0);

/// Distance from `a` to `b` in fractions of `view`.
fn view_distance(view: &View, a: Pt, b: Pt) -> f64 {
    let (ax, ay) = view.normalize(a);
    let (bx, by) = view.normalize(b);
    (ax - bx).hypot(ay - by)
}

fn finite(p: Pt) -> bool {
    p.0.is_finite() && p.1.is_finite()
}

/// Whether `a` and `b` are both beyond the same edge of the view, so
/// nothing between them can show.
fn same_side_outside(view: &View, a: Pt, b: Pt) -> bool {
    let (ax, ay) = view.normalize(a);
    let (bx, by) = view.normalize(b);
    (ax < 0.0 && bx < 0.0)
        || (ax > 1.0 && bx > 1.0)
        || (ay < 0.0 && by < 0.0)
        || (ay > 1.0 && by > 1.0)
}

struct Sampler<'a, F> {
    eval: F,
    view: &'a View,
    lines: Vec<Vec<Pt>>,
    current: Vec<Pt>,
    points: usize,
}

impl<F: Fn(f64) -> Pt> Sampler<'_, F> {
    fn end_line(&mut self) {
        let line = std::mem::take(&mut self.current);
        if line.len() >= 2 {
            self.lines.push(line);
        }
    }

    fn push(&mut self, p: Pt) {
        if finite(p) {
            self.current.push(p);
            self.points += 1;
        } else {
            self.end_line();
        }
    }

    /// Adds the curve over `(s0, s1]`; `p0` was already added.
    fn refine(&mut self, s0: f64, p0: Pt, s1: f64, p1: Pt, depth: u32) {
        let room = depth < MAX_DEPTH && self.points < MAX_POINTS;
        let (f0, f1) = (finite(p0), finite(p1));
        if room && (f0 || f1) {
            let sm = 0.5 * (s0 + s1);
            let pm = (self.eval)(sm);
            let refine = if f0 && f1 {
                if same_side_outside(self.view, p0, p1) {
                    false
                } else if !finite(pm) {
                    true
                } else {
                    let chord_mid = (0.5 * (p0.0 + p1.0), 0.5 * (p0.1 + p1.1));
                    view_distance(self.view, pm, chord_mid) > TOLERANCE
                }
            } else {
                // Pin down where the curve stops or starts being defined.
                true
            };
            if refine {
                self.refine(s0, p0, sm, pm, depth + 1);
                self.refine(sm, pm, s1, p1, depth + 1);
                return;
            }
        }
        // A chord that refining could not shorten crosses a jump.
        let unresolved = !room || depth >= MAX_DEPTH;
        if f0
            && f1
            && unresolved
            && !same_side_outside(self.view, p0, p1)
            && view_distance(self.view, p0, p1) > JUMP
        {
            self.end_line();
        }
        self.push(p1);
    }
}

/// The curve `s ↦ eval(s)` over `[s0, s1]` as polylines, finer where it
/// bends (relative to `view`) and broken where it is undefined or jumps.
pub fn curve(eval: impl Fn(f64) -> Pt, (s0, s1): (f64, f64), view: &View) -> Vec<Vec<Pt>> {
    let mut sampler = Sampler {
        eval,
        view,
        lines: Vec::new(),
        current: Vec::new(),
        points: 0,
    };
    let step = (s1 - s0) / INITIAL_INTERVALS as f64;
    let mut prev_s = s0;
    let mut prev = (sampler.eval)(s0);
    sampler.push(prev);
    for i in 1..=INITIAL_INTERVALS {
        let s = if i == INITIAL_INTERVALS {
            s1
        } else {
            s0 + step * i as f64
        };
        let p = (sampler.eval)(s);
        sampler.refine(prev_s, prev, s, p, 0);
        (prev_s, prev) = (s, p);
    }
    sampler.end_line();
    sampler.lines
}

/// `f(x, y) = 0` over `view` as line segments, by marching squares on a
/// `nx × ny` grid. Saddle cells are split by the value at their center,
/// and a sign change across a pole (where bisection finds the value
/// growing instead of vanishing) is not a crossing.
pub fn contour(f: impl Fn(f64, f64) -> f64, view: &View, nx: usize, ny: usize) -> Vec<[Pt; 2]> {
    let (nx, ny) = (nx.max(1), ny.max(1));
    let dx = (view.x.1 - view.x.0) / nx as f64;
    let dy = (view.y.1 - view.y.0) / ny as f64;
    let at = |i: usize, j: usize| (view.x.0 + dx * i as f64, view.y.0 + dy * j as f64);
    let mut grid = vec![0.0; (nx + 1) * (ny + 1)];
    for j in 0..=ny {
        for i in 0..=nx {
            let (x, y) = at(i, j);
            grid[j * (nx + 1) + i] = f(x, y);
        }
    }
    let value = |i: usize, j: usize| grid[j * (nx + 1) + i];
    // The zero on the edge from `a` (value `va`) to `b`, or `None` for a
    // pole.
    let crossing = |a: Pt, va: f64, b: Pt, vb: f64| -> Option<Pt> {
        let bound = 0.1 * va.abs().max(vb.abs());
        let (mut a, mut va, mut b) = (a, va, b);
        for _ in 0..BISECTIONS {
            let m = (0.5 * (a.0 + b.0), 0.5 * (a.1 + b.1));
            let vm = f(m.0, m.1);
            if !vm.is_finite() {
                return None;
            }
            if (vm > 0.0) == (va > 0.0) {
                (a, va) = (m, vm);
            } else {
                b = m;
            }
        }
        let m = (0.5 * (a.0 + b.0), 0.5 * (a.1 + b.1));
        let vm = f(m.0, m.1);
        (vm.abs() <= bound).then_some(m)
    };
    let mut segments = Vec::new();
    for j in 0..ny {
        for i in 0..nx {
            // Corners counterclockwise from the bottom left.
            let corners = [(i, j), (i + 1, j), (i + 1, j + 1), (i, j + 1)];
            let values = corners.map(|(i, j)| value(i, j));
            if values.iter().any(|v| !v.is_finite()) {
                continue;
            }
            let positive = values.map(|v| v > 0.0);
            if positive.iter().all(|p| *p) || positive.iter().all(|p| !*p) {
                continue;
            }
            let points = corners.map(|(i, j)| at(i, j));
            // Edge k joins corner k and k + 1.
            let edge = |k: usize| -> Option<Pt> {
                let l = (k + 1) % 4;
                (positive[k] != positive[l])
                    .then(|| crossing(points[k], values[k], points[l], values[l]))
                    .flatten()
            };
            let crossed: Vec<usize> = (0..4)
                .filter(|&k| positive[k] != positive[(k + 1) % 4])
                .collect();
            let pairs: Vec<(usize, usize)> = match crossed.as_slice() {
                [a, b] => vec![(*a, *b)],
                [_, _, _, _] => {
                    let center = f(points[0].0 + 0.5 * dx, points[0].1 + 0.5 * dy);
                    // The center joins corner 0's region to corner 2's when
                    // it has their sign: the curve then cuts off corners 1
                    // and 3.
                    if (center > 0.0) == positive[0] {
                        vec![(0, 1), (2, 3)]
                    } else {
                        vec![(3, 0), (1, 2)]
                    }
                }
                _ => Vec::new(),
            };
            for (a, b) in pairs {
                if let (Some(p), Some(q)) = (edge(a), edge(b)) {
                    segments.push([p, q]);
                }
            }
        }
    }
    segments
}

/// `points` clipped to `bounds`, as the pieces of it inside (Liang-Barsky
/// on each segment).
pub fn clip_polyline(points: &[Pt], bounds: &View) -> Vec<Vec<Pt>> {
    let mut out: Vec<Vec<Pt>> = Vec::new();
    let mut current: Vec<Pt> = Vec::new();
    for pair in points.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        match clip_segment(a, b, bounds) {
            Some((p, q)) => {
                if current.last() != Some(&p) {
                    if current.len() >= 2 {
                        out.push(std::mem::take(&mut current));
                    }
                    current.clear();
                    current.push(p);
                }
                current.push(q);
            }
            None => {
                if current.len() >= 2 {
                    out.push(std::mem::take(&mut current));
                }
                current.clear();
            }
        }
    }
    if current.len() >= 2 {
        out.push(current);
    }
    out
}

/// The part of segment `a`–`b` inside `bounds`, or `None`.
pub fn clip_segment(a: Pt, b: Pt, bounds: &View) -> Option<(Pt, Pt)> {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    for (p, q) in [
        (-dx, a.0 - bounds.x.0),
        (dx, bounds.x.1 - a.0),
        (-dy, a.1 - bounds.y.0),
        (dy, bounds.y.1 - a.1),
    ] {
        if p == 0.0 {
            if q < 0.0 {
                return None;
            }
        } else {
            let r = q / p;
            if p < 0.0 {
                t0 = t0.max(r);
            } else {
                t1 = t1.min(r);
            }
        }
    }
    if t0 > t1 {
        return None;
    }
    let point = |t: f64| (a.0 + t * dx, a.1 + t * dy);
    // Keep the original end points exact, so consecutive pieces join.
    let p = if t0 == 0.0 { a } else { point(t0) };
    let q = if t1 == 1.0 { b } else { point(t1) };
    Some((p, q))
}

/// The y range that shows `values` without letting a few huge ones (near
/// an asymptote) flatten the rest: the full range when it is not much
/// wider than the 2nd–98th percentile range, else that range plus a
/// margin.
pub fn robust_range(values: &mut Vec<f64>) -> Option<(f64, f64)> {
    values.retain(|v| v.is_finite());
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let at = |p: f64| values[((values.len() - 1) as f64 * p).round() as usize];
    let (lo, hi) = (values[0], values[values.len() - 1]);
    let (p_lo, p_hi) = (at(0.02), at(0.98));
    let core = p_hi - p_lo;
    if hi - lo <= 3.0 * core || (core <= 0.0 && hi - lo <= 1e-9) {
        return Some((lo, hi));
    }
    let core = if core > 0.0 { core } else { (hi - lo).min(1.0) };
    Some(((p_lo - 0.25 * core).max(lo), (p_hi + 0.25 * core).min(hi)))
}

/// `(lo, hi)` padded by `share` of its span on each side.
fn padded((lo, hi): (f64, f64), share: f64) -> (f64, f64) {
    let (lo, hi) = widen(lo, hi);
    let pad = (hi - lo) * share;
    (lo - pad, hi + pad)
}

fn range_of(range: PlotRange) -> (f64, f64) {
    (range.min, range.max)
}

/// What a layer draws.
#[derive(Debug, Clone, PartialEq)]
pub enum Mark {
    /// Polylines, already clipped near the view.
    Lines(Vec<Vec<Pt>>),
    /// Separate segments (an implicit curve).
    Segments(Vec<[Pt; 2]>),
    /// Points.
    Points(Vec<Pt>),
}

/// One legend entry and what it draws, in palette color `color`.
#[derive(Debug, Clone, PartialEq)]
pub struct Layer {
    pub label: String,
    pub color: usize,
    pub mark: Mark,
}

/// Everything a plot draws for one set of parameter values.
#[derive(Debug, Clone, PartialEq)]
pub struct Scene {
    pub view: View,
    pub layers: Vec<Layer>,
    /// Explicit functions by layer index, for the hover readout.
    pub explicit: Vec<(usize, CompiledFunction)>,
    /// The parameter values the scene was drawn with.
    pub values: Vec<f64>,
}

impl Scene {
    /// `f(x)` of an explicit function with the scene's parameters.
    pub fn eval_explicit(&self, f: &CompiledFunction, x: f64) -> f64 {
        let mut vars = Vec::with_capacity(self.values.len() + 1);
        vars.push(x);
        vars.extend(&self.values);
        f.expr.eval(&vars)
    }
}

/// Evaluates `f` with `vars` followed by `params`, reusing `buf`.
fn eval_with(f: &Expr, buf: &mut Vec<f64>, vars: &[f64], params: &[f64]) -> f64 {
    buf.clear();
    buf.extend_from_slice(vars);
    buf.extend_from_slice(params);
    f.eval(buf)
}

/// The scene of `spec` with its functions `compiled` and parameter values
/// `values` (in the order of `spec.params`).
pub fn scene(spec: &PlotSpec, compiled: &[CompiledFunction], values: &[f64]) -> Scene {
    let param = |f: &Expr, vars: &[f64]| {
        let mut buf = Vec::with_capacity(vars.len() + values.len());
        eval_with(f, &mut buf, vars, values)
    };
    let parametric_at = |f: &CompiledFunction, t: f64| -> Pt {
        let y = f.y_expr.as_ref().map_or(f64::NAN, |y| param(y, &[t]));
        (param(&f.expr, &[t]), y)
    };
    // Parametric samples fit both axes.
    let mut px = Vec::new();
    let mut py = Vec::new();
    for f in compiled
        .iter()
        .filter(|f| f.kind == FunctionKind::Parametric)
    {
        let (t0, t1) = range_of(f.t_range);
        for i in 0..=FIT_SAMPLES {
            let (x, y) = parametric_at(f, t0 + (t1 - t0) * i as f64 / FIT_SAMPLES as f64);
            px.push(x);
            py.push(y);
        }
    }
    let series_x = spec.series.iter().flat_map(|s| s.x.iter().copied());
    let series_y = spec.series.iter().flat_map(|s| s.y.iter().copied());
    let x = match spec.x_range {
        Some(r) => range_of(r),
        None => {
            let mut xs: Vec<f64> = series_x.chain(px).collect();
            match robust_range(&mut xs).map(|r| padded(r, 0.04)) {
                Some((lo, hi)) => nice_range(lo, hi, 6),
                None => DEFAULT_X,
            }
        }
    };
    let y = match spec.y_range {
        Some(r) => range_of(r),
        None => {
            let mut ys = py;
            for f in compiled.iter().filter(|f| f.kind == FunctionKind::Explicit) {
                for i in 0..=FIT_SAMPLES {
                    ys.push(param(
                        &f.expr,
                        &[x.0 + (x.1 - x.0) * i as f64 / FIT_SAMPLES as f64],
                    ));
                }
            }
            let curves = robust_range(&mut ys);
            let mut data: Vec<f64> = series_y.collect();
            data.retain(|v| v.is_finite());
            let data = data.iter().fold(None, |acc: Option<(f64, f64)>, &v| {
                Some(acc.map_or((v, v), |(lo, hi)| (lo.min(v), hi.max(v))))
            });
            match (curves, data) {
                (Some(a), Some(b)) => {
                    let r = padded((a.0.min(b.0), a.1.max(b.1)), 0.05);
                    nice_range(r.0, r.1, 5)
                }
                (Some(r), None) | (None, Some(r)) => {
                    let r = padded(r, 0.05);
                    nice_range(r.0, r.1, 5)
                }
                // Only implicit curves: the same span as x, around 0.
                (None, None) => {
                    let half = 0.5 * (x.1 - x.0);
                    (-half, half)
                }
            }
        }
    };
    let view = View {
        x: widen(x.0, x.1),
        y: widen(y.0, y.1),
    };
    // Paths reach a little past the view so line caps stay outside it.
    let margin = View {
        x: padded(view.x, 0.02),
        y: padded(view.y, 0.02),
    };
    let clip = |lines: Vec<Vec<Pt>>| -> Vec<Vec<Pt>> {
        lines
            .iter()
            .flat_map(|l| clip_polyline(l, &margin))
            .collect()
    };
    let mut layers = Vec::new();
    let mut explicit = Vec::new();
    for f in compiled {
        let mark = match f.kind {
            FunctionKind::Explicit => {
                explicit.push((layers.len(), f.clone()));
                Mark::Lines(clip(curve(|x| (x, param(&f.expr, &[x])), view.x, &view)))
            }
            FunctionKind::Parametric => Mark::Lines(clip(curve(
                |t| parametric_at(f, t),
                range_of(f.t_range),
                &view,
            ))),
            FunctionKind::Implicit => {
                // `contour` takes `Fn`, so the reused buffer lives in a cell.
                let buf = RefCell::new(Vec::with_capacity(2 + values.len()));
                let g = |x: f64, y: f64| eval_with(&f.expr, &mut buf.borrow_mut(), &[x, y], values);
                Mark::Segments(contour(g, &view, GRID_X, GRID_Y))
            }
        };
        layers.push(Layer {
            label: f.label.clone(),
            color: layers.len(),
            mark,
        });
    }
    for s in &spec.series {
        let points: Vec<Pt> = s.x.iter().copied().zip(s.y.iter().copied()).collect();
        let mark = match s.style {
            SeriesStyle::Scatter => Mark::Points(points),
            SeriesStyle::Line => Mark::Lines(clip(vec![points])),
        };
        layers.push(Layer {
            label: s.name.clone(),
            color: layers.len(),
            mark,
        });
    }
    Scene {
        view,
        layers,
        explicit,
        values: values.to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn view() -> View {
        View {
            x: (-5.0, 5.0),
            y: (-5.0, 5.0),
        }
    }

    #[test]
    fn smooth_curves_are_one_line_refined_where_they_bend() {
        let lines = curve(|x| (x, x.sin()), (-5.0, 5.0), &view());
        assert_eq!(lines.len(), 1);
        assert!(lines[0].len() > INITIAL_INTERVALS);
        let straight = curve(|x| (x, 2.0 * x), (-5.0, 5.0), &view());
        // A straight line needs no extra points.
        assert_eq!(straight[0].len(), INITIAL_INTERVALS + 1);
    }

    #[test]
    fn curves_break_at_poles_and_where_undefined() {
        // tan has poles at ±π/2 and ±3π/2 inside [-5, 5].
        let tan = curve(|x| (x, x.tan()), (-5.0, 5.0), &view());
        assert_eq!(
            tan.len(),
            5,
            "{:?}",
            tan.iter().map(Vec::len).collect::<Vec<_>>()
        );
        for line in &tan {
            for pair in line.windows(2) {
                assert!(
                    !(pair[0].1 > 5.0 && pair[1].1 < -5.0),
                    "a line across the pole: {pair:?}"
                );
            }
        }
        // sqrt is undefined left of 0; its line starts near 0.
        let sqrt = curve(|x| (x, x.sqrt()), (-5.0, 5.0), &view());
        assert_eq!(sqrt.len(), 1);
        assert!(sqrt[0][0].0 >= 0.0 && sqrt[0][0].0 < 0.01);
        // 1/x jumps from -∞ to +∞ at 0.
        assert_eq!(curve(|x| (x, 1.0 / x), (-5.0, 5.0), &view()).len(), 2);
        // A step jumps once.
        assert_eq!(curve(|x| (x, x.floor()), (-0.5, 0.5), &view()).len(), 2);
    }

    #[test]
    fn marching_squares_traces_a_circle() {
        let segments = contour(|x, y| x * x + y * y - 4.0, &view(), 40, 40);
        assert!(segments.len() > 40);
        for [p, q] in &segments {
            for (x, y) in [p, q] {
                assert!((x.hypot(*y) - 2.0).abs() < 1e-3, "({x}, {y})");
            }
        }
        // No zero, no segments.
        assert!(contour(|x, y| x * x + y * y + 1.0, &view(), 20, 20).is_empty());
    }

    #[test]
    fn marching_squares_skips_poles() {
        // y = tan(x) as f(x, y) = tan(x) - y: crossing the poles at ±π/2 is
        // a sign change but not a zero.
        let segments = contour(|x, y| x.tan() - y, &view(), 80, 80);
        assert!(!segments.is_empty());
        for [p, q] in &segments {
            assert!((p.0.tan() - p.1).abs() < 0.05, "{p:?}");
            assert!((q.0.tan() - q.1).abs() < 0.05, "{q:?}");
        }
    }

    #[test]
    fn saddles_split_by_the_center() {
        // x·y = 0 is both axes; every cell touching them crosses.
        // Nine cells put the saddle of x·y in the middle of a cell.
        let segments = contour(|x, y| x * y - 0.01, &view(), 9, 9);
        assert!(!segments.is_empty());
        for [p, q] in &segments {
            // On the hyperbola, never across the gap between its branches.
            assert!(p.0 * p.1 > 0.0 && q.0 * q.1 > 0.0, "{p:?} {q:?}");
        }
    }

    #[test]
    fn clipping_keeps_the_inside_pieces() {
        let bounds = View {
            x: (0.0, 1.0),
            y: (0.0, 1.0),
        };
        let pieces = clip_polyline(
            &[(-1.0, 0.5), (0.5, 0.5), (0.5, 2.0), (0.7, 0.5), (0.9, 0.5)],
            &bounds,
        );
        assert_eq!(pieces.len(), 2);
        assert_eq!(pieces[0], [(0.0, 0.5), (0.5, 0.5), (0.5, 1.0)]);
        assert_eq!(pieces[1].last(), Some(&(0.9, 0.5)));
        assert_eq!(clip_segment((2.0, 2.0), (3.0, 3.0), &bounds), None);
    }

    #[test]
    fn robust_ranges_ignore_spikes() {
        let mut smooth: Vec<f64> = (0..100).map(f64::from).collect();
        assert_eq!(robust_range(&mut smooth), Some((0.0, 99.0)));
        let mut spiky: Vec<f64> = (0..100).map(|i| f64::from(i % 10)).collect();
        spiky.push(1e9);
        spiky.push(-1e9);
        spiky.push(f64::NAN);
        let (lo, hi) = robust_range(&mut spiky).unwrap();
        assert!(lo > -10.0 && hi < 20.0, "{lo} {hi}");
        assert_eq!(robust_range(&mut vec![f64::NAN]), None);
        assert_eq!(robust_range(&mut vec![3.0, 3.0]), Some((3.0, 3.0)));
    }

    fn spec(value: serde_json::Value) -> PlotSpec {
        serde_json::from_value(value).unwrap_or_else(|e| panic!("{e}"))
    }

    #[test]
    fn scenes_fit_the_view_and_use_the_parameters() {
        let spec = spec(json!({
            "x_range": {"min": -6, "max": 6},
            "functions": [
                {"kind": "explicit", "expr": "a tan(x)"},
                {"kind": "parametric", "expr": "r cos(t)", "y_expr": "r sin(t)",
                 "t_range": {"min": 0, "max": 6.3}},
                {"kind": "implicit", "expr": "x^2 + y^2 - r^2", "label": "circle"}
            ],
            "params": [
                {"name": "a", "min": 0, "max": 2, "value": 1},
                {"name": "r", "min": 0.5, "max": 3, "value": 2}
            ],
            "series": [{"name": "data", "x": [0, 1], "y": [0.5, 1.5]}]
        }));
        let compiled = spec.compile().unwrap();
        let scene = scene(&spec, &compiled, &spec.param_values());
        assert_eq!(scene.view.x, (-6.0, 6.0));
        // tan's spikes do not set the y range.
        assert!(
            scene.view.y.1 < 100.0 && scene.view.y.0 > -100.0,
            "{:?}",
            scene.view
        );
        assert_eq!(scene.layers.len(), 4);
        assert_eq!(scene.layers[2].label, "circle");
        let Mark::Segments(circle) = &scene.layers[2].mark else {
            panic!("implicit as segments");
        };
        assert!(
            circle
                .iter()
                .all(|[p, _]| (p.0.hypot(p.1) - 2.0).abs() < 0.05)
        );
        let Mark::Lines(tan) = &scene.layers[0].mark else {
            panic!("explicit as lines");
        };
        assert!(tan.len() >= 4);
        assert!(matches!(scene.layers[3].mark, Mark::Points(ref p) if p.len() == 2));
        assert_eq!(scene.explicit.len(), 1);
        let f = &scene.explicit[0].1;
        assert!((scene.eval_explicit(f, 1.0) - 1f64.tan()).abs() < 1e-12);
        let doubled = super::scene(&spec, &compiled, &[2.0, 2.0]);
        assert!((doubled.eval_explicit(f, 1.0) - 2.0 * 1f64.tan()).abs() < 1e-12);
    }

    #[test]
    fn views_without_ranges_fit_the_data_or_default() {
        let data = spec(json!({"series": [{"name": "s", "x": [1, 2, 3], "y": [10, 20, 30]}]}));
        let s = scene(&data, &[], &[]);
        assert!(s.view.x.0 <= 1.0 && s.view.x.1 >= 3.0 && s.view.x.1 < 10.0);
        assert!(s.view.y.0 <= 10.0 && s.view.y.1 >= 30.0);
        let implicit = spec(json!({"functions": [{"kind": "implicit", "expr": "x - y"}]}));
        let s = scene(&implicit, &implicit.compile().unwrap(), &[]);
        assert_eq!(s.view.x, DEFAULT_X);
        assert_eq!(s.view.y, (-10.0, 10.0));
    }
}
