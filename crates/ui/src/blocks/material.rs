//! Renderer of the `material` block kind (`show_material`): a material
//! data card.
//!
//! The name and designation come first with the counts of cited and
//! unverified values, then a table of the properties (symbol, value,
//! unit, condition) whose last column is the value's source, or
//! `unverified` in the warning color when it has none. Notes follow.
//! crest bundles no material data: every number is the call's.

use domain::block::BlockKind;
use domain::engineering::material::MaterialSpec;
use gpui::{
    AnyElement, App, FontWeight, InteractiveElement as _, IntoElement as _, ParentElement as _,
    SharedString, Styled as _, Window, div,
};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};

use super::computed::grid::{Grid, GridCell};
use crate::timeline::TimelineEntry;

/// Text of the source column for a value without one.
pub const UNVERIFIED: &str = "unverified";

/// The property table.
pub fn properties_grid(spec: &MaterialSpec, warning: gpui::Hsla) -> Grid {
    let mut grid = Grid::new(["Property", "Value", "Unit", "Condition", "Source"]);
    for p in &spec.properties {
        let name = match &p.symbol {
            Some(s) => format!("{} ({s})", p.name),
            None => p.name.clone(),
        };
        let source = if p.is_cited() {
            GridCell::from(p.source.clone().unwrap_or_default())
        } else {
            GridCell {
                text: UNVERIFIED.to_owned(),
                bg: Some(warning.opacity(0.12)),
                fg: Some(warning),
            }
        };
        grid.rows.push(vec![
            GridCell::from(name),
            GridCell::from(p.value_text()),
            GridCell::from(p.unit.clone()),
            GridCell::from(p.condition.clone().unwrap_or_default()),
            source,
        ]);
    }
    grid
}

/// The caption: counts of cited and unverified values.
#[must_use]
pub fn caption(spec: &MaterialSpec) -> String {
    let unverified = spec.unverified().len();
    let mut text = format!(
        "{} properties · {} with a source · {unverified} unverified",
        spec.properties.len(),
        spec.properties.len() - unverified
    );
    if let Some(c) = &spec.condition {
        text = format!("{c} · {text}");
    }
    text
}

/// Draws `spec`.
pub fn render_spec(spec: &MaterialSpec, cx: &App) -> AnyElement {
    let theme = cx.theme();
    v_flex()
        .w_full()
        .gap_1()
        .debug_selector(|| "material".to_owned())
        .child(
            h_flex()
                .gap_2()
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(SharedString::from(spec.full_name())),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .debug_selector(|| "material-caption".to_owned())
                        .child(SharedString::from(caption(spec))),
                ),
        )
        .child(properties_grid(spec, theme.warning).render("material-properties", cx))
        .children(spec.notes.iter().map(|n| {
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(SharedString::from(n.clone()))
        }))
        .child(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child("Values as given in the call; crest bundles no material data."),
        )
        .into_any_element()
}

/// Draws the block held by `entry`.
pub fn render(entry: &TimelineEntry, _window: &mut Window, cx: &mut App) -> AnyElement {
    let BlockKind::Material(spec) = &entry.block.kind else {
        return super::placeholder(entry, cx);
    };
    render_spec(spec, cx)
}

#[cfg(test)]
mod tests {
    use application::ports::events::DomainEvent;
    use domain::block::Block;
    use domain::engineering::material::MaterialProperty;
    use gpui::{TestAppContext, point, px, size};

    use super::*;
    use crate::timeline::TimelineView as Timeline;

    fn spec() -> MaterialSpec {
        MaterialSpec {
            name: "PEEK".into(),
            designation: None,
            condition: Some("unfilled".into()),
            properties: vec![
                MaterialProperty {
                    name: "density".into(),
                    symbol: Some("ρ".into()),
                    value: 1300.0,
                    unit: "kg/m³".into(),
                    condition: None,
                    source: Some("user".into()),
                },
                MaterialProperty {
                    name: "tensile strength".into(),
                    symbol: None,
                    value: 100.0,
                    unit: "MPa".into(),
                    condition: Some("23 °C".into()),
                    source: None,
                },
            ],
            notes: vec!["dry as moulded".into()],
        }
    }

    #[test]
    fn unverified_values_are_badged() {
        let grid = properties_grid(&spec(), gpui::red());
        assert_eq!(grid.rows[0][0].text, "density (ρ)");
        assert_eq!(grid.rows[0][4].text, "user");
        assert_eq!(grid.rows[1][4].text, UNVERIFIED);
        assert!(grid.rows[1][4].fg.is_some());
        assert_eq!(
            caption(&spec()),
            "unfilled · 2 properties · 1 with a source · 1 unverified"
        );
    }

    #[gpui::test]
    fn draws_the_card(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, cx| Timeline::new(cx));
        let block = Block::new(BlockKind::Material(Box::new(spec())));
        view.update(cx, |view, cx| {
            view.apply_all([DomainEvent::BlockAdded { block }], cx);
        });
        cx.draw(point(px(0.), px(0.)), size(px(800.), px(600.)), |_, _| {
            div().size_full().child(view.clone())
        });
        cx.run_until_parked();
        for selector in ["material", "material-caption"] {
            assert!(cx.debug_bounds(selector).is_some(), "{selector}");
        }
    }
}
