//! The layered (Sugiyama) layout: edges flow down through layers.
//!
//! The four classic steps, each a function of its own so tests can check
//! it:
//!
//! 1. [`remove_cycles`]: a depth-first search reverses the edges that
//!    close a cycle, which leaves a DAG.
//! 2. [`assign_layers`]: longest-path layering (a node sits one layer
//!    below its lowest predecessor), with sources pulled down next to
//!    their first successor so they do not stretch edges from the top.
//! 3. Edges spanning several layers are split by dummy vertices, one per
//!    layer crossed, and [`reduce_crossings`] sorts every layer by the
//!    barycentre of its neighbours, sweeping down and up, keeping the
//!    order with the fewest [`crossings`].
//! 4. Coordinates: each vertex wants the mean x of its neighbours; a
//!    pool-adjacent-violators pass puts it as near as the spacing of its
//!    layer allows, a few sweeps over. Edges become polylines through
//!    their dummies.
//!
//! Everything here is in the direction "down"; the caller transposes for
//! "right".

/// Space between the boxes of one layer.
pub const NODE_GAP: f64 = 28.0;
/// Space next to a dummy vertex (an edge passing a layer).
pub const DUMMY_GAP: f64 = 12.0;
/// Space between layers.
pub const LAYER_GAP: f64 = 48.0;
/// Extra space between layers when edges carry labels.
pub const LABEL_GAP: f64 = 18.0;
/// Barycentre sweeps (each one down then up) at most.
const SWEEPS: usize = 24;
/// Sweeps without fewer crossings before giving up.
const PATIENCE: usize = 4;
/// Coordinate sweeps.
const PLACE_SWEEPS: usize = 8;

/// For each edge, whether the layout reverses it to break a cycle.
/// Self-loops are never reversed (they take no part in layering).
#[must_use]
pub fn remove_cycles(n: usize, edges: &[(usize, usize)]) -> Vec<bool> {
    let mut out: Vec<Vec<(usize, usize)>> = vec![Vec::new(); n];
    for (ix, &(a, b)) in edges.iter().enumerate() {
        if a != b && a < n && b < n {
            out[a].push((ix, b));
        }
    }
    // 0 unvisited, 1 on the stack, 2 done.
    let mut state = vec![0_u8; n];
    let mut reversed = vec![false; edges.len()];
    for root in 0..n {
        if state[root] != 0 {
            continue;
        }
        state[root] = 1;
        let mut stack = vec![(root, 0_usize)];
        while let Some(&mut (node, ref mut next)) = stack.last_mut() {
            if let Some(&(edge, to)) = out[node].get(*next) {
                *next += 1;
                match state[to] {
                    0 => {
                        state[to] = 1;
                        stack.push((to, 0));
                    }
                    1 => reversed[edge] = true,
                    _ => {}
                }
            } else {
                state[node] = 2;
                stack.pop();
            }
        }
    }
    reversed
}

/// Layer of each node for the acyclic `edges` (self-loops ignored):
/// longest path from the sources, then each source moved down to just
/// above its nearest successor.
#[must_use]
pub fn assign_layers(n: usize, edges: &[(usize, usize)]) -> Vec<usize> {
    let mut succ: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut indegree = vec![0_usize; n];
    for &(a, b) in edges {
        if a != b && a < n && b < n {
            succ[a].push(b);
            indegree[b] += 1;
        }
    }
    let sources: Vec<bool> = indegree.iter().map(|&d| d == 0).collect();
    // Kahn's order; a node left over (a cycle, which callers remove
    // first) keeps layer 0.
    let mut layer = vec![0_usize; n];
    let mut queue: std::collections::VecDeque<usize> = (0..n).filter(|&v| sources[v]).collect();
    let mut order = Vec::with_capacity(n);
    let mut left = indegree;
    while let Some(v) = queue.pop_front() {
        order.push(v);
        for &w in &succ[v] {
            layer[w] = layer[w].max(layer[v] + 1);
            left[w] -= 1;
            if left[w] == 0 {
                queue.push_back(w);
            }
        }
    }
    for &v in order.iter().rev() {
        if sources[v]
            && let Some(nearest) = succ[v].iter().map(|&w| layer[w]).min()
        {
            layer[v] = nearest.saturating_sub(1);
        }
    }
    layer
}

/// Crossings between consecutive layers: `order[l]` lists the vertices
/// of layer `l` left to right, `segments` joins a vertex to one in the
/// next layer. Counted per layer pair as inversions with a Fenwick tree.
#[must_use]
pub fn crossings(order: &[Vec<usize>], segments: &[(usize, usize)], layer_of: &[usize]) -> usize {
    let mut pos = vec![0_usize; layer_of.len()];
    for layer in order {
        for (ix, &v) in layer.iter().enumerate() {
            pos[v] = ix;
        }
    }
    let mut by_layer: Vec<Vec<(usize, usize)>> = vec![Vec::new(); order.len()];
    for &(a, b) in segments {
        by_layer[layer_of[a]].push((pos[a], pos[b]));
    }
    let mut total = 0;
    for (l, pairs) in by_layer.iter_mut().enumerate() {
        if pairs.len() < 2 {
            continue;
        }
        pairs.sort_unstable();
        let width = order.get(l + 1).map_or(0, Vec::len);
        let mut tree = vec![0_usize; width + 1];
        for (seen, &(_, lower)) in pairs.iter().enumerate() {
            // Segments seen so far ending right of `lower` cross this one.
            let mut at_most = 0;
            let mut i = lower + 1;
            while i > 0 {
                at_most += tree[i];
                i &= i - 1;
            }
            total += seen - at_most;
            let mut i = lower + 1;
            while i <= width {
                tree[i] += 1;
                i += i.isolate_lowest_one();
            }
        }
    }
    total
}

/// Sorts each layer of `order` by the barycentre of its neighbours,
/// sweeping down (by the layer above) and up (by the layer below), and
/// keeps the order with the fewest crossings. Returns the crossings
/// before and after.
pub fn reduce_crossings(
    order: &mut [Vec<usize>],
    segments: &[(usize, usize)],
    layer_of: &[usize],
) -> (usize, usize) {
    let n = layer_of.len();
    let mut up: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut down: Vec<Vec<usize>> = vec![Vec::new(); n];
    for &(a, b) in segments {
        down[a].push(b);
        up[b].push(a);
    }
    let initial = crossings(order, segments, layer_of);
    let mut best = initial;
    let mut best_order = order.to_vec();
    let mut pos = vec![0.0_f64; n];
    let mut stale = 0;
    let set_pos = |layer: &[usize], pos: &mut [f64]| {
        for (ix, &v) in layer.iter().enumerate() {
            pos[v] = ix as f64;
        }
    };
    for layer in order.iter() {
        set_pos(layer, &mut pos);
    }
    for _ in 0..SWEEPS {
        if best == 0 {
            break;
        }
        for layer in order.iter_mut().skip(1) {
            sort_by_barycentre(layer, &up, &mut pos);
        }
        for layer in order.iter_mut().rev().skip(1) {
            sort_by_barycentre(layer, &down, &mut pos);
        }
        let now = crossings(order, segments, layer_of);
        if now < best {
            best = now;
            best_order = order.to_vec();
            stale = 0;
        } else {
            stale += 1;
            if stale >= PATIENCE {
                break;
            }
        }
    }
    order.clone_from_slice(&best_order);
    (initial, best)
}

/// Stable sort of `layer` by the mean position of each vertex's
/// `neighbours`; a vertex without any keeps its own position as key.
fn sort_by_barycentre(layer: &mut [usize], neighbours: &[Vec<usize>], pos: &mut [f64]) {
    let mut keyed: Vec<(f64, usize)> = layer
        .iter()
        .map(|&v| {
            let near = &neighbours[v];
            let key = if near.is_empty() {
                pos[v]
            } else {
                near.iter().map(|&w| pos[w]).sum::<f64>() / near.len() as f64
            };
            (key, v)
        })
        .collect();
    keyed.sort_by(|a, b| a.0.total_cmp(&b.0));
    for (ix, (_, v)) in keyed.into_iter().enumerate() {
        layer[ix] = v;
        pos[v] = ix as f64;
    }
}

/// The x nearest to `target` (in least squares) for vertices that must
/// keep their order with `gaps[i]` between vertex `i` and `i + 1`: pool
/// adjacent violators on the targets shifted by the cumulative gaps.
fn place_row(target: &[f64], gaps: &[f64]) -> Vec<f64> {
    let mut offset = Vec::with_capacity(target.len());
    let mut sum = 0.0;
    for ix in 0..target.len() {
        if ix > 0 {
            sum += gaps[ix - 1];
        }
        offset.push(sum);
    }
    // Blocks of (mean, count), merged while they would go backwards.
    let mut blocks: Vec<(f64, usize)> = Vec::new();
    for (t, o) in target.iter().zip(&offset) {
        let mut block = (t - o, 1_usize);
        while let Some(&(mean, count)) = blocks.last() {
            if mean <= block.0 {
                break;
            }
            blocks.pop();
            let total = count + block.1;
            block = (
                (mean * count as f64 + block.0 * block.1 as f64) / total as f64,
                total,
            );
        }
        blocks.push(block);
    }
    let mut out = Vec::with_capacity(target.len());
    for (mean, count) in blocks {
        for _ in 0..count {
            out.push(mean + offset[out.len()]);
        }
    }
    out
}

/// A layered drawing of the nodes, direction down.
#[derive(Debug, Clone, PartialEq)]
pub struct Layered {
    /// Centre of each node.
    pub centres: Vec<(f64, f64)>,
    /// Polyline of each edge, from its `from` centre to its `to` centre
    /// through its bends (empty for a self-loop).
    pub routes: Vec<Vec<(f64, f64)>>,
    /// Layer of each node.
    pub layers: Vec<usize>,
    /// Crossings of the first order and of the order kept.
    pub crossings: (usize, usize),
}

/// Lays out nodes of `sizes` (width, height) joined by `edges` (node
/// indices) in layers going down. `labelled` leaves room between layers
/// for edge labels.
#[must_use]
pub fn layered(sizes: &[(f64, f64)], edges: &[(usize, usize)], labelled: bool) -> Layered {
    let n = sizes.len();
    let reversed = remove_cycles(n, edges);
    let oriented: Vec<(usize, usize)> = edges
        .iter()
        .zip(&reversed)
        .map(|(&(a, b), &rev)| if rev { (b, a) } else { (a, b) })
        .collect();
    let layers = assign_layers(n, &oriented);
    let depth = layers.iter().max().map_or(0, |&l| l + 1);

    // Vertices: the nodes, then one dummy per layer an edge crosses.
    let mut layer_of = layers.clone();
    let mut widths: Vec<f64> = sizes.iter().map(|s| s.0).collect();
    let mut chains: Vec<Vec<usize>> = Vec::with_capacity(edges.len());
    let mut segments: Vec<(usize, usize)> = Vec::new();
    for &(a, b) in &oriented {
        if a == b || a >= n || b >= n {
            chains.push(Vec::new());
            continue;
        }
        let mut chain = vec![a];
        for l in layers[a] + 1..layers[b] {
            layer_of.push(l);
            widths.push(0.0);
            chain.push(layer_of.len() - 1);
        }
        chain.push(b);
        segments.extend(chain.windows(2).map(|w| (w[0], w[1])));
        chains.push(chain);
    }
    let mut order: Vec<Vec<usize>> = vec![Vec::new(); depth];
    for (v, &l) in layer_of.iter().enumerate() {
        order[l].push(v);
    }
    let crossings = reduce_crossings(&mut order, &segments, &layer_of);

    // x: packed, then pulled to the neighbours' mean, sweep by sweep.
    let vertices = layer_of.len();
    let mut up: Vec<Vec<usize>> = vec![Vec::new(); vertices];
    let mut down: Vec<Vec<usize>> = vec![Vec::new(); vertices];
    for &(a, b) in &segments {
        down[a].push(b);
        up[b].push(a);
    }
    let gaps: Vec<Vec<f64>> = order
        .iter()
        .map(|layer| {
            layer
                .windows(2)
                .map(|w| {
                    let gap = if w[0] >= n || w[1] >= n {
                        DUMMY_GAP
                    } else {
                        NODE_GAP
                    };
                    (widths[w[0]] + widths[w[1]]) / 2.0 + gap
                })
                .collect()
        })
        .collect();
    let mut x = vec![0.0_f64; vertices];
    for (layer, gaps) in order.iter().zip(&gaps) {
        let zeros = vec![0.0; layer.len()];
        for (&v, at) in layer.iter().zip(place_row(&zeros, gaps)) {
            x[v] = at;
        }
    }
    // Centre the layers on each other before the sweeps.
    let mid = |layer: &[usize], x: &[f64]| match (layer.first(), layer.last()) {
        (Some(&a), Some(&b)) => (x[a] + x[b]) / 2.0,
        _ => 0.0,
    };
    for layer in &order {
        let shift = mid(layer, &x);
        for &v in layer {
            x[v] -= shift;
        }
    }
    for sweep in 0..PLACE_SWEEPS {
        let layers_now: Vec<usize> = if sweep % 2 == 0 {
            (0..depth).collect()
        } else {
            (0..depth).rev().collect()
        };
        for l in layers_now {
            let layer = &order[l];
            let target: Vec<f64> = layer
                .iter()
                .map(|&v| {
                    let near: Vec<usize> = match sweep % 3 {
                        0 => up[v].clone(),
                        1 => down[v].clone(),
                        _ => up[v].iter().chain(&down[v]).copied().collect(),
                    };
                    if near.is_empty() {
                        x[v]
                    } else {
                        near.iter().map(|&w| x[w]).sum::<f64>() / near.len() as f64
                    }
                })
                .collect();
            for (&v, at) in layer.iter().zip(place_row(&target, &gaps[l])) {
                x[v] = at;
            }
        }
    }

    // y: layers stacked, each as tall as its tallest node.
    let gap = LAYER_GAP + if labelled { LABEL_GAP } else { 0.0 };
    let mut heights = vec![0.0_f64; depth];
    for (v, &(_, h)) in sizes.iter().enumerate() {
        heights[layers[v]] = heights[layers[v]].max(h);
    }
    let mut centre_y = Vec::with_capacity(depth);
    let mut top = 0.0;
    for h in &heights {
        centre_y.push(top + h / 2.0);
        top += h + gap;
    }
    let at = |v: usize| (x[v], centre_y[layer_of[v]]);
    let centres = (0..n).map(at).collect();
    let routes = chains
        .iter()
        .zip(&reversed)
        .map(|(chain, &rev)| {
            let mut points: Vec<(f64, f64)> = chain.iter().map(|&v| at(v)).collect();
            if rev {
                points.reverse();
            }
            points
        })
        .collect();
    Layered {
        centres,
        routes,
        layers,
        crossings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycles_are_broken_by_reversing_back_edges() {
        let edges = [(0, 1), (1, 2), (2, 0), (2, 2)];
        let reversed = remove_cycles(3, &edges);
        assert_eq!(reversed, [false, false, true, false]);
        // Two disjoint cycles: one edge of each.
        let edges = [(0, 1), (1, 0), (2, 3), (3, 2)];
        assert_eq!(remove_cycles(4, &edges).iter().filter(|r| **r).count(), 2);
    }

    #[test]
    fn longest_path_layering_pulls_sources_down() {
        // a → b → c → d, and e → d: e sits just above d.
        let edges = [(0, 1), (1, 2), (2, 3), (4, 3)];
        assert_eq!(assign_layers(5, &edges), [0, 1, 2, 3, 2]);
        // A diamond: d below both b and c.
        let edges = [(0, 1), (0, 2), (1, 3), (2, 3), (0, 3)];
        assert_eq!(assign_layers(4, &edges), [0, 1, 1, 2]);
        // An isolated node stays on top.
        assert_eq!(assign_layers(2, &[]), [0, 0]);
    }

    #[test]
    fn crossings_are_counted_per_layer_pair() {
        // Layer 0: 0 1; layer 1: 2 3. 0→3 and 1→2 cross once.
        let order = vec![vec![0, 1], vec![2, 3]];
        let layer_of = [0, 0, 1, 1];
        assert_eq!(crossings(&order, &[(0, 3), (1, 2)], &layer_of), 1);
        assert_eq!(crossings(&order, &[(0, 2), (1, 3)], &layer_of), 0);
        // K(3,3) in this order: 9 edges, 9 crossings.
        let order = vec![vec![0, 1, 2], vec![3, 4, 5]];
        let layer_of = [0, 0, 0, 1, 1, 1];
        let all: Vec<_> = (0..3).flat_map(|a| (3..6).map(move |b| (a, b))).collect();
        assert_eq!(crossings(&order, &all, &layer_of), 9);
    }

    #[test]
    fn barycentre_sweeps_lower_the_crossings() {
        // Two layers wired in reverse: 0→7, 1→6, ... 3→4 (6 crossings).
        let mut order = vec![vec![0, 1, 2, 3], vec![4, 5, 6, 7]];
        let layer_of = [0, 0, 0, 0, 1, 1, 1, 1];
        let segments = [(0, 7), (1, 6), (2, 5), (3, 4)];
        let (before, after) = reduce_crossings(&mut order, &segments, &layer_of);
        assert_eq!((before, after), (6, 0));
        assert_eq!(crossings(&order, &segments, &layer_of), 0);
    }

    #[test]
    fn a_layered_drawing_has_no_overlaps_and_edges_point_down() {
        // A small DAG with a long edge and a cycle.
        let sizes = vec![(60.0, 24.0); 6];
        let edges = [
            (0, 1),
            (0, 2),
            (1, 3),
            (2, 3),
            (3, 4),
            (0, 4),
            (4, 0),
            (5, 5),
        ];
        let drawing = layered(&sizes, &edges, false);
        assert_eq!(drawing.layers, [0, 1, 1, 2, 3, 0]);
        // Nodes of one layer do not overlap.
        for a in 0..6 {
            for b in a + 1..6 {
                if drawing.layers[a] == drawing.layers[b] {
                    let dx = (drawing.centres[a].0 - drawing.centres[b].0).abs();
                    assert!(dx >= 60.0 + NODE_GAP - 1e-6, "{a} and {b} overlap");
                }
            }
        }
        // The long edge 0 → 4 bends through two dummies; 4 → 0 (reversed
        // to break the cycle) still runs from 4 to 0.
        assert_eq!(drawing.routes[5].len(), 4);
        assert_eq!(drawing.routes[6].first(), Some(&drawing.centres[4]));
        assert_eq!(drawing.routes[6].last(), Some(&drawing.centres[0]));
        assert!(drawing.routes[7].is_empty());
        assert!(drawing.crossings.1 <= drawing.crossings.0);
        // Deterministic.
        assert_eq!(drawing, layered(&sizes, &edges, false));
    }

    #[test]
    fn a_tangled_dag_untangles() {
        // Three layers of eight; each edge goes to a shuffled slot.
        let edges: Vec<(usize, usize)> = (0..8)
            .flat_map(|i| [(i, 8 + (i * 5) % 8), (8 + i, 16 + (i * 3) % 8)])
            .chain((0..8).map(|i| (i, 8 + (i * 5 + 1) % 8)))
            .collect();
        let drawing = layered(&vec![(30.0, 20.0); 24], &edges, false);
        let (before, after) = drawing.crossings;
        assert!(after < before, "{before} → {after}");
        assert!(drawing.centres.iter().all(|c| c.0.is_finite()));
    }

    #[test]
    fn place_row_keeps_order_and_gaps() {
        assert_eq!(place_row(&[0.0, 0.0], &[10.0]), [-5.0, 5.0]);
        assert_eq!(place_row(&[0.0, 100.0], &[10.0]), [0.0, 100.0]);
        let got = place_row(&[5.0, 0.0, 0.0], &[10.0, 10.0]);
        assert!((got[1] - got[0] - 10.0).abs() < 1e-9 && (got[2] - got[1] - 10.0).abs() < 1e-9);
    }
}
