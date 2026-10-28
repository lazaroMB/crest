//! The forecast card: the series with the forecast and its 80% and 95%
//! intervals (and the hold-out forecast where one ran), then the
//! decomposition as stacked panels: trend, each seasonal component and the
//! remainder. Every coordinate comes from the engine.

use domain::computed::{Computed, ForecastBody};
use domain::plot::{PlotSpec, SeriesStyle, XySeries};
use gpui::{
    AnyElement, App, IntoElement as _, ParentElement as _, SharedString, Styled as _, Window, div,
};
use gpui_component::{ActiveTheme as _, v_flex};

use super::distribution::plots;
use super::grid::Grid;

fn line(name: &str, x: Vec<f64>, y: &[f64]) -> XySeries {
    XySeries {
        name: name.to_owned(),
        x,
        y: y.to_vec(),
        style: SeriesStyle::Line,
    }
}

fn index(from: usize, len: usize) -> Vec<f64> {
    #[allow(clippy::cast_precision_loss)]
    (from..from + len).map(|i| i as f64).collect()
}

/// The series, the forecast and its fan.
pub fn forecast_plot(body: &ForecastBody) -> PlotSpec {
    let n = body.values.len();
    let h = body.forecast.len();
    let ahead = index(n, h);
    let mut series = vec![line(&body.variable, index(0, n), &body.values)];
    if let Some(holdout) = &body.holdout {
        series.push(line(
            "hold-out forecast",
            index(holdout.start, holdout.predicted.len()),
            &holdout.predicted,
        ));
    }
    series.push(line("forecast", ahead.clone(), &body.forecast));
    for (name, values) in [
        ("80% low", &body.lo80),
        ("80% high", &body.hi80),
        ("95% low", &body.lo95),
        ("95% high", &body.hi95),
    ] {
        series.push(line(name, ahead.clone(), values));
    }
    PlotSpec {
        title: Some(format!("Forecast · {}", body.model)),
        x_label: Some("t".into()),
        y_label: Some(body.variable.clone()),
        series,
        ..PlotSpec::default()
    }
}

/// One panel per component of the decomposition.
pub fn component_plots(body: &ForecastBody) -> Vec<(String, PlotSpec)> {
    let n = body.values.len();
    let panel = |title: String, values: &[f64]| PlotSpec {
        title: Some(title.clone()),
        x_label: Some("t".into()),
        series: vec![line(&title, index(0, n), values)],
        ..PlotSpec::default()
    };
    let mut out = Vec::new();
    if !body.trend.is_empty() {
        out.push(("trend".to_owned(), panel("Trend".into(), &body.trend)));
    }
    for s in &body.seasonal {
        out.push((
            format!("seasonal-{}", s.period),
            panel(format!("Seasonal (period {})", s.period), &s.values),
        ));
    }
    if !body.remainder.is_empty() {
        out.push((
            "remainder".to_owned(),
            panel("Remainder".into(), &body.remainder),
        ));
    }
    out
}

/// The headline numbers the engine gave.
pub fn summary_grid(computed: &Computed) -> Grid {
    let mut grid = Grid::new(["Result", "Value", "95% low", "95% high"]);
    for (key, label) in [
        ("next", "Next step"),
        ("end", "Last step"),
        ("mape", "Hold-out MAPE (%)"),
        ("smape", "Hold-out sMAPE (%)"),
        ("mae", "Hold-out MAE"),
        ("trend_strength", "Trend strength"),
    ] {
        let Some(value) = computed.value(key) else {
            continue;
        };
        let (lo, hi) = value
            .ci
            .as_ref()
            .map(|ci| {
                let d = value.decimals.unwrap_or(2);
                (
                    domain::computed::apa::number(ci.lo, d),
                    domain::computed::apa::number(ci.hi, d),
                )
            })
            .unwrap_or_default();
        grid.push([label.to_owned(), value.display_text(), lo, hi]);
    }
    for (key, value) in &computed.values {
        if let Some(period) = key.strip_prefix("seasonal_strength_") {
            grid.push([
                format!("Seasonal strength ({period})"),
                value.display_text(),
                String::new(),
                String::new(),
            ]);
        }
    }
    grid
}

/// Draws `body`.
pub fn render(
    id: &str,
    computed: &Computed,
    body: &ForecastBody,
    window: &Window,
    cx: &App,
) -> AnyElement {
    let mut column = v_flex()
        .gap_1()
        .child(summary_grid(computed).render(format!("{id}-summary"), cx));
    if let (Some(first), Some(last)) = (body.time.first(), body.time.last()) {
        column = column.child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(SharedString::from(format!(
                    "t = 0 is {first}; t = {} is {last}; the forecast starts at t = {}",
                    body.values.len().saturating_sub(1),
                    body.values.len()
                ))),
        );
    }
    let mut specs = vec![("forecast".to_owned(), forecast_plot(body))];
    specs.extend(component_plots(body));
    let named: Vec<(&str, PlotSpec)> = specs.iter().map(|(n, s)| (n.as_str(), s.clone())).collect();
    column
        .child(plots(id, named, window, cx))
        .into_any_element()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use domain::computed::{Holdout, SeasonalComponent};

    pub(crate) fn body() -> ForecastBody {
        ForecastBody {
            variable: "sales".into(),
            time: vec![
                "2024-01".into(),
                "2024-02".into(),
                "2024-03".into(),
                "2024-04".into(),
            ],
            model: "MSTL (periods 2) + AutoETS trend".into(),
            values: vec![1.0, 3.0, 2.0, 4.0],
            trend: vec![1.5, 2.0, 2.5, 3.0],
            seasonal: vec![SeasonalComponent {
                period: 2,
                values: vec![-0.5, 1.0, -0.5, 1.0],
            }],
            remainder: vec![0.0, 0.0, 0.0, 0.0],
            forecast: vec![3.0, 5.0],
            lo80: vec![2.5, 4.0],
            hi80: vec![3.5, 6.0],
            lo95: vec![2.0, 3.5],
            hi95: vec![4.0, 6.5],
            holdout: Some(Holdout {
                start: 3,
                predicted: vec![3.8],
            }),
        }
    }

    #[test]
    fn the_fan_continues_the_series() {
        let spec = forecast_plot(&body());
        assert_eq!(spec.validate(), Ok(()));
        assert_eq!(spec.series[1].x, [3.0]);
        assert_eq!(spec.series[2].x, [4.0, 5.0]);
        assert_eq!(spec.series.len(), 7);
        let panels = component_plots(&body());
        let names: Vec<&str> = panels.iter().map(|p| p.0.as_str()).collect();
        assert_eq!(names, ["trend", "seasonal-2", "remainder"]);
    }
}
