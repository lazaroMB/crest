//! The beam card: the reactions per support, the extremes (largest
//! sagging and hogging moment, shear and deflection with where they
//! occur), the loads as given, and the shear, bending-moment and
//! deflection diagrams along the span, the supports marked on each.

use domain::computed::{BeamBody, BeamLoadKind, Computed, SupportKind};
use domain::plot::{PlotSpec, SeriesStyle, XySeries};
use gpui::{
    AnyElement, App, IntoElement as _, ParentElement as _, SharedString, Styled as _, Window, div,
};
use gpui_component::{ActiveTheme as _, v_flex};

use super::distribution::plots;
use super::grid::Grid;

fn kind_name(kind: SupportKind) -> &'static str {
    match kind {
        SupportKind::Pin => "pin",
        SupportKind::Roller => "roller",
        SupportKind::Fixed => "fixed",
    }
}

/// One row per support: position, kind, force and moment.
pub fn reactions_grid(computed: &Computed, body: &BeamBody) -> Grid {
    let (lu, fu, mu) = (
        body.length_unit.symbol(),
        body.force_unit.symbol(),
        body.moment_unit(),
    );
    let mut grid = Grid::new([
        "Support".to_owned(),
        format!("x ({lu})"),
        "Kind".to_owned(),
        format!("R ({fu}, up)"),
        format!("M ({mu}, ccw)"),
    ]);
    for r in &body.reactions {
        let shown = |key: String| {
            computed.value(&key).map_or_else(
                || "—".to_owned(),
                domain::computed::ComputedValue::display_text,
            )
        };
        grid.push([
            r.name.clone(),
            trim(r.at),
            kind_name(r.kind).to_owned(),
            shown(format!("reaction.{}", r.name)),
            shown(format!("moment.{}", r.name)),
        ]);
    }
    grid
}

fn trim(x: f64) -> String {
    let text = format!("{x:.4}");
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// The extremes with their positions.
pub fn extremes_grid(computed: &Computed, body: &BeamBody) -> Grid {
    let lu = body.length_unit.symbol();
    let mut grid = Grid::new([
        "Result".to_owned(),
        "Value".to_owned(),
        format!("at x ({lu})"),
    ]);
    for (label, key) in [
        ("Largest sagging moment", "max_moment"),
        ("Largest hogging moment", "min_moment"),
        ("Largest shear", "max_shear"),
        ("Largest deflection", "max_deflection"),
    ] {
        if let Some(v) = computed.value(key) {
            let at = computed
                .value(&format!("{key}_at"))
                .map(domain::computed::ComputedValue::display_text)
                .unwrap_or_default();
            grid.push([label.to_owned(), v.display_text(), at]);
        }
    }
    if let Some(v) = computed.value("ei") {
        grid.push(["E·I".to_owned(), v.display_text(), String::new()]);
    }
    grid
}

/// The loads as given.
pub fn loads_grid(body: &BeamBody) -> Grid {
    let (lu, fu) = (body.length_unit.symbol(), body.force_unit.symbol());
    let mut grid = Grid::new(["Load", "From", "To", "Value"]);
    for load in &body.loads {
        let (kind, unit) = match load.kind {
            BeamLoadKind::Point => ("point (down)", fu.to_owned()),
            BeamLoadKind::Distributed => ("distributed (down)", format!("{fu}/{lu}")),
            BeamLoadKind::Moment => ("moment (ccw)", body.moment_unit()),
        };
        let value = match (load.kind, load.end_value) {
            (BeamLoadKind::Distributed, Some(end)) if end != load.value => {
                format!("{} → {} {unit}", trim(load.value), trim(end))
            }
            _ => format!("{} {unit}", trim(load.value)),
        };
        let to = match load.kind {
            BeamLoadKind::Distributed => trim(load.end.unwrap_or(body.length)),
            _ => String::new(),
        };
        grid.push([kind.to_owned(), trim(load.at), to, value]);
    }
    grid
}

/// Shear, moment and (with E·I) deflection against x.
pub fn diagrams(body: &BeamBody) -> Vec<(&'static str, PlotSpec)> {
    let lu = body.length_unit.symbol();
    let supports = |y: f64| XySeries {
        name: "supports".into(),
        x: body.supports.iter().map(|s| s.at).collect(),
        y: vec![y; body.supports.len()],
        style: SeriesStyle::Scatter,
    };
    let line = |name: &str, y: &[f64]| XySeries {
        name: name.to_owned(),
        x: body.x.clone(),
        y: y.to_vec(),
        style: SeriesStyle::Line,
    };
    let axis = XySeries {
        name: "beam".into(),
        x: vec![0.0, body.length],
        y: vec![0.0, 0.0],
        style: SeriesStyle::Line,
    };
    let mut out = vec![
        (
            "shear",
            PlotSpec {
                title: Some("Shear force".into()),
                x_label: Some(format!("x ({lu})")),
                y_label: Some(format!("V ({})", body.force_unit.symbol())),
                series: vec![line("V", &body.shear), axis.clone(), supports(0.0)],
                ..PlotSpec::default()
            },
        ),
        (
            "moment",
            PlotSpec {
                title: Some("Bending moment (sagging +)".into()),
                x_label: Some(format!("x ({lu})")),
                y_label: Some(format!("M ({})", body.moment_unit())),
                series: vec![line("M", &body.moment), axis.clone(), supports(0.0)],
                ..PlotSpec::default()
            },
        ),
    ];
    if !body.deflection.is_empty() {
        out.push((
            "deflection",
            PlotSpec {
                title: Some("Deflection (up +)".into()),
                x_label: Some(format!("x ({lu})")),
                y_label: Some(format!(
                    "w ({})",
                    body.length_unit.deflection_unit().symbol()
                )),
                series: vec![line("w", &body.deflection), axis, supports(0.0)],
                ..PlotSpec::default()
            },
        ));
    }
    out
}

/// Draws `body`.
pub fn render(
    id: &str,
    computed: &Computed,
    body: &BeamBody,
    window: &Window,
    cx: &App,
) -> AnyElement {
    let note = if body.indeterminacy == 0 {
        format!(
            "Span {} {} · statically determinate",
            trim(body.length),
            body.length_unit.symbol()
        )
    } else {
        format!(
            "Span {} {} · statically indeterminate (degree {}), uniform E·I assumed",
            trim(body.length),
            body.length_unit.symbol(),
            body.indeterminacy
        )
    };
    v_flex()
        .gap_1()
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(SharedString::from(note)),
        )
        .child(reactions_grid(computed, body).render(format!("{id}-reactions"), cx))
        .child(extremes_grid(computed, body).render(format!("{id}-extremes"), cx))
        .child(loads_grid(body).render(format!("{id}-loads"), cx))
        .child(plots(id, diagrams(body), window, cx))
        .into_any_element()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use domain::computed::{BeamLoad, BeamReaction, BeamSupport, ForceUnit, LengthUnit};

    pub(crate) fn body() -> BeamBody {
        BeamBody {
            length: 6.0,
            length_unit: LengthUnit::M,
            force_unit: ForceUnit::KN,
            supports: vec![
                BeamSupport {
                    at: 0.0,
                    kind: SupportKind::Pin,
                },
                BeamSupport {
                    at: 6.0,
                    kind: SupportKind::Roller,
                },
            ],
            loads: vec![BeamLoad {
                kind: BeamLoadKind::Distributed,
                at: 0.0,
                end: Some(6.0),
                value: 10.0,
                end_value: None,
            }],
            reactions: vec![
                BeamReaction {
                    name: "A".into(),
                    at: 0.0,
                    kind: SupportKind::Pin,
                    force: 30.0,
                    moment: None,
                },
                BeamReaction {
                    name: "B".into(),
                    at: 6.0,
                    kind: SupportKind::Roller,
                    force: 30.0,
                    moment: None,
                },
            ],
            x: vec![0.0, 0.0, 3.0, 6.0, 6.0],
            shear: vec![0.0, 30.0, 0.0, -30.0, 0.0],
            moment: vec![0.0, 0.0, 45.0, 0.0, 0.0],
            deflection: vec![0.0, 0.0, -5.27, 0.0, 0.0],
            ei: Some(16e6),
            indeterminacy: 0,
        }
    }

    #[test]
    fn diagrams_and_tables_follow_the_units() {
        let b = body();
        let specs = diagrams(&b);
        assert_eq!(specs.len(), 3);
        assert_eq!(specs[1].1.y_label.as_deref(), Some("M (kN·m)"));
        assert_eq!(specs[2].1.y_label.as_deref(), Some("w (mm)"));
        let loads = loads_grid(&b);
        assert_eq!(loads.rows[0][3].text, "10 kN/m");
        assert_eq!(loads.rows[0][2].text, "6");
    }
}
