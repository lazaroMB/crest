//! The A/B test card: each arm's visitors, conversions and rate with its
//! Wilson interval, the lift and the checks, plus the two rates with
//! their intervals on one axis.

use domain::computed::{AbTestBody, Computed, apa};
use domain::plot::{PlotRange, PlotSpec, SeriesStyle, XySeries};
use gpui::{AnyElement, App, IntoElement as _, ParentElement as _, Styled as _, Window};
use gpui_component::{ActiveTheme as _, v_flex};

use super::distribution::plots;
use super::grid::{Grid, GridCell};

fn pct(x: f64) -> String {
    format!("{}%", apa::number(x * 100.0, 2))
}

/// One row per arm.
pub fn arms_grid(body: &AbTestBody) -> Grid {
    let level = apa::number(body.conf_level * 100.0, 0);
    let mut grid = Grid::new([
        "Arm".to_owned(),
        "Visitors".to_owned(),
        "Conversions".to_owned(),
        "Rate".to_owned(),
        format!("{level}% CI lo"),
        format!("{level}% CI hi"),
    ]);
    for arm in &body.arms {
        grid.push([
            arm.name.clone(),
            arm.visitors.to_string(),
            arm.conversions.to_string(),
            pct(arm.rate),
            pct(arm.rate_lo),
            pct(arm.rate_hi),
        ]);
    }
    grid
}

/// The comparison: lifts with intervals, test, P(B > A), MDE and the
/// sample-ratio check, as the engine displays them.
pub fn results_grid(computed: &Computed, body: &AbTestBody, cx: &App) -> Grid {
    let mut grid = Grid::new(["Result", "Value", "CI low", "CI high"]);
    let interval = |key: &str, scale: f64, unit: &str| {
        computed
            .value(key)
            .and_then(|v| v.ci.as_ref())
            .map(|ci| {
                (
                    format!("{}{unit}", apa::number(ci.lo * scale, 2)),
                    format!("{}{unit}", apa::number(ci.hi * scale, 2)),
                )
            })
            .unwrap_or_default()
    };
    let shown = |key: &str| {
        computed
            .value(key)
            .map(domain::computed::ComputedValue::display_text)
    };
    let names = (
        body.arms.first().map_or("A", |a| a.name.as_str()),
        body.arms.get(1).map_or("B", |a| a.name.as_str()),
    );
    let mut row = |label: String, key: &str, ci: (String, String)| {
        if let Some(text) = shown(key) {
            grid.push([label, text, ci.0, ci.1]);
        }
    };
    row(
        "Absolute lift".into(),
        "abs_lift",
        interval("abs_lift", 100.0, " pp"),
    );
    row(
        "Relative lift".into(),
        "rel_lift",
        interval("rel_lift", 100.0, "%"),
    );
    row("z".into(), "statistic", Default::default());
    row(
        format!("p ({})", body.method.label()),
        "p_value",
        Default::default(),
    );
    row(
        format!("P({} > {})", names.1, names.0),
        "prob_b_better",
        Default::default(),
    );
    row(
        "Detectable lift (80% power)".into(),
        "mde",
        Default::default(),
    );
    row(
        "Power for the minimum lift".into(),
        "power",
        Default::default(),
    );
    if let (Some(chi2), Some(p)) = (shown("srm_chi2"), shown("srm_p")) {
        let danger = cx.theme().danger;
        grid.rows.push(vec![
            GridCell::from("Sample ratio χ² (p)"),
            GridCell {
                text: format!("{chi2} ({p})"),
                bg: body.sample_ratio_mismatch.then(|| danger.opacity(0.15)),
                fg: body.sample_ratio_mismatch.then_some(danger),
            },
            GridCell::default(),
            GridCell::default(),
        ]);
    }
    grid
}

/// Each arm's rate as a point with its interval as a vertical bar.
pub fn rates_plot(body: &AbTestBody) -> PlotSpec {
    let mut series = Vec::new();
    for (ix, arm) in body.arms.iter().enumerate() {
        #[allow(clippy::cast_precision_loss)]
        let at = (ix + 1) as f64;
        series.push(XySeries {
            name: format!("{} interval", arm.name),
            x: vec![at, at],
            y: vec![arm.rate_lo * 100.0, arm.rate_hi * 100.0],
            style: SeriesStyle::Line,
        });
        series.push(XySeries {
            name: arm.name.clone(),
            x: vec![at],
            y: vec![arm.rate * 100.0],
            style: SeriesStyle::Scatter,
        });
    }
    let names: Vec<String> = body
        .arms
        .iter()
        .enumerate()
        .map(|(i, a)| format!("{} = {}", i + 1, a.name))
        .collect();
    PlotSpec {
        title: Some("Conversion rates".into()),
        x_range: Some(PlotRange { min: 0.5, max: 2.5 }),
        x_label: Some(format!("arm ({})", names.join(", "))),
        y_label: Some("Rate (%)".into()),
        series,
        ..PlotSpec::default()
    }
}

/// Draws `body`.
pub fn render(
    id: &str,
    computed: &Computed,
    body: &AbTestBody,
    window: &Window,
    cx: &App,
) -> AnyElement {
    v_flex()
        .gap_1()
        .child(arms_grid(body).render(format!("{id}-arms"), cx))
        .child(results_grid(computed, body, cx).render(format!("{id}-results"), cx))
        .child(plots(id, vec![("rates", rates_plot(body))], window, cx))
        .into_any_element()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use domain::computed::{AbArm, AbMethod};

    pub(crate) fn body() -> AbTestBody {
        let arm = |name: &str, conversions, rate: f64| AbArm {
            name: name.into(),
            visitors: 1000,
            conversions,
            rate,
            rate_lo: rate - 0.02,
            rate_hi: rate + 0.02,
        };
        AbTestBody {
            arms: vec![arm("A", 100, 0.1), arm("B", 120, 0.12)],
            expected_split: 0.5,
            conf_level: 0.95,
            method: AbMethod::ZTest,
            looks: 1,
            sample_ratio_mismatch: true,
        }
    }

    #[test]
    fn arms_and_rates_come_from_the_body() {
        let b = body();
        let grid = arms_grid(&b);
        assert_eq!(grid.rows[1][3].text, "12.00%");
        assert_eq!(grid.rows[0][4].text, "8.00%");
        let plot = rates_plot(&b);
        assert_eq!(plot.validate(), Ok(()));
        assert!((plot.series[2].y[1] - 14.0).abs() < 1e-9);
    }
}
