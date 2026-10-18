//! A renderer-agnostic 3D description of the board, built from the model
//! and its [`Layout`]: the board slab, chips (bevelled boxes), pins, trace
//! ribbons and label anchors, each with a material. The window's wgpu
//! renderer, its 2.5D fallback and the HTML export all draw this same
//! geometry.
//!
//! World axes: `x` along the columns, `y` up, `z` along the rows; the top
//! of the board is `y = 0`.
//!
//! [`frame_at`] is the moving part: the packets in flight and the colour
//! of every chip at an instant, a pure function of the trace and the time,
//! so scrubbing and replaying need no simulation.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::layout::{self, CHIP, Layout, P2};
use super::model::{ComponentKind, PortDir, SysModel};
use super::trace::{EventKind, Trace};

/// A point or a size in world units.
pub type V3 = [f32; 3];
/// A linear RGB colour.
pub type Rgb = [f32; 3];

/// How a surface looks.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Material {
    /// Base colour.
    pub base: Rgb,
    /// Light it emits (glows under bloom).
    pub emissive: Rgb,
    /// 0 dielectric, 1 metal.
    pub metallic: f32,
    /// 0 mirror, 1 matte.
    pub roughness: f32,
}

impl Material {
    const fn matte(base: Rgb) -> Self {
        Self {
            base,
            emissive: [0.0; 3],
            metallic: 0.0,
            roughness: 0.8,
        }
    }
}

/// The board under everything.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Slab {
    /// Lowest corner.
    pub min: V3,
    /// Highest corner (its `y` is 0).
    pub max: V3,
    /// Look.
    pub material: Material,
}

/// A pin on the side of a chip.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Pin {
    /// Port id.
    pub port: String,
    /// Whether messages enter or leave.
    pub out: bool,
    /// Center of the pin.
    pub at: V3,
}

/// A component.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Chip {
    /// Component id.
    pub id: String,
    /// Shown name.
    pub label: String,
    /// Kind.
    pub kind: ComponentKind,
    /// Center of the box.
    pub center: V3,
    /// Width, height and depth.
    pub size: V3,
    /// Radius of the rounded edges.
    pub bevel: f32,
    /// Look (the emissive part changes with the state, see [`frame_at`]).
    pub material: Material,
    /// Pins.
    pub pins: Vec<Pin>,
}

impl Chip {
    /// Lowest corner of its box.
    #[must_use]
    pub fn min(&self) -> V3 {
        [
            self.center[0] - self.size[0] / 2.0,
            self.center[1] - self.size[1] / 2.0,
            self.center[2] - self.size[2] / 2.0,
        ]
    }

    /// Highest corner of its box.
    #[must_use]
    pub fn max(&self) -> V3 {
        [
            self.center[0] + self.size[0] / 2.0,
            self.center[1] + self.size[1] / 2.0,
            self.center[2] + self.size[2] / 2.0,
        ]
    }
}

/// A connection, as a flat ribbon on the board.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Ribbon {
    /// Connection id.
    pub id: String,
    /// Polyline just above the board.
    pub points: Vec<V3>,
    /// Width.
    pub width: f32,
    /// Look.
    pub material: Material,
}

/// Where a chip's label goes; the front end draws the text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SceneLabel {
    /// Chip id.
    pub chip: String,
    /// Text.
    pub text: String,
    /// World point it is centered on.
    pub at: V3,
}

/// The static part of the board.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Scene {
    /// The board.
    pub board: Slab,
    /// Components.
    pub chips: Vec<Chip>,
    /// Connections.
    pub ribbons: Vec<Ribbon>,
    /// Label anchors.
    pub labels: Vec<SceneLabel>,
}

/// A message in flight.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Packet {
    /// Message id.
    pub msg: u32,
    /// Message type.
    pub kind: String,
    /// Where it is now.
    pub at: V3,
    /// Where it was a moment ago, newest first.
    pub trail: Vec<V3>,
    /// Colour (emissive).
    pub color: Rgb,
}

/// How a chip looks at an instant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ChipLook {
    /// Chip id.
    pub id: String,
    /// State of its machine.
    pub state: String,
    /// Emissive colour for that state.
    pub emissive: Rgb,
    /// Whether it is handling something.
    pub busy: bool,
    /// Whether it is up.
    pub up: bool,
    /// Messages waiting.
    pub queued: usize,
}

/// The moving part of the board at an instant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Frame {
    /// Time.
    pub t_us: u64,
    /// Messages in flight.
    pub packets: Vec<Packet>,
    /// One per chip, in scene order.
    pub chips: Vec<ChipLook>,
}

/// Height of the trace ribbons above the board.
pub const RIBBON_Y: f32 = 0.03;
/// Height of the packets above the board.
pub const PACKET_Y: f32 = 0.35;
/// Packet radius.
pub const PACKET_RADIUS: f32 = 0.28;

const fn kind_style(kind: ComponentKind) -> (Rgb, f32) {
    // Base colour and height.
    match kind {
        ComponentKind::Client => ([0.20, 0.36, 0.62], 0.9),
        ComponentKind::Service => ([0.16, 0.20, 0.27], 1.1),
        ComponentKind::Queue => ([0.42, 0.30, 0.10], 0.8),
        ComponentKind::Db => ([0.10, 0.34, 0.30], 1.6),
        ComponentKind::Cache => ([0.38, 0.14, 0.40], 1.0),
        ComponentKind::Lb => ([0.30, 0.32, 0.12], 0.7),
        ComponentKind::External => ([0.30, 0.30, 0.32], 1.0),
        ComponentKind::Timer => ([0.36, 0.20, 0.12], 0.6),
    }
}

/// A bright, distinct colour for the `i`-th of something (states,
/// message types).
#[must_use]
pub fn palette(i: usize) -> Rgb {
    const COLORS: [Rgb; 8] = [
        [0.10, 0.85, 1.00],
        [0.25, 1.00, 0.45],
        [1.00, 0.70, 0.10],
        [0.90, 0.35, 1.00],
        [1.00, 0.45, 0.55],
        [0.55, 0.65, 1.00],
        [0.95, 0.95, 0.35],
        [0.30, 1.00, 0.85],
    ];
    COLORS[i % COLORS.len()]
}

/// Colour of a dead chip.
pub const DOWN: Rgb = [1.0, 0.12, 0.10];

fn lift(p: P2, y: f32) -> V3 {
    [p.0, y, p.1]
}

/// Builds the board of `model` laid out by `layout`.
#[must_use]
pub fn build_scene(model: &SysModel, layout: &Layout) -> Scene {
    let mut chips = Vec::new();
    let mut labels = Vec::new();
    let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
    let mut grow = |p: P2| {
        lo = [lo[0].min(p.0), lo[1].min(p.1)];
        hi = [hi[0].max(p.0), hi[1].max(p.1)];
    };
    for c in &model.components {
        let Some(center) = layout.center(&c.id) else {
            continue;
        };
        let (base, height) = kind_style(c.kind);
        grow((center.0 - CHIP / 2.0, center.1 - CHIP / 2.0));
        grow((center.0 + CHIP / 2.0, center.1 + CHIP / 2.0));
        let pins = c
            .ports
            .iter()
            .filter_map(|p| {
                let at = layout.pins.get(&format!("{}.{}", c.id, p.id))?;
                let out = p.dir == PortDir::Out;
                let dx = if out { 0.18 } else { -0.18 };
                Some(Pin {
                    port: p.id.clone(),
                    out,
                    at: [at.0 + dx, 0.18, at.1],
                })
            })
            .collect();
        chips.push(Chip {
            id: c.id.clone(),
            label: c.name().to_owned(),
            kind: c.kind,
            center: [center.0, height / 2.0, center.1],
            size: [CHIP, height, CHIP],
            bevel: 0.18,
            material: Material {
                base,
                emissive: [0.0; 3],
                metallic: 0.2,
                roughness: 0.5,
            },
            pins,
        });
        labels.push(SceneLabel {
            chip: c.id.clone(),
            text: c.name().to_owned(),
            at: [center.0, height + 0.4, center.1],
        });
    }
    let mut ribbons = Vec::new();
    for conn in &model.connections {
        let Some(route) = layout.routes.get(&conn.id) else {
            continue;
        };
        for p in route {
            grow(*p);
        }
        ribbons.push(Ribbon {
            id: conn.id.clone(),
            points: route.iter().map(|p| lift(*p, RIBBON_Y)).collect(),
            width: 0.22,
            material: Material {
                base: [0.75, 0.55, 0.20],
                emissive: [0.08, 0.05, 0.01],
                metallic: 0.9,
                roughness: 0.35,
            },
        });
    }
    if lo[0] > hi[0] {
        (lo, hi) = ([0.0; 2], [0.0; 2]);
    }
    let margin = 2.0;
    Scene {
        board: Slab {
            min: [lo[0] - margin, -0.4, lo[1] - margin],
            max: [hi[0] + margin, 0.0, hi[1] + margin],
            material: Material::matte([0.03, 0.12, 0.08]),
        },
        chips,
        ribbons,
        labels,
    }
}

/// Lays out and builds the board of `model`.
#[must_use]
pub fn scene_of(model: &SysModel) -> Scene {
    build_scene(model, &layout::layout(model))
}

/// Colour of message type `kind` among the model's types.
#[must_use]
pub fn message_color(model: &SysModel, kind: &str) -> Rgb {
    let mut kinds: Vec<&str> = model
        .components
        .iter()
        .flat_map(|c| c.machine.transitions.iter().map(|t| t.on.as_str()))
        .collect();
    kinds.sort_unstable();
    kinds.dedup();
    palette(kinds.iter().position(|k| *k == kind).unwrap_or(kinds.len()))
}

fn along(points: &[V3], fraction: f32) -> V3 {
    let flat: Vec<P2> = points.iter().map(|p| (p[0], p[2])).collect();
    let p = layout::point_at(&flat, fraction).unwrap_or((0.0, 0.0));
    [p.0, PACKET_Y, p.1]
}

/// Packets in flight and chip colours of `trace` at `t_us`.
#[must_use]
pub fn frame_at(model: &SysModel, scene: &Scene, trace: &Trace, t_us: u64) -> Frame {
    let mut packets = Vec::new();
    // Events after `t_us` cannot be in flight yet.
    let until = trace.events_until(t_us);
    for event in &trace.events[..until] {
        let EventKind::Sent {
            msg,
            kind,
            conn,
            arrive_us,
            ..
        } = &event.kind
        else {
            continue;
        };
        if *arrive_us <= t_us || *arrive_us <= event.at_us {
            continue;
        }
        let Some(ribbon) = scene.ribbons.iter().find(|r| &r.id == conn) else {
            continue;
        };
        let span = (*arrive_us - event.at_us) as f32;
        let f = (t_us - event.at_us) as f32 / span;
        let trail = (1..=4)
            .map(|k| along(&ribbon.points, (f - 0.05 * k as f32).max(0.0)))
            .collect();
        packets.push(Packet {
            msg: *msg,
            kind: kind.clone(),
            at: along(&ribbon.points, f),
            trail,
            color: message_color(model, kind),
        });
    }
    let snap = trace.snapshot_after(until);
    let chips = scene
        .chips
        .iter()
        .map(|chip| {
            let state = snap.components.get(&chip.id);
            let name = state.map_or_else(String::new, |s| s.state.clone());
            let up = state.is_none_or(|s| s.up);
            let index = model
                .component(&chip.id)
                .and_then(|c| c.machine.states.iter().position(|s| *s == name))
                .unwrap_or(0);
            ChipLook {
                id: chip.id.clone(),
                emissive: if up { palette(index) } else { DOWN },
                state: name,
                busy: state.is_some_and(|s| s.busy > 0),
                up,
                queued: state.map_or(0, |s| s.queue.len()),
            }
        })
        .collect();
    Frame {
        t_us,
        packets,
        chips,
    }
}
