//! Grid auto-layout of the components and Manhattan routing of the traces.
//!
//! Components with a `layout` hint keep their cell. The others get a
//! column by their longest distance from a source (a component nothing
//! sends to, or the first one of a cycle), following connections in model
//! order and ignoring the edges that close a cycle; rows fill each column
//! top to bottom, skipping hinted cells. Coordinates are board units on
//! the board plane: `x` grows with the column, `z` with the row.
//!
//! Out ports sit on the right side of a chip and in ports on the left.
//! A forward trace runs right, turns at a lane between the columns and
//! enters the target from the left. A trace going back (to the same or an
//! earlier column) leaves right, drops into a channel below the rows,
//! runs left and comes up into the target. Each trace gets its own lane
//! offset so parallel traces do not overlap.

use std::collections::{BTreeMap, BTreeSet};

use super::model::{GridPos, PortDir, SysModel, split_ref};

/// Distance between two cell centers.
pub const CELL: f32 = 8.0;
/// Width and depth of a chip.
pub const CHIP: f32 = 3.6;
/// Space between parallel trace lanes.
const LANE: f32 = 0.45;

/// A point on the board plane, `(x, z)`.
pub type P2 = (f32, f32);

/// Where everything goes.
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    /// Cell of each component.
    pub cells: BTreeMap<String, GridPos>,
    /// Columns used.
    pub cols: u32,
    /// Rows used.
    pub rows: u32,
    /// Polyline of each connection, from the source pin to the target pin.
    pub routes: BTreeMap<String, Vec<P2>>,
    /// Pin position of each `component.port`.
    pub pins: BTreeMap<String, P2>,
}

impl Layout {
    /// Center of a component's cell.
    #[must_use]
    pub fn center(&self, comp: &str) -> Option<P2> {
        self.cells.get(comp).map(|c| cell_center(*c))
    }
}

/// Center of `cell` on the board plane.
#[must_use]
pub fn cell_center(cell: GridPos) -> P2 {
    (cell.col as f32 * CELL, cell.row as f32 * CELL)
}

fn columns(model: &SysModel) -> BTreeMap<String, u32> {
    let ids: Vec<&str> = model.components.iter().map(|c| c.id.as_str()).collect();
    let mut edges: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    let mut indegree: BTreeMap<&str, usize> = ids.iter().map(|id| (*id, 0)).collect();
    for conn in &model.connections {
        let (Some((a, _)), Some((b, _))) = (split_ref(&conn.from), split_ref(&conn.to)) else {
            continue;
        };
        if a == b || !indegree.contains_key(a) || !indegree.contains_key(b) {
            continue;
        }
        edges.entry(a).or_default().push(b);
        *indegree.entry(b).or_default() += 1;
    }
    // Depth-first from the sources (then from whatever is left), keeping
    // only the edges that do not close a cycle.
    let mut order: Vec<&str> = ids.iter().copied().filter(|id| indegree[id] == 0).collect();
    order.extend(ids.iter().copied().filter(|id| indegree[id] != 0));
    let mut forward: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    let mut done: BTreeSet<&str> = BTreeSet::new();
    for root in order {
        if done.contains(root) {
            continue;
        }
        let mut stack: Vec<(&str, usize)> = vec![(root, 0)];
        let mut on_path: BTreeSet<&str> = BTreeSet::from([root]);
        while let Some((node, next)) = stack.pop() {
            let outs = edges.get(node).map_or(&[][..], Vec::as_slice);
            if let Some(&child) = outs.get(next) {
                stack.push((node, next + 1));
                if on_path.contains(child) {
                    continue; // a back edge
                }
                forward.entry(node).or_default().push(child);
                if !done.contains(child) {
                    on_path.insert(child);
                    stack.push((child, 0));
                }
            } else {
                on_path.remove(node);
                done.insert(node);
            }
        }
    }
    // Longest path over the forward edges (a DAG), in topological order.
    let mut col: BTreeMap<String, u32> = ids.iter().map(|id| ((*id).to_owned(), 0)).collect();
    let mut indeg: BTreeMap<&str, usize> = ids.iter().map(|id| (*id, 0)).collect();
    for outs in forward.values() {
        for b in outs {
            *indeg.entry(b).or_default() += 1;
        }
    }
    let mut ready: Vec<&str> = ids.iter().copied().filter(|id| indeg[id] == 0).collect();
    let mut i = 0;
    while i < ready.len() {
        let node = ready[i];
        i += 1;
        let here = col[node];
        for &b in forward.get(node).map_or(&[][..], Vec::as_slice) {
            let entry = col.entry(b.to_owned()).or_default();
            *entry = (*entry).max(here + 1);
            if let Some(d) = indeg.get_mut(b) {
                *d -= 1;
                if *d == 0 {
                    ready.push(b);
                }
            }
        }
    }
    col
}

/// Places the components and routes the connections of `model`.
#[must_use]
pub fn layout(model: &SysModel) -> Layout {
    let cols_of = columns(model);
    let mut taken: BTreeSet<(u32, u32)> = model
        .components
        .iter()
        .filter_map(|c| c.layout.map(|g| (g.col, g.row)))
        .collect();
    let mut cells = BTreeMap::new();
    for c in &model.components {
        if let Some(cell) = c.layout {
            cells.insert(c.id.clone(), cell);
            continue;
        }
        let col = cols_of.get(&c.id).copied().unwrap_or(0);
        let row = (0..).find(|r| !taken.contains(&(col, *r))).unwrap_or(0);
        taken.insert((col, row));
        cells.insert(c.id.clone(), GridPos { col, row });
    }
    let cols = cells.values().map(|c| c.col + 1).max().unwrap_or(1);
    let rows = cells.values().map(|c| c.row + 1).max().unwrap_or(1);

    // Pins: out ports on the right edge, in ports on the left, spread
    // along the edge.
    let mut pins = BTreeMap::new();
    for c in &model.components {
        let Some(&cell) = cells.get(&c.id) else {
            continue;
        };
        let (cx, cz) = cell_center(cell);
        for dir in [PortDir::In, PortDir::Out] {
            let ports: Vec<_> = c.ports.iter().filter(|p| p.dir == dir).collect();
            let n = ports.len() as f32;
            for (i, p) in ports.iter().enumerate() {
                let z = cz + (i as f32 + 0.5 - n / 2.0) * (CHIP / (n + 1.0)).min(0.9);
                let x = match dir {
                    PortDir::In => cx - CHIP / 2.0,
                    PortDir::Out => cx + CHIP / 2.0,
                };
                pins.insert(format!("{}.{}", c.id, p.id), (x, z));
            }
        }
    }

    let mut routes = BTreeMap::new();
    let bottom = (rows as f32 - 0.5) * CELL;
    let mut gap_lanes: BTreeMap<u32, u32> = BTreeMap::new();
    let mut channel_lanes = 0u32;
    for conn in &model.connections {
        let (Some(&start), Some(&end)) = (pins.get(&conn.from), pins.get(&conn.to)) else {
            continue;
        };
        let (Some((a, _)), Some((b, _))) = (split_ref(&conn.from), split_ref(&conn.to)) else {
            continue;
        };
        let (Some(ca), Some(cb)) = (cells.get(a), cells.get(b)) else {
            continue;
        };
        let lane_in_gap = |gaps: &mut BTreeMap<u32, u32>, col: u32| {
            let n = gaps.entry(col).or_insert(0);
            *n += 1;
            (*n as f32 - 1.0) * LANE
        };
        let path = if cb.col > ca.col {
            // Turn in the gap right after the source column.
            let offset = lane_in_gap(&mut gap_lanes, ca.col);
            let mid = ca.col as f32 * CELL + CELL / 2.0 - 1.0 + offset;
            vec![start, (mid, start.1), (mid, end.1), end]
        } else {
            let out = lane_in_gap(&mut gap_lanes, ca.col);
            let back = lane_in_gap(&mut gap_lanes, cb.col.wrapping_sub(1));
            channel_lanes += 1;
            let low = bottom + 0.6 + channel_lanes as f32 * LANE;
            let right = ca.col as f32 * CELL + CELL / 2.0 - 1.0 + out;
            let left = cb.col as f32 * CELL - CELL / 2.0 + 1.0 + back;
            vec![
                start,
                (right, start.1),
                (right, low),
                (left, low),
                (left, end.1),
                end,
            ]
        };
        routes.insert(conn.id.clone(), dedup(path));
    }
    Layout {
        cells,
        cols,
        rows,
        routes,
        pins,
    }
}

fn dedup(points: Vec<P2>) -> Vec<P2> {
    let mut out: Vec<P2> = Vec::with_capacity(points.len());
    for p in points {
        if out
            .last()
            .is_none_or(|q| (q.0 - p.0).abs() > 1e-4 || (q.1 - p.1).abs() > 1e-4)
        {
            out.push(p);
        }
    }
    out
}

/// Length of a polyline.
#[must_use]
pub fn length(points: &[P2]) -> f32 {
    points
        .windows(2)
        .map(|w| ((w[1].0 - w[0].0).powi(2) + (w[1].1 - w[0].1).powi(2)).sqrt())
        .sum()
}

/// The point at `fraction` (0..=1) of the length of a polyline.
#[must_use]
pub fn point_at(points: &[P2], fraction: f32) -> Option<P2> {
    let first = *points.first()?;
    let total = length(points);
    if total <= 0.0 {
        return Some(first);
    }
    let mut left = fraction.clamp(0.0, 1.0) * total;
    for w in points.windows(2) {
        let seg = ((w[1].0 - w[0].0).powi(2) + (w[1].1 - w[0].1).powi(2)).sqrt();
        if left <= seg && seg > 0.0 {
            let t = left / seg;
            return Some((
                w[0].0 + (w[1].0 - w[0].0) * t,
                w[0].1 + (w[1].1 - w[0].1) * t,
            ));
        }
        left -= seg;
    }
    points.last().copied()
}
