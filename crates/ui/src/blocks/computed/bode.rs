//! The Bode / Nyquist card: the margins table (phase margin at the gain
//! crossover, gain margin at the phase crossover, DC gain, bandwidth,
//! closed-loop stability), the magnitude and phase against frequency on a
//! log axis with the crossovers marked, the Nyquist curve near −1 (with
//! the unit circle), and the poles, zeros and closed-loop poles.

use domain::computed::{BodeBody, Complex, Computed};
use domain::plot::{AxisScale, PlotAspect, PlotSpec, SeriesStyle, XySeries};
use gpui::{
    AnyElement, App, IntoElement as _, ParentElement as _, SharedString, Styled as _, Window, div,
};
use gpui_component::{ActiveTheme as _, v_flex};

use super::distribution::plots;
use super::grid::Grid;

/// Nyquist points with |G| above this are left out, to keep −1 in view.
pub const NYQUIST_CLIP: f64 = 10.0;

/// The headline numbers, as the engine displays them.
pub fn margins_grid(computed: &Computed, body: &BodeBody) -> Grid {
    let unit = body.unit.symbol();
    let mut grid = Grid::new(["Result", "Value"]);
    let rows = [
        ("Phase margin", "phase_margin", "°"),
        ("Gain crossover", "gain_crossover", unit),
        ("Gain margin", "gain_margin", "dB"),
        ("Phase crossover", "phase_crossover", unit),
        ("DC gain", "dc_gain", "dB"),
        ("−3 dB bandwidth of G", "bandwidth", unit),
    ];
    for (label, key, suffix) in rows {
        let text = computed.value(key).map_or_else(
            || match key {
                "phase_margin" => "none (no 0 dB crossing)".to_owned(),
                "gain_margin" => "∞ (phase never −180°)".to_owned(),
                _ => "—".to_owned(),
            },
            |v| format!("{} {suffix}", v.display_text()),
        );
        grid.push([label.to_owned(), text]);
    }
    if let Some(v) = computed.value("closed_loop_max_re") {
        grid.push([
            "Closed loop (unity feedback)".to_owned(),
            format!(
                "{}, max Re(pole) = {}",
                v.label.as_deref().unwrap_or(""),
                v.display_text()
            ),
        ]);
    }
    grid
}

fn roots_text(roots: &[Complex]) -> String {
    if roots.is_empty() {
        "—".to_owned()
    } else {
        roots
            .iter()
            .map(|r| r.text(4))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Poles, zeros and closed-loop poles.
pub fn roots_grid(body: &BodeBody) -> Grid {
    let mut grid = Grid::new(["", "Roots"]);
    grid.push(["Zeros".to_owned(), roots_text(&body.zeros)]);
    grid.push(["Poles".to_owned(), roots_text(&body.poles)]);
    if body.delay == 0.0 {
        grid.push([
            "Closed-loop poles".to_owned(),
            roots_text(&body.closed_loop_poles),
        ]);
    }
    grid
}

/// Magnitude, phase and Nyquist plots.
pub fn plots_of(body: &BodeBody) -> Vec<(&'static str, PlotSpec)> {
    let unit = body.unit;
    let x: Vec<f64> = body.points.iter().map(|p| unit.from_rad(p.w)).collect();
    let (first, last) = (
        x.first().copied().unwrap_or(1.0),
        x.last().copied().unwrap_or(10.0),
    );
    let mut magnitude = vec![
        XySeries {
            name: "|G| (dB)".into(),
            x: x.clone(),
            y: body.points.iter().map(|p| p.magnitude_db).collect(),
            style: SeriesStyle::Line,
        },
        XySeries {
            name: "0 dB".into(),
            x: vec![first, last],
            y: vec![0.0, 0.0],
            style: SeriesStyle::Line,
        },
    ];
    if !body.gain_crossovers.is_empty() {
        magnitude.push(XySeries {
            name: "gain crossover".into(),
            x: body
                .gain_crossovers
                .iter()
                .map(|c| unit.from_rad(c.w))
                .collect(),
            y: vec![0.0; body.gain_crossovers.len()],
            style: SeriesStyle::Scatter,
        });
    }
    let mut phase = vec![XySeries {
        name: "arg G (°)".into(),
        x: x.clone(),
        y: body.points.iter().map(|p| p.phase_deg).collect(),
        style: SeriesStyle::Line,
    }];
    let (lo, hi) = body
        .points
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), p| {
            (lo.min(p.phase_deg), hi.max(p.phase_deg))
        });
    // The −180° + k·360° lines within the phase range.
    let mut k = ((lo + 180.0) / 360.0).ceil();
    while 360.0 * k - 180.0 <= hi {
        let level = 360.0 * k - 180.0;
        phase.push(XySeries {
            name: format!("{level}°"),
            x: vec![first, last],
            y: vec![level, level],
            style: SeriesStyle::Line,
        });
        k += 1.0;
    }
    if !body.phase_crossovers.is_empty() {
        phase.push(XySeries {
            name: "phase crossover".into(),
            x: body
                .phase_crossovers
                .iter()
                .map(|c| unit.from_rad(c.w))
                .collect(),
            y: body
                .phase_crossovers
                .iter()
                .map(|c| {
                    body.points
                        .iter()
                        .min_by(|a, b| (a.w - c.w).abs().total_cmp(&(b.w - c.w).abs()))
                        .map_or(-180.0, |p| {
                            ((p.phase_deg + 180.0) / 360.0).round() * 360.0 - 180.0
                        })
                })
                .collect(),
            style: SeriesStyle::Scatter,
        });
    }
    let kept: Vec<_> = body
        .points
        .iter()
        .filter(|p| p.re.hypot(p.im) <= NYQUIST_CLIP)
        .collect();
    let circle: Vec<f64> = (0..=72)
        .map(|i| f64::from(i) * std::f64::consts::TAU / 72.0)
        .collect();
    let nyquist = vec![
        XySeries {
            name: "G(jω), ω > 0".into(),
            x: kept.iter().map(|p| p.re).collect(),
            y: kept.iter().map(|p| p.im).collect(),
            style: SeriesStyle::Line,
        },
        XySeries {
            name: "G(−jω)".into(),
            x: kept.iter().map(|p| p.re).collect(),
            y: kept.iter().map(|p| -p.im).collect(),
            style: SeriesStyle::Line,
        },
        XySeries {
            name: "unit circle".into(),
            x: circle.iter().map(|t| t.cos()).collect(),
            y: circle.iter().map(|t| t.sin()).collect(),
            style: SeriesStyle::Line,
        },
        XySeries {
            name: "−1".into(),
            x: vec![-1.0],
            y: vec![0.0],
            style: SeriesStyle::Scatter,
        },
    ];
    let label = format!("ω ({})", unit.symbol());
    vec![
        (
            "magnitude",
            PlotSpec {
                title: Some("Magnitude".into()),
                x_label: Some(label.clone()),
                y_label: Some("dB".into()),
                series: magnitude,
                x_scale: AxisScale::Log,
                ..PlotSpec::default()
            },
        ),
        (
            "phase",
            PlotSpec {
                title: Some("Phase".into()),
                x_label: Some(label),
                y_label: Some("degrees".into()),
                series: phase,
                x_scale: AxisScale::Log,
                ..PlotSpec::default()
            },
        ),
        (
            "nyquist",
            PlotSpec {
                title: Some(format!("Nyquist (|G| ≤ {NYQUIST_CLIP})")),
                x_label: Some("Re G".into()),
                y_label: Some("Im G".into()),
                series: nyquist,
                aspect: PlotAspect::Equal,
                ..PlotSpec::default()
            },
        ),
    ]
}

/// Draws `body`.
pub fn render(
    id: &str,
    computed: &Computed,
    body: &BodeBody,
    window: &Window,
    cx: &App,
) -> AnyElement {
    v_flex()
        .gap_1()
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(SharedString::from(format!(
                    "G(s) = {}",
                    body.transfer_text()
                ))),
        )
        .child(margins_grid(computed, body).render(format!("{id}-margins"), cx))
        .child(plots(id, plots_of(body), window, cx))
        .child(roots_grid(body).render(format!("{id}-roots"), cx))
        .into_any_element()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use domain::computed::{BodePoint, Crossover, FrequencyUnit};

    pub(crate) fn body() -> BodeBody {
        let point = |w: f64, db: f64, deg: f64| BodePoint {
            w,
            magnitude_db: db,
            phase_deg: deg,
            re: 10f64.powf(db / 20.0) * deg.to_radians().cos(),
            im: 10f64.powf(db / 20.0) * deg.to_radians().sin(),
        };
        BodeBody {
            numerator: vec![1.0],
            denominator: vec![1.0, 3.0, 2.0, 0.0],
            delay: 0.0,
            unit: FrequencyUnit::RadPerS,
            points: vec![
                point(0.01, 34.0, -90.6),
                point(0.45, 0.0, -126.6),
                point(1.414, -15.56, -180.0),
                point(100.0, -120.0, -268.0),
            ],
            zeros: Vec::new(),
            poles: vec![
                Complex { re: -2.0, im: 0.0 },
                Complex { re: -1.0, im: 0.0 },
                Complex { re: 0.0, im: 0.0 },
            ],
            gain_crossovers: vec![Crossover {
                w: 0.446,
                margin: 53.4,
            }],
            phase_crossovers: vec![Crossover {
                w: 1.414,
                margin: 15.56,
            }],
            closed_loop_poles: vec![Complex { re: -2.3, im: 0.0 }],
        }
    }

    #[test]
    fn plots_mark_crossovers_and_clip_the_nyquist_curve() {
        let b = body();
        let specs = plots_of(&b);
        assert_eq!(specs.len(), 3);
        assert_eq!(specs[0].1.x_scale, AxisScale::Log);
        assert!(specs[1].1.series.iter().any(|s| s.name == "-180°"));
        assert_eq!(
            specs[1].1.series.last().map(|s| s.y.clone()),
            Some(vec![-180.0])
        );
        // The 34 dB point is outside |G| ≤ 10.
        assert_eq!(specs[2].1.series[0].x.len(), 3);
        let roots = roots_grid(&b);
        assert_eq!(roots.rows[1][1].text, "-2.0000, -1.0000, 0.0000");
    }
}
