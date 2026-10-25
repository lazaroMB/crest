//! The LP/MILP card: status and objective, the variables with their
//! values and reduced costs, the constraints with activity, slack, binding
//! mark and shadow price, and, for two continuous variables, the
//! constraint lines with the optimum.

use domain::computed::{OptimizationBody, OptimizationStatus, apa};
use domain::plot::{PlotRange, PlotSpec, SeriesStyle, XySeries};
use gpui::{
    AnyElement, App, IntoElement as _, ParentElement as _, SharedString, Styled as _, Window, div,
};
use gpui_component::{ActiveTheme as _, v_flex};

use super::distribution::plots;
use super::grid::{Grid, GridCell};

fn num(x: f64) -> String {
    let text = apa::number(x, 4);
    if text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.').to_owned()
    } else {
        text
    }
}

fn opt(x: Option<f64>, none: &str) -> String {
    x.map_or_else(|| none.to_owned(), num)
}

/// The variables table.
pub fn variables_grid(body: &OptimizationBody) -> Grid {
    let mut grid = Grid::new([
        "Variable",
        "Value",
        "Lower",
        "Upper",
        "Objective coef.",
        "Reduced cost",
    ]);
    for v in &body.variables {
        let name = if v.integer {
            format!("{} (integer)", v.name)
        } else {
            v.name.clone()
        };
        grid.push([
            name,
            opt(v.value, ""),
            opt(v.lower, "−∞"),
            opt(v.upper, "∞"),
            num(v.objective_coefficient),
            opt(v.reduced_cost, ""),
        ]);
    }
    grid
}

/// The constraints table; binding rows are highlighted.
pub fn constraints_grid(body: &OptimizationBody, accent: gpui::Hsla) -> Grid {
    let mut grid = Grid::new([
        "Constraint",
        "Activity",
        "Op",
        "RHS",
        "Slack",
        "Shadow price",
    ]);
    for k in &body.constraints {
        let bg = k.binding.then(|| accent.opacity(0.12));
        let cell = |text: String| GridCell { text, bg, fg: None };
        grid.rows.push(vec![
            cell(if k.binding {
                format!("{} (binding)", k.name)
            } else {
                k.name.clone()
            }),
            cell(opt(k.activity, "")),
            cell(k.op.symbol().to_owned()),
            cell(num(k.rhs)),
            cell(opt(k.slack, "")),
            cell(opt(k.dual, "")),
        ]);
    }
    grid
}

/// For two continuous variables: each constraint's boundary line over the
/// first quadrant around the optimum, and the optimum as a point.
pub fn region_plot(body: &OptimizationBody) -> Option<PlotSpec> {
    let [a, b] = body.variables.as_slice() else {
        return None;
    };
    let (x0, y0) = (a.value?, b.value?);
    let span = |v: f64| (v.abs() * 2.0).max(1.0);
    let (xmax, ymax) = (span(x0), span(y0));
    let mut series = Vec::new();
    for k in &body.constraints {
        if let Some((p, q)) = boundary(&k.coefficients, k.rhs, xmax) {
            series.push(XySeries {
                name: k.name.clone(),
                x: vec![p.0, q.0],
                y: vec![p.1, q.1],
                style: SeriesStyle::Line,
            });
        }
    }
    series.push(XySeries {
        name: "optimum".into(),
        x: vec![x0],
        y: vec![y0],
        style: SeriesStyle::Scatter,
    });
    Some(PlotSpec {
        title: Some("Constraints and the optimum".into()),
        x_range: Some(PlotRange {
            min: 0.0_f64.min(x0 - 1.0),
            max: xmax,
        }),
        y_range: Some(PlotRange {
            min: 0.0_f64.min(y0 - 1.0),
            max: ymax,
        }),
        x_label: Some(a.name.clone()),
        y_label: Some(b.name.clone()),
        series,
        ..PlotSpec::default()
    })
}

/// Two points of the line `ax·x + ay·y = rhs` (the engine's
/// coefficients), at x = 0 and x = `xmax`, or a vertical line.
fn boundary(coefficients: &[f64], rhs: f64, xmax: f64) -> Option<((f64, f64), (f64, f64))> {
    let [ax, ay] = coefficients else {
        return None;
    };
    if ay.abs() > 1e-12 {
        let at = |vx: f64| (vx, (rhs - ax * vx) / ay);
        Some((at(0.0), at(xmax)))
    } else if ax.abs() > 1e-12 {
        let vx = rhs / ax;
        Some(((vx, 0.0), (vx, xmax * 10.0)))
    } else {
        None
    }
}

/// Draws `body`.
pub fn render(id: &str, body: &OptimizationBody, window: &Window, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let status = match (body.status, body.objective) {
        (OptimizationStatus::Optimal | OptimizationStatus::Feasible, Some(obj)) => {
            let gap = body
                .mip_gap
                .filter(|g| *g > 0.0)
                .map(|g| format!(" · MIP gap {}", apa::number(g * 100.0, 4)))
                .unwrap_or_default();
            format!(
                "{} · objective {} = {}{gap}",
                body.status.label(),
                body.objective_expr,
                num(obj)
            )
        }
        (status, _) => status.label().to_owned(),
    };
    let tone = match body.status {
        OptimizationStatus::Optimal => theme.success,
        OptimizationStatus::Feasible => theme.warning,
        _ => theme.danger,
    };
    let mut column = v_flex()
        .gap_1()
        .child(div().text_color(tone).child(SharedString::from(status)))
        .child(variables_grid(body).render(format!("{id}-variables"), cx));
    if !body.constraints.is_empty() {
        column = column
            .child(constraints_grid(body, theme.info).render(format!("{id}-constraints"), cx));
    }
    if body.variables.iter().all(|v| !v.integer)
        && let Some(spec) = region_plot(body)
    {
        column = column.child(plots(id, vec![("region", spec)], window, cx));
    }
    column.into_any_element()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use domain::computed::{ConstraintOp, OptConstraint, OptVariable, Sense};

    pub(crate) fn body() -> OptimizationBody {
        let var = |name: &str, value| OptVariable {
            name: name.into(),
            value: Some(value),
            lower: Some(0.0),
            upper: None,
            integer: false,
            objective_coefficient: 1.0,
            reduced_cost: Some(0.0),
        };
        let row = |name: &str, expr: &str, a: [f64; 2], rhs, slack, dual| OptConstraint {
            name: name.into(),
            expr: expr.into(),
            coefficients: a.to_vec(),
            op: ConstraintOp::Le,
            rhs,
            activity: Some(rhs - slack),
            slack: Some(slack),
            binding: slack == 0.0,
            dual: Some(dual),
        };
        OptimizationBody {
            sense: Sense::Maximize,
            objective_expr: "3*x1 + 5*x2".into(),
            status: OptimizationStatus::Optimal,
            objective: Some(36.0),
            integer: false,
            mip_gap: Some(0.0),
            variables: vec![var("x1", 2.0), var("x2", 6.0)],
            constraints: vec![
                row("plant1", "x1 <= 4", [1.0, 0.0], 4.0, 2.0, 0.0),
                row("plant2", "2*x2 <= 12", [0.0, 2.0], 12.0, 0.0, 1.5),
                row("plant3", "3*x1 + 2*x2 <= 18", [3.0, 2.0], 18.0, 0.0, 1.0),
            ],
        }
    }

    #[test]
    fn tables_and_the_region_come_from_the_body() {
        let b = body();
        let vars = variables_grid(&b);
        assert_eq!(vars.rows[1][1].text, "6");
        assert_eq!(vars.rows[0][3].text, "∞");
        let rows = constraints_grid(&b, gpui::blue());
        assert_eq!(rows.rows[1][0].text, "plant2 (binding)");
        assert_eq!(rows.rows[1][5].text, "1.5");
        assert!(rows.rows[1][0].bg.is_some() && rows.rows[0][0].bg.is_none());
        let plot = region_plot(&b).unwrap();
        assert_eq!(plot.validate(), Ok(()));
        // plant3: 3 x1 + 2 x2 = 18 → x2 = 9 at x1 = 0.
        assert_eq!(plot.series[2].y[0], 9.0);
        // plant1 is the vertical line x1 = 4.
        assert_eq!(plot.series[0].x, [4.0, 4.0]);
        assert_eq!(plot.series.last().unwrap().x, [2.0]);
    }
}
