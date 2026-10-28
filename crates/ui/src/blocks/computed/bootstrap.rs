//! The bootstrap card: the intervals with their Monte Carlo error, the
//! resampling settings (B, seed, generator, z₀, acceleration) and the
//! histogram of the replicates with the observed value and the first
//! interval marked.

use domain::computed::{BootstrapBody, Computed};
use domain::plot::{PlotSpec, SeriesStyle, XySeries};
use gpui::{
    AnyElement, App, IntoElement as _, ParentElement as _, SharedString, Styled as _, Window, div,
};
use gpui_component::{ActiveTheme as _, v_flex};

use super::distribution::plots;
use super::grid::Grid;
use super::{auto_decimals, number};

/// The intervals table.
pub fn intervals_grid(body: &BootstrapBody) -> Grid {
    let mut grid = Grid::new(["Interval", "Low", "High", "MCSE low", "MCSE high"]);
    let decimals = auto_decimals(
        body.intervals
            .first()
            .map_or(1.0, |i| (i.hi - i.lo).abs() / 4.0),
    )
    .max(2);
    let mcse = |m: Option<f64>| m.map(|m| number(m, decimals + 1)).unwrap_or_default();
    for interval in &body.intervals {
        grid.push([
            interval.method.label().to_owned(),
            number(interval.lo, decimals),
            number(interval.hi, decimals),
            mcse(interval.lo_mcse),
            mcse(interval.hi_mcse),
        ]);
    }
    grid
}

/// The replicates' histogram, the observed value and the first (BCa or
/// percentile) interval as vertical lines.
pub fn replicates_plot(body: &BootstrapBody) -> PlotSpec {
    let h = &body.histogram;
    let (mut x, mut y) = (vec![h.edges[0]], vec![0.0]);
    let mut top = 0.0_f64;
    for (i, &c) in h.counts.iter().enumerate() {
        #[allow(clippy::cast_precision_loss)]
        let c = c as f64;
        top = top.max(c);
        x.extend([h.edges[i], h.edges[i + 1]]);
        y.extend([c, c]);
    }
    x.push(h.edges[h.edges.len() - 1]);
    y.push(0.0);
    let vertical = |name: String, at: f64| XySeries {
        name,
        x: vec![at, at],
        y: vec![0.0, top],
        style: SeriesStyle::Line,
    };
    let mut series = vec![XySeries {
        name: format!("{} replicates", body.resamples),
        x,
        y,
        style: SeriesStyle::Line,
    }];
    if body.observed.is_finite() {
        series.push(vertical("observed".into(), body.observed));
    }
    if let Some(first) = body.intervals.first() {
        let label = first.method.label();
        series.push(vertical(format!("{label} low"), first.lo));
        series.push(vertical(format!("{label} high"), first.hi));
    }
    PlotSpec {
        title: Some(format!(
            "Bootstrap distribution of the {}",
            body.statistic.label()
        )),
        x_label: Some(body.variable.clone()),
        y_label: Some("Resamples".into()),
        series,
        ..PlotSpec::default()
    }
}

/// Draws `body`.
pub fn render(
    id: &str,
    computed: &Computed,
    body: &BootstrapBody,
    window: &Window,
    cx: &App,
) -> AnyElement {
    let mut settings = vec![format!("B = {}", body.resamples)];
    if let Some(seed) = computed.seed {
        settings.push(format!("seed {seed}"));
    }
    if let Some(rng) = &computed.rng {
        settings.push(rng.clone());
    }
    if let Some(z0) = body.z0 {
        settings.push(format!("z₀ = {}", number(z0, 3)));
    }
    if let Some(a) = body.acceleration {
        settings.push(format!("a = {}", number(a, 4)));
    }
    v_flex()
        .gap_1()
        .child(intervals_grid(body).render(format!("{id}-intervals"), cx))
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(SharedString::from(settings.join(" · "))),
        )
        .child(plots(
            id,
            vec![("replicates", replicates_plot(body))],
            window,
            cx,
        ))
        .into_any_element()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use domain::computed::{BootInterval, BootMethod, BootStatistic, Histogram};

    pub(crate) fn body() -> BootstrapBody {
        BootstrapBody {
            statistic: BootStatistic::Mean,
            variable: "lsat".into(),
            versus: None,
            observed: 600.3,
            resamples: 2000,
            conf_level: 0.95,
            histogram: Histogram {
                edges: vec![580.0, 600.0, 620.0],
                counts: vec![900, 1100],
            },
            intervals: vec![
                BootInterval {
                    method: BootMethod::Bca,
                    lo: 581.2,
                    hi: 619.9,
                    lo_mcse: Some(0.4),
                    hi_mcse: Some(0.5),
                },
                BootInterval {
                    method: BootMethod::Normal,
                    lo: 580.9,
                    hi: 619.6,
                    lo_mcse: None,
                    hi_mcse: None,
                },
            ],
            z0: Some(0.01),
            acceleration: Some(0.02),
        }
    }

    #[test]
    fn the_plot_marks_the_observed_value_and_the_interval() {
        let spec = replicates_plot(&body());
        assert_eq!(spec.validate(), Ok(()));
        assert_eq!(spec.series[1].x, [600.3, 600.3]);
        assert_eq!(spec.series[1].y, [0.0, 1100.0]);
        assert_eq!(spec.series[2].name, "BCa low");
        let grid = intervals_grid(&body());
        assert_eq!(grid.rows[0][0].text, "BCa");
        assert_eq!(grid.rows[1][3].text, "");
    }
}
