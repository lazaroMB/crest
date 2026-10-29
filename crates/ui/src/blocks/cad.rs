//! Renderer of the `cad_model` block kind (`show_step`): a STEP part in
//! the 3D view.
//!
//! A summary line comes first (protocol, CAD tool, unit, box, solid,
//! face and triangle counts), then the faces that could not be meshed in
//! the warning color. The model viewer ([`super::model_view`]) draws the
//! part inline with its section plane and measuring. The solid list under
//! it gives each solid's product, faces, triangles and box, with a toggle
//! that hides it in the view. Reader notes are listed last.

use std::collections::BTreeSet;
use std::rc::Rc;

use domain::block::{BlockId, BlockKind};
use domain::engineering::cad::CadModelSpec;
use domain::model3d::ModelSpec;
use gpui::{
    AnyElement, App, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    prelude::FluentBuilder as _,
};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};

use super::custom::node_id;
use super::model_view::model_view_inline;
use crate::timeline::TimelineEntry;

/// Height of the inline 3D view, in pixels.
pub const VIEW_HEIGHT: f32 = 440.0;
/// Solids listed; the rest are counted.
pub const MAX_ROWS: usize = 200;
/// Notes listed.
const LISTED: usize = 6;

/// A length: up to 3 decimals, no trailing zeros.
fn number(v: f32) -> String {
    let text = format!("{v:.3}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text.is_empty() || text == "-0" {
        "0".to_owned()
    } else {
        text.to_owned()
    }
}

fn size_text(size: [f32; 3], unit: &str) -> String {
    format!(
        "{} × {} × {} {unit}",
        number(size[0]),
        number(size[1]),
        number(size[2])
    )
}

/// The summary text.
#[must_use]
pub fn summary_text(spec: &CadModelSpec) -> String {
    let mut parts = vec![spec.protocol().unwrap_or("STEP").to_owned()];
    if let Some(app) = &spec.application {
        parts.push(app.clone());
    }
    if let Some((min, max)) = spec.bounds() {
        let size = [max[0] - min[0], max[1] - min[1], max[2] - min[2]];
        parts.push(size_text(size, spec.unit_label()));
    }
    parts.push(format!("{} solids", spec.solids.len()));
    parts.push(format!("{} faces", spec.faces()));
    parts.push(format!("{} triangles", spec.mesh.triangles.len()));
    if spec.unit.is_none() {
        parts.push("no unit in the file".to_owned());
    }
    parts.join(" · ")
}

/// The not-meshed line, if any face failed.
#[must_use]
pub fn failed_text(spec: &CadModelSpec) -> Option<String> {
    let failed = spec.failed_faces();
    (failed > 0).then(|| {
        format!(
            "{failed} of {} faces could not be meshed and are missing from the view",
            spec.faces()
        )
    })
}

/// Per-block state: the part, the model the viewer draws and the hidden
/// solids.
pub struct CadView {
    id: BlockId,
    spec: Rc<CadModelSpec>,
    model: Rc<ModelSpec>,
    hidden: BTreeSet<usize>,
}

impl CadView {
    fn new(id: BlockId, spec: &CadModelSpec) -> Self {
        Self {
            id,
            spec: Rc::new(spec.clone()),
            model: Rc::new(spec.model()),
            hidden: BTreeSet::new(),
        }
    }

    fn sync(&mut self, spec: &CadModelSpec) {
        if *self.spec != *spec {
            *self = Self::new(self.id, spec);
        }
    }

    /// Hides solid `index` in the view, or shows it again.
    pub fn toggle(&mut self, index: usize, cx: &mut Context<Self>) {
        if !self.hidden.remove(&index) {
            self.hidden.insert(index);
        }
        cx.notify();
    }

    /// The hidden solids.
    pub fn hidden(&self) -> &BTreeSet<usize> {
        &self.hidden
    }

    /// The triangle ranges the viewer leaves out.
    fn hidden_ranges(&self) -> Vec<std::ops::Range<usize>> {
        self.hidden
            .iter()
            .filter_map(|&i| {
                self.spec
                    .solids
                    .get(i)
                    .map(domain::engineering::cad::CadSolid::range)
            })
            .collect()
    }

    fn solid_rows(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let (muted, warning, border) = (theme.muted_foreground, theme.warning, theme.border);
        let unit = self.spec.unit_label().to_owned();
        let rows: Vec<AnyElement> = self
            .spec
            .solids
            .iter()
            .enumerate()
            .take(MAX_ROWS)
            .map(|(i, s)| {
                let shown = !self.hidden.contains(&i);
                let mut detail = format!(
                    "{} faces · {} triangles · {}",
                    s.faces,
                    s.triangles,
                    size_text(s.size(), &unit)
                );
                if let Some(p) = &s.product {
                    detail = format!("{p} · {detail}");
                }
                h_flex()
                    .id(("cad-solid", i))
                    .debug_selector(move || format!("cad-solid-{i}"))
                    .gap_2()
                    .px_1()
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.toggle(i, cx);
                    }))
                    .child(div().w_4().child(if shown { "●" } else { "○" }))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .when(!shown, |d| d.text_color(muted))
                            .child(SharedString::from(s.name.clone())),
                    )
                    .child(div().text_color(muted).child(detail))
                    .when(s.failed_faces > 0, |row| {
                        row.child(
                            div()
                                .text_color(warning)
                                .child(format!("{} faces not meshed", s.failed_faces)),
                        )
                    })
                    .into_any_element()
            })
            .collect();
        let more = self.spec.solids.len().saturating_sub(MAX_ROWS);
        v_flex()
            .debug_selector(|| "cad-solids".to_owned())
            .text_xs()
            .border_t_1()
            .border_color(border)
            .pt_1()
            .children(rows)
            .when(more > 0, |d| {
                d.child(
                    div()
                        .text_color(muted)
                        .child(format!("… {more} more solids")),
                )
            })
    }
}

impl Render for CadView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let model = model_view_inline(node_id(self.id, 0), &self.model, VIEW_HEIGHT, window, cx);
        let ranges = self.hidden_ranges();
        model.update(cx, |view, cx| view.set_hidden(ranges, cx));
        let rows = self.solid_rows(cx);
        let theme = cx.theme();
        let (muted, warning) = (theme.muted_foreground, theme.warning);
        let warnings = &self.spec.warnings;
        v_flex()
            .w_full()
            .gap_1()
            .debug_selector(|| "cad".to_owned())
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(self.spec.display_title()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .debug_selector(|| "cad-summary".to_owned())
                    .child(summary_text(&self.spec)),
            )
            .when_some(failed_text(&self.spec), |d, text| {
                d.child(
                    div()
                        .text_xs()
                        .text_color(warning)
                        .debug_selector(|| "cad-failed".to_owned())
                        .child(text),
                )
            })
            .child(model)
            .child(rows)
            .when(!warnings.is_empty(), |d| {
                d.child(
                    v_flex()
                        .text_xs()
                        .text_color(muted)
                        .debug_selector(|| "cad-warnings".to_owned())
                        .children(warnings.iter().take(LISTED).map(|w| div().child(w.clone())))
                        .when(warnings.len() > LISTED, |d| {
                            d.child(format!("… {} more", warnings.len() - LISTED))
                        }),
                )
            })
    }
}

/// The CAD entity of block `id`, in sync with `spec`.
pub fn view(
    id: BlockId,
    spec: &CadModelSpec,
    window: &mut Window,
    cx: &mut App,
) -> gpui::Entity<CadView> {
    let view = super::block_state(id, window, cx, |_, _| CadView::new(id, spec));
    view.update(cx, |view, _| view.sync(spec));
    view
}

/// Draws the block held by `entry`.
pub fn render(entry: &TimelineEntry, window: &mut Window, cx: &mut App) -> AnyElement {
    let BlockKind::CadModel(spec) = &entry.block.kind else {
        return super::placeholder(entry, cx);
    };
    view(entry.block.id, spec, window, cx).into_any_element()
}

#[cfg(test)]
mod tests {
    use application::ports::events::DomainEvent;
    use domain::block::Block;
    use domain::engineering::cad::CadSolid;
    use domain::model3d::{ModelMesh, UpAxis};
    use gpui::{Modifiers, TestAppContext, point, px, size};

    use super::*;
    use crate::timeline::TimelineView as Timeline;

    fn spec() -> CadModelSpec {
        let solid = |name: &str, first: usize, failed: usize| CadSolid {
            name: name.into(),
            product: Some("Bracket".into()),
            faces: 4,
            failed_faces: failed,
            first_triangle: first,
            triangles: 2,
            min: [0.0; 3],
            max: [10.0, 10.0, 10.0],
        };
        CadModelSpec {
            title: None,
            path: "cad/bracket.step".into(),
            schema: Some("AUTOMOTIVE_DESIGN".into()),
            application: Some("FreeCAD".into()),
            unit: Some("mm".into()),
            products: vec!["Bracket".into()],
            solids: vec![solid("Plate", 0, 0), solid("Boss", 2, 1)],
            mesh: ModelMesh {
                positions: vec![
                    [0.0, 0.0, 0.0],
                    [10.0, 0.0, 0.0],
                    [0.0, 10.0, 0.0],
                    [0.0, 0.0, 10.0],
                ],
                triangles: vec![[0, 2, 1], [0, 1, 3], [0, 3, 2], [1, 2, 3]],
                up: UpAxis::Z,
            },
            tolerance: 0.01,
            warnings: vec!["assembly placements are not applied".into()],
        }
    }

    #[test]
    fn summary_and_failed_faces() {
        assert_eq!(
            summary_text(&spec()),
            "AP214 · FreeCAD · 10 × 10 × 10 mm · 2 solids · 8 faces · 4 triangles"
        );
        assert_eq!(
            failed_text(&spec()).as_deref(),
            Some("1 of 8 faces could not be meshed and are missing from the view")
        );
    }

    #[gpui::test]
    fn draws_the_part_and_hides_a_solid_on_click(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::sysboard::disable_gpu(cx);
        });
        let (view, cx) = cx.add_window_view(|_, cx| Timeline::new(cx));
        let block = Block::new(BlockKind::CadModel(Box::new(spec())));
        let id = block.id;
        view.update(cx, |view, cx| {
            view.apply_all([DomainEvent::BlockAdded { block }], cx);
        });
        cx.draw(point(px(0.), px(0.)), size(px(1000.), px(1200.)), |_, _| {
            div().size_full().child(view.clone())
        });
        cx.run_until_parked();
        for selector in [
            "cad",
            "cad-summary",
            "cad-failed",
            "cad-solids",
            "cad-solid-1",
            "cad-warnings",
            "model-viewport",
        ] {
            assert!(cx.debug_bounds(selector).is_some(), "{selector}");
        }
        let row = cx.debug_bounds("cad-solid-1").unwrap().center();
        cx.simulate_click(row, Modifiers::none());
        let cad = cx.update(|window, cx| super::view(id, &spec(), window, cx));
        assert!(cad.read_with(cx, |c, _| c.hidden().contains(&1)));
        let model = cx.update(|window, cx| {
            model_view_inline(
                node_id(id, 0),
                &cad.read(cx).model.clone(),
                VIEW_HEIGHT,
                window,
                cx,
            )
        });
        cx.run_until_parked();
        assert_eq!(
            model.read_with(cx, |m, _| m.mesh().indices.len()),
            2 * 3,
            "the second solid's triangles are left out"
        );
    }
}
