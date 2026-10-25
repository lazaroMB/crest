//! The distribution card: a summary row per group, then four plots drawn
//! with the plot painter from the engine's coordinates: histogram with the
//! density curve (scaled to counts), box and violin per group, normal QQ
//! with its pointwise band, and the ECDF with its DKW band.

use domain::computed::{DistributionBody, DistributionGroup};
use domain::plot::{PlotSpec, SeriesStyle, XySeries};
use gpui::{
    AnyElement, App, InteractiveElement as _, IntoElement as _, ParentElement as _, SharedString,
    Styled as _, Window, div, px,
};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};

use super::grid::Grid;
use super::{auto_decimals, number};

/// Narrowest a plot gets before the plots wrap.
const PLOT_MIN: f32 = 260.0;
/// Half the width of a box (and the widest violin), in group units.
const BOX_HALF: f64 = 0.3;

fn group_name(body: &DistributionBody, group: &DistributionGroup) -> String {
    group.name.clone().unwrap_or_else(|| body.variable.clone())
}

fn series(name: String, x: Vec<f64>, y: Vec<f64>, style: SeriesStyle) -> XySeries {
    XySeries { name, x, y, style }
}

fn axis_label(body: &DistributionBody) -> String {
    match &body.unit {
        Some(unit) if !unit.is_empty() => format!("{} ({unit})", body.variable),
        _ => body.variable.clone(),
    }
}

/// The histogram outline of each group, with its density scaled to
/// counts (density × n × bin width).
pub fn histogram_plot(body: &DistributionBody) -> PlotSpec {
    let mut all = Vec::new();
    for group in &body.groups {
        let name = group_name(body, group);
        let h = &group.histogram;
        let (mut x, mut y) = (vec![h.edges[0]], vec![0.0]);
        for (i, &c) in h.counts.iter().enumerate() {
            #[allow(clippy::cast_precision_loss)]
            let c = c as f64;
            x.extend([h.edges[i], h.edges[i + 1]]);
            y.extend([c, c]);
        }
        x.push(h.edges[h.edges.len() - 1]);
        y.push(0.0);
        all.push(series(format!("{name} histogram"), x, y, SeriesStyle::Line));
        #[allow(clippy::cast_precision_loss)]
        let scale = group.n as f64 * body.bin_width;
        all.push(series(
            format!("{name} density"),
            group.kde.x.clone(),
            group.kde.y.iter().map(|d| d * scale).collect(),
            SeriesStyle::Line,
        ));
    }
    PlotSpec {
        title: Some("Histogram and density".into()),
        x_label: Some(axis_label(body)),
        y_label: Some("Count".into()),
        series: all,
        ..PlotSpec::default()
    }
}

/// A violin (the density mirrored) and a box per group, groups at x = 1,
/// 2, …; outliers as points.
pub fn box_plot(body: &DistributionBody) -> PlotSpec {
    let peak = body
        .groups
        .iter()
        .flat_map(|g| g.kde.y.iter().copied())
        .fold(0.0_f64, f64::max);
    let mut all = Vec::new();
    let (mut out_x, mut out_y) = (Vec::new(), Vec::new());
    for (ix, group) in body.groups.iter().enumerate() {
        #[allow(clippy::cast_precision_loss)]
        let at = (ix + 1) as f64;
        let name = group_name(body, group);
        if peak > 0.0 && !group.kde.x.is_empty() {
            let half = |d: f64| d / peak * BOX_HALF;
            let mut x: Vec<f64> = group.kde.y.iter().map(|&d| at + half(d)).collect();
            let mut y = group.kde.x.clone();
            x.extend(group.kde.y.iter().rev().map(|&d| at - half(d)));
            y.extend(group.kde.x.iter().rev());
            x.push(x[0]);
            y.push(y[0]);
            all.push(series(format!("{name} violin"), x, y, SeriesStyle::Line));
        }
        let b = &group.boxplot;
        let w = BOX_HALF / 2.0;
        // One stroke: whisker, box (retracing edges), median, whisker.
        let path = [
            (at, b.whisker_lo),
            (at, b.lower_hinge),
            (at - w, b.lower_hinge),
            (at - w, b.upper_hinge),
            (at + w, b.upper_hinge),
            (at + w, b.lower_hinge),
            (at - w, b.lower_hinge),
            (at - w, b.median),
            (at + w, b.median),
            (at + w, b.upper_hinge),
            (at, b.upper_hinge),
            (at, b.whisker_hi),
        ];
        all.push(series(
            format!("{name} box"),
            path.iter().map(|p| p.0).collect(),
            path.iter().map(|p| p.1).collect(),
            SeriesStyle::Line,
        ));
        out_x.extend(std::iter::repeat_n(at, b.outliers.len()));
        out_y.extend(b.outliers.iter().copied());
    }
    if !out_x.is_empty() {
        all.push(series(
            "outliers".into(),
            out_x,
            out_y,
            SeriesStyle::Scatter,
        ));
    }
    let x_label = match &body.by {
        Some(by) => {
            let names: Vec<String> = body
                .groups
                .iter()
                .enumerate()
                .map(|(i, g)| format!("{} = {}", i + 1, group_name(body, g)))
                .collect();
            format!("{by} ({})", names.join(", "))
        }
        None => String::new(),
    };
    #[allow(clippy::cast_precision_loss)]
    let groups = body.groups.len() as f64;
    PlotSpec {
        title: Some("Box and violin".into()),
        x_range: Some(domain::plot::PlotRange {
            min: 0.5,
            max: groups + 0.5,
        }),
        x_label: (!x_label.is_empty()).then_some(x_label),
        y_label: Some(axis_label(body)),
        series: all,
        ..PlotSpec::default()
    }
}

/// Normal QQ points of each group with the pointwise band.
pub fn qq_plot(body: &DistributionBody) -> Option<PlotSpec> {
    let mut all = Vec::new();
    for group in body.groups.iter().filter(|g| g.qq.len() >= 3) {
        let name = group_name(body, group);
        let x: Vec<f64> = group.qq.iter().map(|p| p.theoretical).collect();
        all.push(series(
            name.clone(),
            x.clone(),
            group.qq.iter().map(|p| p.sample).collect(),
            SeriesStyle::Scatter,
        ));
        all.push(series(
            format!("{name} band low"),
            x.clone(),
            group.qq.iter().map(|p| p.lo).collect(),
            SeriesStyle::Line,
        ));
        all.push(series(
            format!("{name} band high"),
            x,
            group.qq.iter().map(|p| p.hi).collect(),
            SeriesStyle::Line,
        ));
    }
    (!all.is_empty()).then(|| PlotSpec {
        title: Some(format!(
            "Normal QQ, {}% pointwise band",
            number(body.conf_level * 100.0, 0)
        )),
        x_label: Some("Theoretical quantile".into()),
        y_label: Some(axis_label(body)),
        series: all,
        ..PlotSpec::default()
    })
}

/// The ECDF of each group as a step line, with its DKW band.
pub fn ecdf_plot(body: &DistributionBody) -> PlotSpec {
    let mut all = Vec::new();
    for group in &body.groups {
        let name = group_name(body, group);
        let (mut x, mut y) = (Vec::new(), Vec::new());
        let mut last = 0.0;
        for step in &group.ecdf {
            x.extend([step.x, step.x]);
            y.extend([last, step.p]);
            last = step.p;
        }
        let e = group.dkw_epsilon;
        let lo: Vec<f64> = y.iter().map(|p| (p - e).max(0.0)).collect();
        let hi: Vec<f64> = y.iter().map(|p| (p + e).min(1.0)).collect();
        all.push(series(
            format!("{name} ECDF"),
            x.clone(),
            y,
            SeriesStyle::Line,
        ));
        all.push(series(
            format!("{name} DKW low"),
            x.clone(),
            lo,
            SeriesStyle::Line,
        ));
        all.push(series(format!("{name} DKW high"), x, hi, SeriesStyle::Line));
    }
    PlotSpec {
        title: Some(format!(
            "ECDF, {}% DKW band",
            number(body.conf_level * 100.0, 0)
        )),
        y_range: Some(domain::plot::PlotRange { min: 0.0, max: 1.0 }),
        x_label: Some(axis_label(body)),
        y_label: Some("Share ≤ x".into()),
        series: all,
        ..PlotSpec::default()
    }
}

/// The plots side by side, wrapping when narrow.
pub fn plots(id: &str, specs: Vec<(&str, PlotSpec)>, window: &Window, cx: &App) -> AnyElement {
    let muted = cx.theme().muted_foreground;
    h_flex()
        .debug_selector(|| "computed-plots".to_owned())
        .w_full()
        .flex_wrap()
        .gap_3()
        .items_start()
        .children(specs.into_iter().map(|(name, spec)| {
            let title = spec.title.clone().unwrap_or_default();
            v_flex()
                .flex_grow_1()
                .flex_shrink_1()
                .flex_basis(px(0.))
                .min_w(px(PLOT_MIN))
                .gap_1()
                .child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child(SharedString::from(title)),
                )
                .child(crate::blocks::plot::static_plot(
                    &PlotSpec {
                        title: None,
                        ..spec
                    },
                    &format!("{id}-{name}"),
                    window,
                    cx,
                ))
        }))
        .into_any_element()
}

/// Draws `body`.
pub fn render(id: &str, body: &DistributionBody, window: &Window, cx: &App) -> AnyElement {
    let mut table = Grid::new([
        body.by.clone().unwrap_or_else(|| "Variable".into()),
        "n".into(),
        "Median".into(),
        "Lower hinge".into(),
        "Upper hinge".into(),
        "Outliers".into(),
        "Bandwidth".into(),
    ]);
    for group in &body.groups {
        let b = &group.boxplot;
        let decimals = auto_decimals((b.upper_hinge - b.lower_hinge).abs());
        table.push([
            group_name(body, group),
            group.n.to_string(),
            number(b.median, decimals),
            number(b.lower_hinge, decimals),
            number(b.upper_hinge, decimals),
            b.outliers.len().to_string(),
            number(group.bandwidth, decimals + 1),
        ]);
    }
    let decimals = auto_decimals(body.bin_width);
    let rules = format!(
        "Bins: {} of width {} ({}) · bandwidth: {}",
        body.groups.first().map_or(0, |g| g.histogram.counts.len()),
        number(body.bin_width, decimals),
        body.bin_rule.label(),
        body.bandwidth_rule.label()
    );
    let mut specs = vec![("histogram", histogram_plot(body)), ("box", box_plot(body))];
    if let Some(qq) = qq_plot(body) {
        specs.push(("qq", qq));
    }
    specs.push(("ecdf", ecdf_plot(body)));
    v_flex()
        .gap_1()
        .child(table.render(format!("{id}-groups"), cx))
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(SharedString::from(rules)),
        )
        .child(plots(id, specs, window, cx))
        .into_any_element()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use domain::computed::{
        BandwidthRule, BinRule, BoxSummary, Curve, EcdfStep, Histogram, QqPoint,
    };

    pub(crate) fn body() -> DistributionBody {
        let group = |name: &str, shift: f64| DistributionGroup {
            name: Some(name.into()),
            n: 4,
            histogram: Histogram {
                edges: vec![0.0, 1.0, 2.0],
                counts: vec![3, 1],
            },
            kde: Curve {
                x: vec![0.0, 1.0, 2.0],
                y: vec![0.1, 0.4, 0.1],
            },
            bandwidth: 0.5,
            boxplot: BoxSummary {
                lower_hinge: 0.5 + shift,
                median: 0.8 + shift,
                upper_hinge: 1.2 + shift,
                whisker_lo: 0.1 + shift,
                whisker_hi: 1.5 + shift,
                outliers: vec![3.0],
            },
            qq: vec![
                QqPoint {
                    theoretical: -1.0,
                    sample: 0.1,
                    lo: -0.5,
                    hi: 0.6,
                },
                QqPoint {
                    theoretical: 0.0,
                    sample: 0.8,
                    lo: 0.3,
                    hi: 1.2,
                },
                QqPoint {
                    theoretical: 1.0,
                    sample: 1.5,
                    lo: 1.0,
                    hi: 2.2,
                },
            ],
            ecdf: vec![EcdfStep { x: 0.1, p: 0.5 }, EcdfStep { x: 1.5, p: 1.0 }],
            dkw_epsilon: 0.3,
        };
        DistributionBody {
            variable: "mpg".into(),
            unit: None,
            by: Some("am".into()),
            bin_rule: BinRule::FreedmanDiaconis,
            bin_width: 1.0,
            bandwidth_rule: BandwidthRule::Silverman,
            conf_level: 0.95,
            groups: vec![group("0", 0.0), group("1", 0.5)],
        }
    }

    #[test]
    fn plots_come_from_the_engine_coordinates() {
        let b = body();
        let hist = histogram_plot(&b);
        assert_eq!(hist.series[0].x, [0.0, 0.0, 1.0, 1.0, 2.0, 2.0]);
        assert_eq!(hist.series[0].y, [0.0, 3.0, 3.0, 1.0, 1.0, 0.0]);
        // The density is scaled to counts: n × width = 4.
        assert_eq!(hist.series[1].y, [0.4, 1.6, 0.4]);
        let boxes = box_plot(&b);
        assert_eq!(boxes.validate(), Ok(()));
        let outliers = boxes.series.last().unwrap();
        assert_eq!(outliers.x, [1.0, 2.0]);
        assert_eq!(outliers.style, SeriesStyle::Scatter);
        assert!(boxes.x_label.as_deref().unwrap().contains("1 = 0, 2 = 1"));
        let qq = qq_plot(&b).unwrap();
        assert_eq!(qq.series.len(), 6);
        let ecdf = ecdf_plot(&b);
        assert_eq!(ecdf.series[0].y, [0.0, 0.5, 0.5, 1.0]);
        assert_eq!(ecdf.series[1].y, [0.0, 0.2, 0.2, 0.7]);
        assert_eq!(ecdf.series[2].y[3], 1.0);
        for spec in [hist, boxes, qq, ecdf] {
            assert_eq!(spec.validate(), Ok(()));
        }
    }
}
