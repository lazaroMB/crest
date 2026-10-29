//! Renderer of the `ifc_schedule` block kind (`show_ifc_schedule`): an
//! IFC model's room schedule.
//!
//! A summary line comes first (schema, project, units, storey and space
//! counts, wall/door/window totals), then the spaces without an area in
//! the file in the warning color. The schedule is a tree table
//! ([`crate::widgets::tree`]): storeys with their elevation, the sum of
//! the areas the file gives and their element counts, each space under
//! its storey with its long name, area and the quantity or property it
//! came from ("not given" otherwise). Reader notes come last.

use std::rc::Rc;

use domain::block::{BlockId, BlockKind};
use domain::engineering::ifc::IfcScheduleSpec;
use domain::widget::tree::TreeSpec;
use gpui::{
    AnyElement, App, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, Styled as _, Window, div, prelude::FluentBuilder as _,
};
use gpui_component::{ActiveTheme as _, v_flex};

use super::custom::node_id;
use crate::timeline::TimelineEntry;
use crate::widgets;

/// Notes listed.
const LISTED: usize = 6;

/// The summary text.
#[must_use]
pub fn summary_text(spec: &IfcScheduleSpec) -> String {
    let mut parts = vec![spec.schema.clone()];
    if let Some(p) = &spec.project {
        parts.push(p.clone());
    }
    let units: Vec<&str> = [spec.length_unit.as_deref(), spec.area_unit.as_deref()]
        .into_iter()
        .flatten()
        .collect();
    if !units.is_empty() {
        parts.push(units.join(", "));
    }
    let totals = spec.element_totals();
    parts.push(format!("{} storeys", spec.storeys.len()));
    parts.push(format!("{} spaces", spec.space_count()));
    parts.push(format!(
        "{} walls · {} doors · {} windows",
        totals.walls, totals.doors, totals.windows
    ));
    parts.join(" · ")
}

/// The missing-areas line, if any space has none.
#[must_use]
pub fn missing_text(spec: &IfcScheduleSpec) -> Option<String> {
    let missing = spec.missing_areas();
    (missing > 0).then(|| {
        format!(
            "{missing} of {} spaces have no area in the file (not computed from geometry)",
            spec.space_count()
        )
    })
}

/// Per-block state: the schedule and its tree.
pub struct IfcView {
    id: BlockId,
    spec: Rc<IfcScheduleSpec>,
    tree: Rc<TreeSpec>,
}

impl IfcView {
    fn new(id: BlockId, spec: &IfcScheduleSpec) -> Self {
        Self {
            id,
            spec: Rc::new(spec.clone()),
            tree: Rc::new(spec.to_tree()),
        }
    }

    fn sync(&mut self, spec: &IfcScheduleSpec) {
        if *self.spec != *spec {
            *self = Self::new(self.id, spec);
        }
    }

    /// The schedule tree.
    pub fn tree(&self) -> &TreeSpec {
        &self.tree
    }
}

impl Render for IfcView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tree = self.tree.clone();
        let table = widgets::tree::render(node_id(self.id, 0), &tree, window, cx);
        let theme = cx.theme();
        let (muted, warning) = (theme.muted_foreground, theme.warning);
        let warnings = &self.spec.warnings;
        v_flex()
            .w_full()
            .gap_1()
            .debug_selector(|| "ifc".to_owned())
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
                    .debug_selector(|| "ifc-summary".to_owned())
                    .child(summary_text(&self.spec)),
            )
            .when_some(missing_text(&self.spec), |d, text| {
                d.child(
                    div()
                        .text_xs()
                        .text_color(warning)
                        .debug_selector(|| "ifc-missing".to_owned())
                        .child(text),
                )
            })
            .child(table)
            .when(!warnings.is_empty(), |d| {
                d.child(
                    v_flex()
                        .text_xs()
                        .text_color(muted)
                        .debug_selector(|| "ifc-warnings".to_owned())
                        .children(warnings.iter().take(LISTED).map(|w| div().child(w.clone()))),
                )
            })
    }
}

/// The schedule entity of block `id`, in sync with `spec`.
pub fn view(
    id: BlockId,
    spec: &IfcScheduleSpec,
    window: &mut Window,
    cx: &mut App,
) -> gpui::Entity<IfcView> {
    let view = super::block_state(id, window, cx, |_, _| IfcView::new(id, spec));
    view.update(cx, |view, _| view.sync(spec));
    view
}

/// Draws the block held by `entry`.
pub fn render(entry: &TimelineEntry, window: &mut Window, cx: &mut App) -> AnyElement {
    let BlockKind::IfcSchedule(spec) = &entry.block.kind else {
        return super::placeholder(entry, cx);
    };
    view(entry.block.id, spec, window, cx).into_any_element()
}

#[cfg(test)]
mod tests {
    use application::ports::events::DomainEvent;
    use domain::block::Block;
    use domain::engineering::ifc::{ElementCounts, IfcSpace, IfcStorey};
    use gpui::{TestAppContext, point, px, size};

    use super::*;
    use crate::timeline::TimelineView as Timeline;

    fn spec() -> IfcScheduleSpec {
        IfcScheduleSpec {
            title: None,
            path: "bim/house.ifc".into(),
            schema: "IFC4".into(),
            project: Some("House".into()),
            site: None,
            buildings: vec!["A".into()],
            length_unit: Some("mm".into()),
            area_unit: Some("m²".into()),
            storeys: vec![IfcStorey {
                name: "Ground floor".into(),
                elevation: Some(0.0),
                building: Some("A".into()),
                spaces: vec![
                    IfcSpace {
                        name: Some("101".into()),
                        long_name: Some("Living".into()),
                        area: Some(24.5),
                        area_source: Some("Qto_SpaceBaseQuantities.NetFloorArea".into()),
                        global_id: None,
                    },
                    IfcSpace {
                        name: Some("102".into()),
                        long_name: None,
                        area: None,
                        area_source: None,
                        global_id: None,
                    },
                ],
                elements: ElementCounts {
                    walls: 3,
                    doors: 2,
                    windows: 1,
                },
            }],
            unplaced_spaces: Vec::new(),
            unplaced: ElementCounts::default(),
            warnings: vec!["a note".into()],
        }
    }

    #[test]
    fn summary_and_missing_areas() {
        assert_eq!(
            summary_text(&spec()),
            "IFC4 · House · mm, m² · 1 storeys · 2 spaces · 3 walls · 2 doors · 1 windows"
        );
        assert_eq!(
            missing_text(&spec()).as_deref(),
            Some("1 of 2 spaces have no area in the file (not computed from geometry)")
        );
    }

    #[gpui::test]
    fn draws_the_schedule_tree(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, cx| Timeline::new(cx));
        let block = Block::new(BlockKind::IfcSchedule(Box::new(spec())));
        let id = block.id;
        view.update(cx, |view, cx| {
            view.apply_all([DomainEvent::BlockAdded { block }], cx);
        });
        cx.draw(point(px(0.), px(0.)), size(px(900.), px(900.)), |_, _| {
            div().size_full().child(view.clone())
        });
        cx.run_until_parked();
        for selector in ["ifc", "ifc-summary", "ifc-missing", "ifc-warnings", "tree"] {
            assert!(cx.debug_bounds(selector).is_some(), "{selector}");
        }
        let ifc = cx.update(|window, cx| super::view(id, &spec(), window, cx));
        assert_eq!(
            ifc.read_with(cx, |v, _| v.tree().nodes[0].children.len()),
            2
        );
    }
}
