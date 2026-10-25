//! The power card: the four quantities with the one solved for marked,
//! and the power curve over n with the solution as a point.

use domain::computed::{Computed, PowerBody, PowerTarget, apa};
use domain::plot::{PlotRange, PlotSpec, SeriesStyle, XySeries};
use gpui::{AnyElement, App, IntoElement as _, ParentElement as _, Styled as _, Window};
use gpui_component::{ActiveTheme as _, v_flex};

use super::distribution::plots;
use super::grid::{Grid, GridCell};
use super::number;

/// The quantities table: given or solved, with the engine's display.
pub fn quantities(computed: &Computed, body: &PowerBody, cx: &App) -> Grid {
    let mut grid = Grid::new(["Quantity", "Value", "Role"]);
    let n_label = if body.test.n_per_group() {
        "n per group"
    } else {
        "n"
    };
    let effect_label = format!("Effect {}", body.test.effect_symbol());
    let rows = [
        (PowerTarget::N, n_label.to_owned(), "n"),
        (PowerTarget::Effect, effect_label, "effect"),
        (PowerTarget::Alpha, "α".to_owned(), "alpha"),
        (PowerTarget::Power, "Power 1 − β".to_owned(), "power"),
    ];
    let accent = cx.theme().info;
    for (target, label, key) in rows {
        let text = computed
            .value(key)
            .map(domain::computed::ComputedValue::display_text)
            .unwrap_or_default();
        let solved = target == body.solved_for;
        grid.rows.push(vec![
            GridCell::from(label),
            GridCell {
                text,
                bg: solved.then(|| accent.opacity(0.15)),
                fg: None,
            },
            GridCell::from(if solved { "solved" } else { "given" }),
        ]);
    }
    if let Some(required) = computed.value("n_required") {
        let total = computed
            .value("n_total")
            .map(|t| format!(" ({} in total)", t.display_text()))
            .unwrap_or_default();
        grid.push([
            "Recruit".to_owned(),
            format!("{}{total}", required.display_text()),
            "rounded up".to_owned(),
        ]);
    }
    grid
}

/// Power against n, the solution as a point and the power reached as a
/// level line.
pub fn curve_plot(body: &PowerBody) -> PlotSpec {
    let curve = &body.curve;
    let mut series = vec![XySeries {
        name: format!(
            "power at {} = {}, α = {}",
            body.test.effect_symbol(),
            number(body.effect, 2),
            apa::bounded(body.alpha, 3)
        ),
        x: curve.x.clone(),
        y: curve.y.clone(),
        style: SeriesStyle::Line,
    }];
    if let (Some(first), Some(last)) = (curve.x.first(), curve.x.last()) {
        series.push(XySeries {
            name: format!("power {}", apa::bounded(body.power, 2)),
            x: vec![*first, *last],
            y: vec![body.power, body.power],
            style: SeriesStyle::Line,
        });
    }
    series.push(XySeries {
        name: "this design".into(),
        x: vec![body.n],
        y: vec![body.power],
        style: SeriesStyle::Scatter,
    });
    PlotSpec {
        title: Some("Power curve".into()),
        y_range: Some(PlotRange { min: 0.0, max: 1.0 }),
        x_label: Some(if body.test.n_per_group() {
            "n per group".into()
        } else {
            "n".into()
        }),
        y_label: Some("Power".into()),
        series,
        ..PlotSpec::default()
    }
}

/// Draws `body`.
pub fn render(
    id: &str,
    computed: &Computed,
    body: &PowerBody,
    window: &Window,
    cx: &App,
) -> AnyElement {
    let mut column = v_flex()
        .gap_1()
        .child(quantities(computed, body, cx).render(format!("{id}-quantities"), cx));
    if !body.curve.x.is_empty() {
        column = column.child(plots(id, vec![("curve", curve_plot(body))], window, cx));
    }
    column.into_any_element()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use domain::computed::{Alternative, Curve, PowerTest};

    pub(crate) fn body() -> PowerBody {
        PowerBody {
            test: PowerTest::TwoSampleT,
            solved_for: PowerTarget::N,
            alternative: Alternative::TwoSided,
            n: 25.5,
            effect: 0.8,
            alpha: 0.05,
            power: 0.8,
            groups: None,
            curve: Curve {
                x: vec![6.0, 25.5, 76.5],
                y: vec![0.25, 0.8, 0.999],
            },
        }
    }

    #[test]
    fn the_curve_marks_the_solution() {
        let spec = curve_plot(&body());
        assert_eq!(spec.validate(), Ok(()));
        assert_eq!(spec.series[1].y, [0.8, 0.8]);
        assert_eq!(spec.series[2].x, [25.5]);
        assert_eq!(spec.series[2].style, SeriesStyle::Scatter);
    }
}
