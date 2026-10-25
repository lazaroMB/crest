//! The force layout: a seeded Fruchterman–Reingold spring embedding.
//!
//! Nodes repel each other, edges pull their ends together and a weak
//! gravity keeps separate components in view. The start positions come
//! from a small seeded generator and the arithmetic runs in a fixed
//! order, so the same spec and seed always give the same picture. A last
//! pass pushes overlapping boxes apart.

/// A deterministic generator (SplitMix64).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// A number in `[0, 1)`.
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1_u64 << 53) as f64
    }
}

/// Iterations for `n` nodes: fewer for big graphs, whose repulsion costs
/// `n²` per step.
fn iterations(n: usize) -> usize {
    match n {
        0..=200 => 300,
        201..=800 => 120,
        _ => 50,
    }
}

/// Centres of nodes of `sizes` (width, height) joined by `edges`, from
/// `seed`.
#[must_use]
pub fn force(sizes: &[(f64, f64)], edges: &[(usize, usize)], seed: u64) -> Vec<(f64, f64)> {
    let n = sizes.len();
    if n == 0 {
        return Vec::new();
    }
    let mean_size = sizes.iter().map(|s| s.0.max(s.1)).sum::<f64>() / n as f64;
    // The ideal edge length.
    let k = mean_size + 40.0;
    let side = k * (n as f64).sqrt();
    let mut rng = Rng(seed);
    let mut pos: Vec<(f64, f64)> = (0..n)
        .map(|_| ((rng.unit() - 0.5) * side, (rng.unit() - 0.5) * side))
        .collect();
    let links: Vec<(usize, usize)> = edges
        .iter()
        .copied()
        .filter(|&(a, b)| a != b && a < n && b < n)
        .collect();
    let steps = iterations(n);
    let start = side / 10.0 + k;
    let mut disp = vec![(0.0_f64, 0.0_f64); n];
    for step in 0..steps {
        let temperature = start * (1.0 - step as f64 / steps as f64) + 0.5;
        disp.iter_mut().for_each(|d| *d = (0.0, 0.0));
        for a in 0..n {
            for b in a + 1..n {
                let (mut dx, mut dy) = (pos[a].0 - pos[b].0, pos[a].1 - pos[b].1);
                let mut dist = (dx * dx + dy * dy).sqrt();
                if dist < 1e-3 {
                    // Coincident nodes: split them along a fixed direction.
                    let angle = (a * 31 + b * 17) as f64;
                    (dx, dy) = (angle.cos(), angle.sin());
                    dist = 1.0;
                }
                let push = k * k / dist;
                let (fx, fy) = (dx / dist * push, dy / dist * push);
                disp[a].0 += fx;
                disp[a].1 += fy;
                disp[b].0 -= fx;
                disp[b].1 -= fy;
            }
        }
        for &(a, b) in &links {
            let (dx, dy) = (pos[a].0 - pos[b].0, pos[a].1 - pos[b].1);
            let dist = (dx * dx + dy * dy).sqrt().max(1e-3);
            let pull = dist * dist / k;
            let (fx, fy) = (dx / dist * pull, dy / dist * pull);
            disp[a].0 -= fx;
            disp[a].1 -= fy;
            disp[b].0 += fx;
            disp[b].1 += fy;
        }
        for (p, d) in pos.iter_mut().zip(&mut disp) {
            d.0 -= p.0 * 0.05;
            d.1 -= p.1 * 0.05;
            let len = (d.0 * d.0 + d.1 * d.1).sqrt();
            if len > 1e-9 {
                let step = len.min(temperature);
                p.0 += d.0 / len * step;
                p.1 += d.1 / len * step;
            }
        }
    }
    separate(&mut pos, sizes);
    pos
}

/// Pushes overlapping boxes apart along the axis they overlap least on.
fn separate(pos: &mut [(f64, f64)], sizes: &[(f64, f64)]) {
    const PAD: f64 = 8.0;
    for _ in 0..12 {
        let mut moved = false;
        for a in 0..pos.len() {
            for b in a + 1..pos.len() {
                let (dx, dy) = (pos[b].0 - pos[a].0, pos[b].1 - pos[a].1);
                let ox = (sizes[a].0 + sizes[b].0) / 2.0 + PAD - dx.abs();
                let oy = (sizes[a].1 + sizes[b].1) / 2.0 + PAD - dy.abs();
                if ox <= 0.0 || oy <= 0.0 {
                    continue;
                }
                moved = true;
                if ox < oy {
                    let shift = ox / 2.0 * if dx < 0.0 { -1.0 } else { 1.0 };
                    pos[a].0 -= shift;
                    pos[b].0 += shift;
                } else {
                    let shift = oy / 2.0 * if dy < 0.0 { -1.0 } else { 1.0 };
                    pos[a].1 -= shift;
                    pos[b].1 += shift;
                }
            }
        }
        if !moved {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ring(n: usize) -> Vec<(usize, usize)> {
        (0..n).map(|i| (i, (i + 1) % n)).collect()
    }

    #[test]
    fn the_same_seed_gives_the_same_layout() {
        let sizes = vec![(40.0, 20.0); 12];
        let edges = ring(12);
        let a = force(&sizes, &edges, 7);
        assert_eq!(a, force(&sizes, &edges, 7));
        assert_ne!(a, force(&sizes, &edges, 8));
        assert!(a.iter().all(|p| p.0.is_finite() && p.1.is_finite()));
    }

    #[test]
    fn boxes_do_not_overlap_and_neighbours_sit_close() {
        let sizes = vec![(40.0, 20.0); 10];
        // Two triangles joined by nothing, plus four loose nodes.
        let edges = [(0, 1), (1, 2), (2, 0), (3, 4), (4, 5), (5, 3)];
        let pos = force(&sizes, &edges, 1);
        for a in 0..10 {
            for b in a + 1..10 {
                let (dx, dy) = ((pos[a].0 - pos[b].0).abs(), (pos[a].1 - pos[b].1).abs());
                assert!(dx >= 40.0 || dy >= 20.0, "{a} and {b} overlap");
            }
        }
        let dist = |a: usize, b: usize| {
            ((pos[a].0 - pos[b].0).powi(2) + (pos[a].1 - pos[b].1).powi(2)).sqrt()
        };
        let linked = (dist(0, 1) + dist(1, 2) + dist(3, 4)) / 3.0;
        let across = (dist(0, 3) + dist(1, 4) + dist(2, 5)) / 3.0;
        assert!(linked < across, "{linked} vs {across}");
    }

    #[test]
    fn tiny_graphs_work() {
        assert!(force(&[], &[], 1).is_empty());
        assert_eq!(force(&[(10.0, 10.0)], &[(0, 0)], 1).len(), 1);
    }
}
