//! The tolerance stack-up card: the results (nominal, worst case, RSS,
//! the Monte Carlo mean, spread and ±3σ range, the share out of
//! specification and Cpk), the chain with each dimension's contribution,
//! and the histogram of the simulated stack with the limits and the
//! worst-case and RSS ranges marked.

use domain::computed::{Computed, StackDirection, ToleranceBody, ToleranceDistribution, apa};
use domain::plot::{PlotSpec, SeriesStyle, XySeries};
use gpui::{AnyElement, App, IntoElement as _, ParentElement as _, Styled as _, Window};
use gpui_component::v_flex;

use super::distribution::plots;
use super::grid::Grid;

/// The results as the engine displays them.
pub fn results_grid(computed: &Computed) -> Grid {
    let mut grid = Grid::new(["Result", "Low", "High"]);
    let shown = |key: &str| {
        computed
            .value(key)
            .map(domain::computed::ComputedValue::display_text)
            .unwrap_or_default()
    };
    grid.push(["Nominal".to_owned(), shown("nominal"), String::new()]);
    grid.push([
        "Worst case".to_owned(),
        shown("worst_case_min"),
        shown("worst_case_max"),
    ]);
    grid.push(["RSS".to_owned(), shown("rss_min"), shown("rss_max")]);
    grid.push([
        "Monte Carlo 0.135% / 99.865%".to_owned(),
        shown("mc_low"),
        shown("mc_high"),
    ]);
    grid.push([
        "Monte Carlo mean / SD".to_owned(),
        shown("mc_mean"),
        shown("mc_sd"),
    ]);
    if computed.value("out_of_spec").is_some() {
        grid.push([
            "Out of specification".to_owned(),
            shown("out_of_spec"),
            String::new(),
        ]);
    }
    if computed.value("cpk").is_some() {
        grid.push(["Cpk".to_owned(), shown("cpk"), String::new()]);
    }
    grid
}

/// `x` with up to six decimals, trailing zeros dropped.
fn short(x: f64) -> String {
    let text = format!("{x:.6}");
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

fn pct(x: f64) -> String {
    format!("{}%", apa::number(x * 100.0, 1))
}

/// The chain with the contributions.
pub fn chain_grid(body: &ToleranceBody) -> Grid {
    let mut grid = Grid::new([
        "Dimension",
        "±",
        "Nominal",
        "Tolerance",
        "Distribution",
        "Worst case",
        "RSS",
        "Variance",
    ]);
    for d in &body.dimensions {
        let tolerance = if d.plus == d.minus {
            format!("±{}", short(d.plus))
        } else {
            format!("+{} / −{}", short(d.plus), short(d.minus))
        };
        grid.push([
            d.name.clone(),
            match d.direction {
                StackDirection::Add => "+",
                StackDirection::Subtract => "−",
            }
            .to_owned(),
            short(d.nominal),
            tolerance,
            match d.distribution {
                ToleranceDistribution::Normal => {
                    format!("normal (±{}σ)", apa::number(body.sigmas, 1))
                }
                ToleranceDistribution::Uniform => "uniform".to_owned(),
            },
            pct(d.worst_case_share),
            pct(d.rss_share),
            pct(d.variance_share),
        ]);
    }
    grid
}

/// The simulated stack with the ranges and limits.
pub fn histogram_plot(body: &ToleranceBody) -> PlotSpec {
    let h = &body.histogram;
    let (mut x, mut y) = (vec![h.edges[0]], vec![0.0]);
    let mut top: f64 = 0.0;
    for (i, &c) in h.counts.iter().enumerate() {
        #[allow(clippy::cast_precision_loss)]
        let c = c as f64;
        top = top.max(c);
        x.extend([h.edges[i], h.edges[i + 1]]);
        y.extend([c, c]);
    }
    x.push(h.edges[h.edges.len() - 1]);
    y.push(0.0);
    let mark = |name: String, at: f64| XySeries {
        name,
        x: vec![at, at],
        y: vec![0.0, top],
        style: SeriesStyle::Line,
    };
    let mut series = vec![XySeries {
        name: "simulated".into(),
        x,
        y,
        style: SeriesStyle::Line,
    }];
    series.push(mark("worst case low".into(), body.worst_case[0]));
    series.push(mark("worst case high".into(), body.worst_case[1]));
    series.push(XySeries {
        name: "RSS".into(),
        x: vec![body.rss[0], body.rss[1]],
        y: vec![top * 0.5, top * 0.5],
        style: SeriesStyle::Line,
    });
    if let Some(lo) = body.lower_limit {
        series.push(mark("lower limit".into(), lo));
    }
    if let Some(hi) = body.upper_limit {
        series.push(mark("upper limit".into(), hi));
    }
    let unit = body
        .unit
        .as_deref()
        .map(|u| format!(" ({u})"))
        .unwrap_or_default();
    PlotSpec {
        title: Some(format!("Stack ({} draws)", body.samples)),
        x_label: Some(format!("result{unit}")),
        y_label: Some("Draws".into()),
        series,
        ..PlotSpec::default()
    }
}

/// Draws `body`.
pub fn render(
    id: &str,
    computed: &Computed,
    body: &ToleranceBody,
    window: &Window,
    cx: &App,
) -> AnyElement {
    v_flex()
        .gap_1()
        .child(results_grid(computed).render(format!("{id}-results"), cx))
        .child(chain_grid(body).render(format!("{id}-chain"), cx))
        .child(plots(id, vec![("stack", histogram_plot(body))], window, cx))
        .into_any_element()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use domain::computed::{Histogram, StackDimension};

    pub(crate) fn body() -> ToleranceBody {
        let dim = |name: &str, nominal: f64, t: f64, direction, share: f64| StackDimension {
            name: name.into(),
            nominal,
            plus: t,
            minus: t,
            direction,
            distribution: ToleranceDistribution::Normal,
            center: nominal,
            half_width: t,
            sd: t / 3.0,
            worst_case_share: share,
            rss_share: share,
            variance_share: share,
        };
        ToleranceBody {
            unit: Some("mm".into()),
            dimensions: vec![
                dim("housing", 50.0, 0.1, StackDirection::Add, 0.5),
                dim("part", 49.8, 0.1, StackDirection::Subtract, 0.5),
            ],
            nominal: 0.2,
            center: 0.2,
            worst_case: [0.0, 0.4],
            rss: [0.0586, 0.3414],
            sigmas: 3.0,
            samples: 1000,
            mc_mean: 0.2,
            mc_sd: 0.047,
            mc_range: [0.06, 0.34],
            histogram: Histogram {
                edges: vec![0.0, 0.2, 0.4],
                counts: vec![500, 500],
            },
            lower_limit: Some(0.05),
            upper_limit: None,
        }
    }

    #[test]
    fn the_chain_and_histogram_show_signs_and_limits() {
        let b = body();
        let chain = chain_grid(&b);
        assert_eq!(chain.rows[1][1].text, "−");
        assert_eq!(chain.rows[0][3].text, "±0.1");
        let plot = histogram_plot(&b);
        assert!(plot.series.iter().any(|s| s.name == "lower limit"));
        assert_eq!(plot.x_label.as_deref(), Some("result (mm)"));
    }
}
