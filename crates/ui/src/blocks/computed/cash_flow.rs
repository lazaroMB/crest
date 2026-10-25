//! The cash-flow card: NPV, IRR(s), MIRR and payback, the period table
//! (flow, discount factor, present value, running totals) and the NPV
//! against the discount rate with every IRR where it crosses zero.

use domain::computed::{CashFlowBody, Computed, apa};
use domain::plot::{PlotSpec, SeriesStyle, XySeries};
use gpui::{AnyElement, App, IntoElement as _, ParentElement as _, Styled as _, Window};
use gpui_component::v_flex;

use super::distribution::plots;
use super::grid::Grid;

fn money(x: f64) -> String {
    apa::number(x, 2)
}

/// The headline numbers, as the engine displays them.
pub fn summary_grid(computed: &Computed) -> Grid {
    let mut grid = Grid::new(["Result", "Value"]);
    let mut keys: Vec<(String, String)> = vec![
        ("NPV".into(), "npv".into()),
        ("Sum of flows".into(), "total".into()),
        ("IRR".into(), "irr".into()),
    ];
    for key in computed.values.keys().filter(|k| k.starts_with("irr_")) {
        keys.push((format!("IRR {}", &key[4..]), key.clone()));
    }
    keys.extend([
        ("MIRR".into(), "mirr".into()),
        ("Payback (periods)".into(), "payback".into()),
        ("Discounted payback".into(), "discounted_payback".into()),
        ("Sign changes".into(), "sign_changes".into()),
    ]);
    for (label, key) in keys {
        if let Some(v) = computed.value(&key) {
            grid.push([label, v.display_text()]);
        }
    }
    grid
}

/// The period table.
pub fn periods_grid(body: &CashFlowBody) -> Grid {
    let period = body.period_label.clone().unwrap_or_else(|| "Period".into());
    let mut grid = Grid::new([
        period,
        "Cash flow".into(),
        "Discount factor".into(),
        "Present value".into(),
        "Cumulative".into(),
        "Cumulative PV".into(),
    ]);
    for row in &body.rows {
        grid.push([
            row.period.to_string(),
            money(row.cash_flow),
            apa::number(row.discount_factor, 6),
            money(row.present_value),
            money(row.cumulative),
            money(row.cumulative_pv),
        ]);
    }
    grid
}

/// NPV over the discount rate (in %), the IRRs on the zero line and the
/// chosen rate.
pub fn npv_plot(body: &CashFlowBody) -> Option<PlotSpec> {
    let c = &body.npv_curve;
    if c.x.len() < 2 {
        return None;
    }
    let pct: Vec<f64> = c.x.iter().map(|r| r * 100.0).collect();
    let (first, last) = (pct[0], pct[pct.len() - 1]);
    let mut series = vec![
        XySeries {
            name: "NPV".into(),
            x: pct,
            y: c.y.clone(),
            style: SeriesStyle::Line,
        },
        XySeries {
            name: "0".into(),
            x: vec![first, last],
            y: vec![0.0, 0.0],
            style: SeriesStyle::Line,
        },
    ];
    if !body.irrs.is_empty() {
        series.push(XySeries {
            name: "IRR".into(),
            x: body.irrs.iter().map(|r| r * 100.0).collect(),
            y: vec![0.0; body.irrs.len()],
            style: SeriesStyle::Scatter,
        });
    }
    if let Some(npv) = body.rows.last().map(|r| r.cumulative_pv) {
        series.push(XySeries {
            name: format!("rate {}%", apa::number(body.rate * 100.0, 2)),
            x: vec![body.rate * 100.0],
            y: vec![npv],
            style: SeriesStyle::Scatter,
        });
    }
    Some(PlotSpec {
        title: Some("NPV against the discount rate".into()),
        x_label: Some("rate (%)".into()),
        y_label: Some(match &body.currency {
            Some(c) => format!("NPV ({c})"),
            None => "NPV".into(),
        }),
        series,
        ..PlotSpec::default()
    })
}

/// Draws `body`.
pub fn render(
    id: &str,
    computed: &Computed,
    body: &CashFlowBody,
    window: &Window,
    cx: &App,
) -> AnyElement {
    let mut column = v_flex()
        .gap_1()
        .child(summary_grid(computed).render(format!("{id}-summary"), cx))
        .child(periods_grid(body).render(format!("{id}-periods"), cx));
    if let Some(spec) = npv_plot(body) {
        column = column.child(plots(id, vec![("npv", spec)], window, cx));
    }
    column.into_any_element()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use domain::computed::{CashFlowRow, Curve};

    pub(crate) fn body() -> CashFlowBody {
        let row = |period, cash_flow: f64, factor: f64, cum: f64, cum_pv: f64| CashFlowRow {
            period,
            cash_flow,
            discount_factor: factor,
            present_value: cash_flow * factor,
            cumulative: cum,
            cumulative_pv: cum_pv,
        };
        CashFlowBody {
            rate: 0.15,
            finance_rate: 0.15,
            reinvest_rate: 0.15,
            currency: Some("USD".into()),
            period_label: Some("year".into()),
            rows: vec![
                row(0, -100.0, 1.0, -100.0, -100.0),
                row(1, 230.0, 0.869_565, 130.0, 100.0),
                row(2, -132.0, 0.756_144, -2.0, 0.189),
            ],
            irrs: vec![0.1, 0.2],
            sign_changes: 2,
            npv_curve: Curve {
                x: vec![0.0, 0.15, 0.3],
                y: vec![-2.0, 0.19, -0.9],
            },
        }
    }

    #[test]
    fn the_curve_marks_every_irr() {
        let b = body();
        let plot = npv_plot(&b).unwrap();
        assert_eq!(plot.validate(), Ok(()));
        assert_eq!(plot.series[2].x, [10.0, 20.0]);
        assert_eq!(plot.series[3].x, [15.0]);
        let grid = periods_grid(&b);
        assert_eq!(grid.headers[0], "year");
        assert_eq!(grid.rows[2][1].text, "−132.00");
    }
}
