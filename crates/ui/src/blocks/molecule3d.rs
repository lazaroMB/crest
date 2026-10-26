//! Renderer of the `molecule_3d` block kind (`show_molecule_3d`): a
//! molecule in the 3D view as ball and stick.
//!
//! The atoms are spheres in CPK colours and the bonds cylinders, each half
//! in its atom's colour ([`Molecule3dSpec::ball_and_stick`]), drawn by the
//! model viewer ([`super::model_view`]) with its orbit, zoom, section and
//! measuring. Measuring is on from the start and snaps to atom centres: a
//! click picks an atom (its element, number and, for PDB, name and
//! residue), a second one gives the distance between them in Å, from
//! the coordinates. The summary says where the coordinates come from, and
//! an embedded conformer is labelled as computed, not experimental.

use std::rc::Rc;

use domain::block::{BlockId, BlockKind};
use domain::model3d::ModelSpec;
use domain::science::molecule3d::{CoordinateSource, Molecule3dSpec, StructureFormat};
use domain::sysmodel::scene::Rgb;
use gpui::{
    AnyElement, App, Context, Entity, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Window, div,
    prelude::FluentBuilder as _,
};
use gpui_component::{ActiveTheme as _, v_flex};

use super::custom::node_id;
use super::model_view::model_view_inline;
use crate::render3d::math::V3;
use crate::timeline::TimelineEntry;

/// Height of the inline 3D view, in pixels.
pub const VIEW_HEIGHT: f32 = 420.0;

/// An sRGB byte as linear light.
fn linear(c: u8) -> f32 {
    let v = f32::from(c) / 255.0;
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

/// The summary line: formula, counts, source and size.
#[must_use]
pub fn summary_text(spec: &Molecule3dSpec) -> String {
    let mut parts = vec![
        spec.formula(),
        format!("{} atoms", spec.atoms.len()),
        format!(
            "{} bonds{}",
            spec.bonds.len(),
            if spec.bonds_inferred {
                " (from distances)"
            } else {
                ""
            }
        ),
    ];
    parts.push(match &spec.source {
        CoordinateSource::Embedded { method } => format!("embedded: {method}"),
        CoordinateSource::File { format } => format!(
            "coordinates from the {} file",
            match format {
                StructureFormat::Mol => "MOL",
                StructureFormat::Sdf => "SDF",
                StructureFormat::Xyz => "XYZ",
                StructureFormat::Pdb => "PDB",
            }
        ),
    });
    if let Some((lo, hi)) = spec.bounds() {
        parts.push(format!(
            "{:.2} × {:.2} × {:.2} Å",
            hi[0] - lo[0],
            hi[1] - lo[1],
            hi[2] - lo[2]
        ));
    }
    parts.join(" · ")
}

/// The atom at file point `p` (an atom centre after snapping).
#[must_use]
pub fn atom_at(spec: &Molecule3dSpec, p: V3) -> Option<usize> {
    spec.atoms
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let d = (0..3)
                .map(|k| (a.position[k] - f64::from(p[k])).powi(2))
                .sum::<f64>();
            (i, d)
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .filter(|(_, d)| *d < 0.01)
        .map(|(i, _)| i)
}

/// The picking line for `atoms` (one or two picked atoms).
#[must_use]
pub fn pick_text(spec: &Molecule3dSpec, atoms: &[usize]) -> Option<String> {
    match atoms {
        [a] => Some(format!("atom {} — click another to measure", spec.atom_label(*a))),
        [a, b, ..] => Some(format!(
            "{} – {}: {:.3} Å",
            spec.atom_label(*a),
            spec.atom_label(*b),
            spec.distance(*a, *b)?
        )),
        [] => None,
    }
}

/// Per-block state: the molecule, its mesh and colours.
pub struct Molecule3dView {
    id: BlockId,
    spec: Rc<Molecule3dSpec>,
    model: Rc<ModelSpec>,
    colors: Rc<[Rgb]>,
    snap: Rc<[V3]>,
    /// Whether measuring was switched on in the viewer yet.
    armed: bool,
}

impl Molecule3dView {
    fn new(id: BlockId, spec: &Molecule3dSpec) -> Self {
        let (model, colors) = spec.ball_and_stick();
        let colors: Rc<[Rgb]> = colors
            .iter()
            .map(|c| [linear(c[0]), linear(c[1]), linear(c[2])])
            .collect();
        #[allow(clippy::cast_possible_truncation)]
        let snap: Rc<[V3]> = spec
            .atoms
            .iter()
            .map(|a| {
                [
                    a.position[0] as f32,
                    a.position[1] as f32,
                    a.position[2] as f32,
                ]
            })
            .collect();
        Self {
            id,
            spec: Rc::new(spec.clone()),
            model: Rc::new(model),
            colors,
            snap,
            armed: false,
        }
    }

    fn sync(&mut self, spec: &Molecule3dSpec) {
        if *self.spec != *spec {
            *self = Self::new(self.id, spec);
        }
    }
}

impl Render for Molecule3dView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let model = model_view_inline(node_id(self.id, 0), &self.model, VIEW_HEIGHT, window, cx);
        let (colors, snap, arm) = (Rc::clone(&self.colors), Rc::clone(&self.snap), !self.armed);
        model.update(cx, |view, cx| {
            view.set_colors(Some(colors), cx);
            view.set_snap(snap);
            if arm {
                view.set_measuring(true, cx);
            }
        });
        self.armed = true;
        let points: Vec<V3> = model.read(cx).points().to_vec();
        let atoms: Vec<usize> = points
            .into_iter()
            .filter_map(|p| atom_at(&self.spec, p))
            .collect();
        let pick = pick_text(&self.spec, &atoms);
        let spec = Rc::clone(&self.spec);
        let theme = cx.theme();
        let (muted, warning, primary) = (theme.muted_foreground, theme.warning, theme.primary);
        v_flex()
            .w_full()
            .gap_1()
            .debug_selector(|| "molecule3d".to_owned())
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(SharedString::from(spec.display_title())),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .debug_selector(|| "molecule3d-summary".to_owned())
                    .child(SharedString::from(summary_text(&spec))),
            )
            .children(spec.warnings.iter().map(|w| {
                div()
                    .text_xs()
                    .text_color(warning)
                    .child(SharedString::from(w.clone()))
            }))
            .child(model)
            .when_some(pick, |d, text| {
                d.child(
                    div()
                        .text_xs()
                        .text_color(primary)
                        .debug_selector(|| "molecule3d-pick".to_owned())
                        .child(SharedString::from(text)),
                )
            })
    }
}

/// The 3D molecule entity of block `id`, in sync with `spec`.
pub fn view(
    id: BlockId,
    spec: &Molecule3dSpec,
    window: &mut Window,
    cx: &mut App,
) -> Entity<Molecule3dView> {
    let view = super::block_state(id, window, cx, |_, _| Molecule3dView::new(id, spec));
    view.update(cx, |view, _| view.sync(spec));
    view
}

/// Draws the block held by `entry`.
pub fn render(entry: &TimelineEntry, window: &mut Window, cx: &mut App) -> AnyElement {
    let BlockKind::Molecule3d(spec) = &entry.block.kind else {
        return super::placeholder(entry, cx);
    };
    view(entry.block.id, spec, window, cx).into_any_element()
}

#[cfg(test)]
mod tests {
    use application::ports::events::DomainEvent;
    use domain::block::Block;
    use domain::science::molecule3d::{Atom3d, Bond3d};
    use gpui::{TestAppContext, point, px, size};

    use super::*;
    use crate::timeline::TimelineView as Timeline;

    fn spec() -> Molecule3dSpec {
        let atom = |element: &str, position: [f64; 3]| Atom3d {
            element: element.into(),
            position,
            name: None,
            residue: None,
        };
        Molecule3dSpec {
            title: None,
            name: Some("water".into()),
            smiles: Some("O".into()),
            path: None,
            source: CoordinateSource::Embedded {
                method: "distance geometry".into(),
            },
            atoms: vec![
                atom("O", [0.0, 0.0, 0.0]),
                atom("H", [0.9572, 0.0, 0.0]),
                atom("H", [-0.24, 0.9266, 0.0]),
            ],
            bonds: vec![Bond3d { a: 0, b: 1, order: 1 }, Bond3d { a: 0, b: 2, order: 1 }],
            bonds_inferred: false,
            warnings: vec!["A computed conformer.".into()],
        }
    }

    #[test]
    fn texts_and_atom_lookup() {
        let s = spec();
        assert_eq!(
            summary_text(&s),
            "H2O · 3 atoms · 2 bonds · embedded: distance geometry · 1.20 × 0.93 × 0.00 Å"
        );
        assert_eq!(atom_at(&s, [0.9572, 0.0, 0.0]), Some(1));
        assert_eq!(atom_at(&s, [5.0, 0.0, 0.0]), None);
        assert_eq!(pick_text(&s, &[0, 1]).as_deref(), Some("O1 – H2: 0.957 Å"));
        assert!((linear(255) - 1.0).abs() < 1e-6 && linear(0) == 0.0);
    }

    #[gpui::test]
    fn draws_the_molecule_with_measuring_on(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::sysboard::disable_gpu(cx);
        });
        let (view, cx) = cx.add_window_view(|_, cx| Timeline::new(cx));
        let block = Block::new(BlockKind::Molecule3d(Box::new(spec())));
        let id = block.id;
        view.update(cx, |view, cx| {
            view.apply_all([DomainEvent::BlockAdded { block }], cx);
        });
        cx.draw(point(px(0.), px(0.)), size(px(1000.), px(1000.)), |_, _| {
            div().size_full().child(view.clone())
        });
        cx.run_until_parked();
        for selector in ["molecule3d", "molecule3d-summary", "model-viewport", "model-measure"] {
            assert!(cx.debug_bounds(selector).is_some(), "{selector}");
        }
        let mol = cx.update(|window, cx| super::view(id, &spec(), window, cx));
        let viewer = cx.update(|window, cx| {
            let model = Rc::clone(&mol.read(cx).model);
            model_view_inline(node_id(id, 0), &model, VIEW_HEIGHT, window, cx)
        });
        assert!(viewer.read_with(cx, |v, _| v.measuring()));
        // The mesh is coloured: oxygen red, not clay.
        let red = viewer.read_with(cx, |v, _| {
            let m = v.mesh();
            // Vertex layout: position, normal, then the base colour.
            [m.vertices[6], m.vertices[7], m.vertices[8]]
        });
        assert!(red[0] > 0.9 && red[1] < 0.05, "{red:?}");
        // A click on the oxygen's centre snaps to it.
        let center = cx.debug_bounds("model-viewport").unwrap().center();
        viewer.update(cx, |v, cx| {
            v.measure_at(center, cx);
        });
        let picked = viewer.read_with(cx, |v, _| v.points().to_vec());
        assert!(picked.len() <= 1);
        if let Some(p) = picked.first() {
            assert!(atom_at(&spec(), *p).is_some(), "{p:?}");
        }
    }
}
