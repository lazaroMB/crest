//! Geometry of a [`GraphSpec`]: node boxes, edge polylines, group boxes.
//!
//! [`layout`] runs the spec's [`LayoutKind`] on the node sizes the UI
//! measured and returns a [`Geometry`] with the top-left corner at
//! `(0, 0)`: edges clipped to the borders of their boxes (arrowheads go
//! at the last point), a label anchor per labelled edge and a box per
//! group around its nodes.

use super::{DRAW_LIMIT, FlowDirection, GraphSpec, LayoutKind, force, layered};

/// Space around the drawing.
pub const MARGIN: f64 = 16.0;
/// Space between a group's box and its nodes.
pub const GROUP_PAD: f64 = 10.0;
/// Room above a group's nodes for its caption.
pub const GROUP_CAPTION: f64 = 16.0;
/// Space between grid cells.
const GRID_GAP: f64 = 32.0;
/// Seed of the force layout when the spec has none.
pub const DEFAULT_SEED: u64 = 1;

/// An axis-aligned box: top-left corner and size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    /// Left.
    pub x: f64,
    /// Top.
    pub y: f64,
    /// Width.
    pub w: f64,
    /// Height.
    pub h: f64,
}

impl Rect {
    /// The box of size `(w, h)` centred on `(x, y)`.
    #[must_use]
    pub fn centred((x, y): (f64, f64), (w, h): (f64, f64)) -> Self {
        Self {
            x: x - w / 2.0,
            y: y - h / 2.0,
            w,
            h,
        }
    }

    /// The centre.
    #[must_use]
    pub fn centre(&self) -> (f64, f64) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }

    /// Whether `(x, y)` is inside.
    #[must_use]
    pub fn contains(&self, (x, y): (f64, f64)) -> bool {
        x >= self.x && x <= self.x + self.w && y >= self.y && y <= self.y + self.h
    }

    /// Where the segment from the centre towards `to` leaves the box.
    #[must_use]
    pub fn border_towards(&self, to: (f64, f64)) -> (f64, f64) {
        let (cx, cy) = self.centre();
        let (dx, dy) = (to.0 - cx, to.1 - cy);
        let tx = if dx.abs() > 1e-9 {
            self.w / 2.0 / dx.abs()
        } else {
            f64::INFINITY
        };
        let ty = if dy.abs() > 1e-9 {
            self.h / 2.0 / dy.abs()
        } else {
            f64::INFINITY
        };
        let t = tx.min(ty).min(1.0);
        if t.is_finite() {
            (cx + dx * t, cy + dy * t)
        } else {
            (cx, cy)
        }
    }

    fn translate(&mut self, dx: f64, dy: f64) {
        self.x += dx;
        self.y += dy;
    }

    fn union(self, other: Self) -> Self {
        let (x, y) = (self.x.min(other.x), self.y.min(other.y));
        let right = (self.x + self.w).max(other.x + other.w);
        let bottom = (self.y + self.h).max(other.y + other.h);
        Self {
            x,
            y,
            w: right - x,
            h: bottom - y,
        }
    }
}

/// One edge drawn.
#[derive(Debug, Clone, PartialEq)]
pub struct EdgeRoute {
    /// Index of the edge in the spec.
    pub edge: usize,
    /// From the border of the source box to the border of the target
    /// box; a self-loop is a small loop on the box's right side.
    pub points: Vec<(f64, f64)>,
    /// Where the edge's label goes (the middle of the route).
    pub label_at: Option<(f64, f64)>,
}

/// One group drawn.
#[derive(Debug, Clone, PartialEq)]
pub struct GroupBox {
    /// The group id.
    pub id: String,
    /// Its caption.
    pub label: String,
    /// Around its nodes, with room for the caption on top.
    pub rect: Rect,
}

/// A laid-out graph.
#[derive(Debug, Clone, PartialEq)]
pub struct Geometry {
    /// Box of each node, in spec order.
    pub nodes: Vec<Rect>,
    /// Route of each edge, in spec order.
    pub edges: Vec<EdgeRoute>,
    /// Groups in order of first use.
    pub groups: Vec<GroupBox>,
    /// Size of the drawing, margins included.
    pub width: f64,
    /// Height of the drawing, margins included.
    pub height: f64,
    /// Edge crossings of a layered layout (before and after crossing
    /// reduction); `None` for the others.
    pub crossings: Option<(usize, usize)>,
}

impl Geometry {
    /// The node whose box holds `point`, the topmost (last) first.
    #[must_use]
    pub fn node_at(&self, point: (f64, f64)) -> Option<usize> {
        self.nodes.iter().rposition(|r| r.contains(point))
    }
}

/// Why a graph was not laid out.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LayoutError {
    /// More nodes than [`DRAW_LIMIT`].
    #[error("{nodes} nodes is too many to draw (at most {limit}); filter the graph first")]
    TooLarge {
        /// Nodes of the spec.
        nodes: usize,
        /// The limit.
        limit: usize,
    },
    /// `sizes` does not have one entry per node.
    #[error("{sizes} sizes for {nodes} nodes")]
    Sizes {
        /// Nodes of the spec.
        nodes: usize,
        /// Sizes given.
        sizes: usize,
    },
}

/// Node centres and the bends of each edge.
type Placed = (Vec<(f64, f64)>, Vec<Vec<(f64, f64)>>);

/// Lays out `spec` with one `(width, height)` per node in `sizes`.
///
/// # Errors
///
/// [`LayoutError::TooLarge`] past [`DRAW_LIMIT`] nodes, and
/// [`LayoutError::Sizes`] when `sizes` does not match the nodes.
pub fn layout(spec: &GraphSpec, sizes: &[(f64, f64)]) -> Result<Geometry, LayoutError> {
    let n = spec.nodes.len();
    if n > DRAW_LIMIT {
        return Err(LayoutError::TooLarge {
            nodes: n,
            limit: DRAW_LIMIT,
        });
    }
    if sizes.len() != n {
        return Err(LayoutError::Sizes {
            nodes: n,
            sizes: sizes.len(),
        });
    }
    let ends = spec.ends();
    let mut crossings = None;
    let (centres, bends): Placed = match spec.layout {
        LayoutKind::Layered => {
            let right = spec.direction == FlowDirection::Right;
            let flip = |(a, b): (f64, f64)| if right { (b, a) } else { (a, b) };
            let sizes: Vec<(f64, f64)> = sizes.iter().copied().map(flip).collect();
            let labelled = spec.edges.iter().any(|e| e.label.is_some());
            let drawing = layered::layered(&sizes, &ends, labelled);
            crossings = Some(drawing.crossings);
            let centres = drawing.centres.into_iter().map(flip).collect();
            let routes = drawing
                .routes
                .into_iter()
                .map(|route| {
                    let inner = route.len().saturating_sub(2);
                    route.into_iter().skip(1).take(inner).map(flip).collect()
                })
                .collect();
            (centres, routes)
        }
        LayoutKind::Force => (
            force::force(sizes, &ends, spec.seed.unwrap_or(DEFAULT_SEED)),
            vec![Vec::new(); ends.len()],
        ),
        LayoutKind::Grid => (grid(spec, sizes), vec![Vec::new(); ends.len()]),
    };
    let nodes: Vec<Rect> = centres
        .iter()
        .zip(sizes)
        .map(|(&c, &s)| Rect::centred(c, s))
        .collect();
    let edges = ends
        .iter()
        .zip(bends)
        .enumerate()
        .map(|(ix, (&(a, b), bends))| route(ix, &nodes, a, b, bends, spec))
        .collect();
    let groups = spec
        .groups_in_use()
        .into_iter()
        .filter_map(|id| {
            let rect = spec
                .nodes
                .iter()
                .zip(&nodes)
                .filter(|(node, _)| node.group.as_deref() == Some(id))
                .map(|(_, rect)| *rect)
                .reduce(Rect::union)?;
            Some(GroupBox {
                id: id.to_owned(),
                label: spec.group_label(id).to_owned(),
                rect: Rect {
                    x: rect.x - GROUP_PAD,
                    y: rect.y - GROUP_PAD - GROUP_CAPTION,
                    w: rect.w + 2.0 * GROUP_PAD,
                    h: rect.h + 2.0 * GROUP_PAD + GROUP_CAPTION,
                },
            })
        })
        .collect();
    let mut geometry = Geometry {
        nodes,
        edges,
        groups,
        width: 0.0,
        height: 0.0,
        crossings,
    };
    normalise(&mut geometry);
    Ok(geometry)
}

/// Centres on a grid in node order, nodes of a group together.
fn grid(spec: &GraphSpec, sizes: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let n = sizes.len();
    let groups = spec.groups_in_use();
    let rank = |ix: usize| {
        spec.nodes[ix]
            .group
            .as_deref()
            .and_then(|g| groups.iter().position(|x| *x == g))
            .map_or(0, |p| p + 1)
    };
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by_key(|&ix| rank(ix));
    let columns = ((n as f64).sqrt().ceil() as usize).max(1);
    let cell_w = sizes.iter().map(|s| s.0).fold(0.0, f64::max) + GRID_GAP;
    let cell_h = sizes.iter().map(|s| s.1).fold(0.0, f64::max) + GRID_GAP * 1.5;
    let mut centres = vec![(0.0, 0.0); n];
    for (slot, ix) in order.into_iter().enumerate() {
        let (col, row) = (slot % columns, slot / columns);
        centres[ix] = (
            col as f64 * cell_w + cell_w / 2.0,
            row as f64 * cell_h + cell_h / 2.0,
        );
    }
    centres
}

/// The route of edge `ix` from node `a` to node `b` through `bends`.
fn route(
    ix: usize,
    nodes: &[Rect],
    a: usize,
    b: usize,
    bends: Vec<(f64, f64)>,
    spec: &GraphSpec,
) -> EdgeRoute {
    let (Some(from), Some(to)) = (nodes.get(a), nodes.get(b)) else {
        return EdgeRoute {
            edge: ix,
            points: Vec::new(),
            label_at: None,
        };
    };
    let points = if a == b {
        // A loop out of the right side and back in.
        let (right, top) = (from.x + from.w, from.y);
        let h = from.h;
        vec![
            (right, top + h * 0.25),
            (right + 18.0, top + h * 0.1),
            (right + 18.0, top + h * 0.9),
            (right, top + h * 0.75),
        ]
    } else {
        let first = bends.first().copied().unwrap_or(to.centre());
        let last = bends.last().copied().unwrap_or(from.centre());
        let mut points = Vec::with_capacity(bends.len() + 2);
        points.push(from.border_towards(first));
        points.extend(bends);
        points.push(to.border_towards(last));
        points
    };
    let label_at = spec
        .edges
        .get(ix)
        .and_then(|e| e.label.as_ref())
        .map(|_| middle(&points));
    EdgeRoute {
        edge: ix,
        points,
        label_at,
    }
}

/// The middle of a polyline: its middle point, or the middle of its
/// middle segment.
fn middle(points: &[(f64, f64)]) -> (f64, f64) {
    match points.len() {
        0 => (0.0, 0.0),
        len if len % 2 == 1 => points[len / 2],
        len => {
            let (a, b) = (points[len / 2 - 1], points[len / 2]);
            ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0)
        }
    }
}

/// Moves the drawing so its top-left corner is at the margin and sets
/// its size.
fn normalise(geometry: &mut Geometry) {
    let boxes = geometry
        .nodes
        .iter()
        .copied()
        .chain(geometry.groups.iter().map(|g| g.rect));
    let points = geometry
        .edges
        .iter()
        .flat_map(|e| e.points.iter().copied())
        .map(|(x, y)| Rect {
            x,
            y,
            w: 0.0,
            h: 0.0,
        });
    let Some(bounds) = boxes.chain(points).reduce(Rect::union) else {
        geometry.width = 2.0 * MARGIN;
        geometry.height = 2.0 * MARGIN;
        return;
    };
    let (dx, dy) = (MARGIN - bounds.x, MARGIN - bounds.y);
    geometry.nodes.iter_mut().for_each(|r| r.translate(dx, dy));
    geometry
        .groups
        .iter_mut()
        .for_each(|g| g.rect.translate(dx, dy));
    for edge in &mut geometry.edges {
        for p in &mut edge.points {
            *p = (p.0 + dx, p.1 + dy);
        }
        if let Some(at) = &mut edge.label_at {
            *at = (at.0 + dx, at.1 + dy);
        }
    }
    geometry.width = bounds.w + 2.0 * MARGIN;
    geometry.height = bounds.h + 2.0 * MARGIN;
}

#[cfg(test)]
mod tests {
    use super::super::tests::spec;
    use super::*;

    fn sizes(n: usize) -> Vec<(f64, f64)> {
        vec![(60.0, 24.0); n]
    }

    #[test]
    fn layered_edges_start_and_end_on_the_boxes() {
        let mut spec = spec(&["a", "b", "c"], &[("a", "b"), ("b", "c"), ("a", "c")]);
        spec.edges[0].label = Some("x".into());
        let g = layout(&spec, &sizes(3)).unwrap();
        assert!(
            g.nodes
                .iter()
                .all(|r| r.x >= MARGIN - 1e-9 && r.y >= MARGIN - 1e-9)
        );
        // a above b above c.
        assert!(g.nodes[0].y < g.nodes[1].y && g.nodes[1].y < g.nodes[2].y);
        let first = g.edges[0].points[0];
        assert!(
            (first.1 - (g.nodes[0].y + g.nodes[0].h)).abs() < 1e-6,
            "{first:?}"
        );
        let last = *g.edges[0].points.last().unwrap();
        assert!((last.1 - g.nodes[1].y).abs() < 1e-6);
        // a → c skips a layer: one bend.
        assert_eq!(g.edges[2].points.len(), 3);
        assert!(g.edges[0].label_at.is_some() && g.edges[1].label_at.is_none());
        assert!(g.crossings.is_some());
        assert!(g.width > 60.0 && g.height > 3.0 * 24.0);
    }

    #[test]
    fn rightward_layers_run_left_to_right() {
        let mut spec = spec(&["a", "b"], &[("a", "b")]);
        spec.direction = FlowDirection::Right;
        let g = layout(&spec, &sizes(2)).unwrap();
        assert!(g.nodes[0].x + g.nodes[0].w < g.nodes[1].x);
        assert!((g.nodes[0].y - g.nodes[1].y).abs() < 1e-6);
    }

    #[test]
    fn grid_and_force_place_every_node_and_groups_wrap_them() {
        let mut spec = spec(&["a", "b", "c", "d", "e"], &[("a", "b"), ("c", "c")]);
        spec.nodes[1].group = Some("g".into());
        spec.nodes[3].group = Some("g".into());
        for kind in [LayoutKind::Grid, LayoutKind::Force] {
            spec.layout = kind;
            let g = layout(&spec, &sizes(5)).unwrap();
            assert_eq!(g.nodes.len(), 5);
            assert_eq!(g.groups.len(), 1);
            let group = g.groups[0].rect;
            for ix in [1, 3] {
                assert!(group.contains(g.nodes[ix].centre()), "{kind:?}");
            }
            // The self-loop is a small loop on the right of c.
            assert_eq!(g.edges[1].points.len(), 4);
            assert!(g.edges[1].points[1].0 > g.nodes[2].x + g.nodes[2].w);
            assert_eq!(g.node_at(g.nodes[4].centre()), Some(4));
            assert_eq!(g.crossings, None);
        }
        // Grid keeps the group's nodes next to each other.
        spec.layout = LayoutKind::Grid;
        let g = layout(&spec, &sizes(5)).unwrap();
        let order = |ix: usize| (g.nodes[ix].y.round() as i64, g.nodes[ix].x.round() as i64);
        let mut sorted: Vec<usize> = (0..5).collect();
        sorted.sort_by_key(|&ix| order(ix));
        assert_eq!(sorted, [0, 2, 4, 1, 3]);
    }

    #[test]
    fn big_graphs_and_wrong_sizes_are_refused() {
        let ids: Vec<String> = (0..=DRAW_LIMIT).map(|i| format!("n{i}")).collect();
        let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        let big = spec(&refs, &[]);
        let err = layout(&big, &sizes(DRAW_LIMIT + 1)).unwrap_err();
        assert!(err.to_string().contains("too many"), "{err}");
        let small = spec(&["a"], &[]);
        assert!(matches!(
            layout(&small, &[]),
            Err(LayoutError::Sizes { .. })
        ));
        let empty = spec(&[], &[]);
        assert_eq!(layout(&empty, &[]).unwrap().width, 2.0 * MARGIN);
    }

    #[test]
    fn border_points_sit_on_the_box() {
        let r = Rect::centred((0.0, 0.0), (40.0, 20.0));
        assert_eq!(r.border_towards((100.0, 0.0)), (20.0, 0.0));
        assert_eq!(r.border_towards((0.0, -100.0)), (0.0, -10.0));
        assert_eq!(r.border_towards((0.0, 0.0)), (0.0, 0.0));
    }
}
