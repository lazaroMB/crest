//! The Monte Carlo card: the inputs and their percentiles (with sources
//! for a Fermi estimate), the output's summary, its histogram, the
//! convergence of the running mean with ±2 MCSE, and the tornado of
//! one-at-a-time swings around the base case.

use domain::computed::{Computed, SimulationBody, apa};
use domain::plot::{PlotRange, PlotSpec, SeriesStyle, XySeries};
use gpui::{AnyElement, App, IntoElement as _, ParentElement as _, Styled as _, Window};
use gpui_component::v_flex;

use super::distribution::plots;
use super::grid::Grid;

fn sig(x: f64) -> String {
    apa::significant(x, 4)
}

/// The inputs table.
pub fn inputs_grid(body: &SimulationBody) -> Grid {
    let mut grid = Grid::new(["Input", "Distribution", "P10", "P50", "P90", "Source"]);
    for i in &body.inputs {
        let name = match &i.unit {
            Some(u) => format!("{} ({u})", i.name),
            None => i.name.clone(),
        };
        grid.push([
            name,
            i.distribution.clone(),
            sig(i.p10),
            sig(i.p50),
            sig(i.p90),
            i.source.clone().unwrap_or_else(|| "—".into()),
        ]);
    }
    grid
}

/// The output's summary, as the engine displays it.
pub fn results_grid(computed: &Computed) -> Grid {
    let mut grid = Grid::new(["Output", "Simulated", "Exact (lognormal)"]);
    let shown = |key: &str| {
        computed
            .value(key)
            .map(domain::computed::ComputedValue::display_text)
            .unwrap_or_default()
    };
    for (label, key, exact) in [
        ("Mean", "mean", "analytic_mean"),
        ("MCSE of the mean", "mcse", ""),
        ("SD", "sd", ""),
        ("P5", "p5", "analytic_p5"),
        ("Median", "p50", "analytic_p50"),
        ("P95", "p95", "analytic_p95"),
        ("Base case (medians)", "base", ""),
    ] {
        if computed.value(key).is_some() {
            grid.push([label.to_owned(), shown(key), shown(exact)]);
        }
    }
    grid
}

/// The output's histogram.
pub fn histogram_plot(body: &SimulationBody) -> PlotSpec {
    let h = &body.histogram;
    let (mut x, mut y) = (vec![h.edges[0]], vec![0.0]);
    for (i, &c) in h.counts.iter().enumerate() {
        #[allow(clippy::cast_precision_loss)]
        let c = c as f64;
        x.extend([h.edges[i], h.edges[i + 1]]);
        y.extend([c, c]);
    }
    x.push(h.edges[h.edges.len() - 1]);
    y.push(0.0);
    PlotSpec {
        title: Some(format!(
            "Distribution of {} ({} draws)",
            body.output, body.samples
        )),
        x_label: Some(body.output.clone()),
        y_label: Some("Draws".into()),
        series: vec![XySeries {
            name: body.output.clone(),
            x,
            y,
            style: SeriesStyle::Line,
        }],
        ..PlotSpec::default()
    }
}

/// The running mean with a ±2 MCSE band, on a log axis of draws.
pub fn convergence_plot(body: &SimulationBody) -> Option<PlotSpec> {
    let c = &body.convergence;
    if c.x.len() < 2 {
        return None;
    }
    let band = |sign: f64| -> Vec<f64> {
        c.y.iter()
            .zip(&body.convergence_mcse)
            .map(|(m, s)| m + sign * 2.0 * s)
            .collect()
    };
    let mut series = vec![XySeries {
        name: "running mean".into(),
        x: c.x.clone(),
        y: c.y.clone(),
        style: SeriesStyle::Line,
    }];
    if body.convergence_mcse.len() == c.x.len() {
        series.push(XySeries {
            name: "−2 MCSE".into(),
            x: c.x.clone(),
            y: band(-1.0),
            style: SeriesStyle::Line,
        });
        series.push(XySeries {
            name: "+2 MCSE".into(),
            x: c.x.clone(),
            y: band(1.0),
            style: SeriesStyle::Line,
        });
    }
    Some(PlotSpec {
        title: Some("Convergence of the mean".into()),
        x_label: Some("draws".into()),
        y_label: Some(format!("mean of {}", body.output)),
        x_scale: domain::plot::AxisScale::Log,
        series,
        ..PlotSpec::default()
    })
}

/// The tornado: one horizontal bar per input, from the output at its P10
/// to the output at its P90, largest swing on top, and the base case.
pub fn tornado_plot(body: &SimulationBody) -> Option<PlotSpec> {
    if body.tornado.is_empty() {
        return None;
    }
    let k = body.tornado.len();
    let mut series = Vec::new();
    for (i, bar) in body.tornado.iter().enumerate() {
        #[allow(clippy::cast_precision_loss)]
        let at = (k - i) as f64;
        series.push(XySeries {
            name: bar.input.clone(),
            x: vec![bar.at_p10, bar.at_p90],
            y: vec![at, at],
            style: SeriesStyle::Line,
        });
    }
    #[allow(clippy::cast_precision_loss)]
    let top = k as f64 + 0.5;
    series.push(XySeries {
        name: "base".into(),
        x: vec![body.base, body.base],
        y: vec![0.5, top],
        style: SeriesStyle::Line,
    });
    let names: Vec<String> = body
        .tornado
        .iter()
        .enumerate()
        .map(|(i, b)| format!("{} = {}", k - i, b.input))
        .collect();
    Some(PlotSpec {
        title: Some("Tornado (P10 to P90 of each input)".into()),
        y_range: Some(PlotRange {
            min: 0.0,
            max: top + 0.5,
        }),
        x_label: Some(body.output.clone()),
        y_label: Some(names.join(", ")),
        series,
        ..PlotSpec::default()
    })
}

/// Draws `body`.
pub fn render(
    id: &str,
    computed: &Computed,
    body: &SimulationBody,
    window: &Window,
    cx: &App,
) -> AnyElement {
    let mut specs = vec![("histogram", histogram_plot(body))];
    if let Some(t) = tornado_plot(body) {
        specs.push(("tornado", t));
    }
    if let Some(c) = convergence_plot(body) {
        specs.push(("convergence", c));
    }
    v_flex()
        .gap_1()
        .child(inputs_grid(body).render(format!("{id}-inputs"), cx))
        .child(results_grid(computed).render(format!("{id}-results"), cx))
        .child(plots(id, specs, window, cx))
        .into_any_element()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use domain::computed::{Curve, Histogram, SimInputSummary, TornadoBar};

    pub(crate) fn body() -> SimulationBody {
        SimulationBody {
            output: "profit".into(),
            unit: Some("€".into()),
            model: "price * volume".into(),
            fermi: false,
            samples: 1000,
            inputs: vec![SimInputSummary {
                name: "price".into(),
                distribution: "normal, mean 10, SD 2".into(),
                p10: 7.4,
                p50: 10.0,
                p90: 12.6,
                source: None,
                unit: None,
            }],
            histogram: Histogram {
                edges: vec![0.0, 10.0, 20.0],
                counts: vec![400, 600],
            },
            convergence: Curve {
                x: vec![10.0, 100.0, 1000.0],
                y: vec![9.0, 10.2, 10.0],
            },
            convergence_mcse: vec![0.6, 0.2, 0.06],
            base: 10.0,
            tornado: vec![
                TornadoBar {
                    input: "price".into(),
                    at_p10: 7.4,
                    at_p90: 12.6,
                    swing: 5.2,
                },
                TornadoBar {
                    input: "volume".into(),
                    at_p10: 9.0,
                    at_p90: 11.0,
                    swing: 2.0,
                },
            ],
        }
    }

    #[test]
    fn tornado_and_convergence_come_from_the_body() {
        let b = body();
        let tornado = tornado_plot(&b).unwrap();
        assert_eq!(tornado.validate(), Ok(()));
        // The largest swing is drawn on top.
        assert_eq!(tornado.series[0].y, [2.0, 2.0]);
        assert_eq!(tornado.series[0].x, [7.4, 12.6]);
        let conv = convergence_plot(&b).unwrap();
        assert_eq!(conv.validate(), Ok(()));
        assert!((conv.series[2].y[0] - 10.2).abs() < 1e-12);
        assert_eq!(histogram_plot(&b).validate(), Ok(()));
        assert_eq!(inputs_grid(&b).rows[0][5].text, "—");
    }
}
