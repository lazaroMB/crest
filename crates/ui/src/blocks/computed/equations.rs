//! The equation-solver card: the solution, the plug-back table (each
//! equation's two sides at the solution and their difference), how the
//! solver got there, and for one unknown the curve of `lhs − rhs` with its
//! roots.

use domain::computed::{EquationsBody, apa};
use domain::plot::{PlotSpec, SeriesStyle, XySeries};
use gpui::{
    AnyElement, App, IntoElement as _, ParentElement as _, SharedString, Styled as _, Window, div,
};
use gpui_component::{ActiveTheme as _, v_flex};

use super::distribution::plots;
use super::grid::Grid;

/// Ten significant digits without trailing zeros: `2`, `1.414213562`.
fn sig(x: f64) -> String {
    let text = apa::significant(x, 10);
    if text.contains('.') && !text.contains('e') {
        text.trim_end_matches('0').trim_end_matches('.').to_owned()
    } else {
        text
    }
}

/// The unknowns, or every root of a scan.
pub fn solution_grid(body: &EquationsBody) -> Grid {
    let mut grid = Grid::new(["Unknown", "Value"]);
    if body.roots.len() > 1 {
        let name = body.unknowns.first().map_or("x", |u| u.name.as_str());
        for (i, r) in body.roots.iter().enumerate() {
            grid.push([format!("{name} (root {})", i + 1), sig(*r)]);
        }
    } else {
        for u in &body.unknowns {
            grid.push([u.name.clone(), sig(u.value)]);
        }
    }
    grid
}

/// Each equation's sides at the solution.
pub fn plug_back_grid(body: &EquationsBody) -> Grid {
    let mut grid = Grid::new(["Equation", "LHS", "RHS", "LHS − RHS"]);
    for p in &body.plug_back {
        grid.push([
            p.equation.clone(),
            sig(p.lhs),
            sig(p.rhs),
            apa::significant(p.residual, 3),
        ]);
    }
    grid
}

/// `lhs − rhs` over the range, the zero line and the roots.
pub fn curve_plot(body: &EquationsBody) -> Option<PlotSpec> {
    let curve = body.curve.as_ref().filter(|c| c.x.len() >= 2)?;
    let name = body.unknowns.first().map_or("x", |u| u.name.as_str());
    let (first, last) = (curve.x[0], curve.x[curve.x.len() - 1]);
    let roots: Vec<f64> = if body.roots.is_empty() {
        body.unknowns
            .iter()
            .map(|u| u.value)
            .filter(|v| v.is_finite())
            .collect()
    } else {
        body.roots.clone()
    };
    let mut series = vec![
        XySeries {
            name: "lhs − rhs".into(),
            x: curve.x.clone(),
            y: curve.y.clone(),
            style: SeriesStyle::Line,
        },
        XySeries {
            name: "0".into(),
            x: vec![first, last],
            y: vec![0.0, 0.0],
            style: SeriesStyle::Line,
        },
    ];
    if body.converged && !roots.is_empty() {
        series.push(XySeries {
            name: "roots".into(),
            y: vec![0.0; roots.len()],
            x: roots,
            style: SeriesStyle::Scatter,
        });
    }
    Some(PlotSpec {
        title: Some(format!("lhs − rhs over {name}")),
        x_label: Some(name.to_owned()),
        y_label: Some("lhs − rhs".into()),
        series,
        ..PlotSpec::default()
    })
}

/// Draws `body`.
pub fn render(id: &str, body: &EquationsBody, window: &Window, cx: &App) -> AnyElement {
    let mut facts = vec![
        body.method.label().to_owned(),
        format!("{} iterations", body.iterations),
        format!(
            "largest residual {}",
            apa::significant(body.residual_norm, 3)
        ),
        format!("tolerance {}", apa::significant(body.tolerance, 2)),
    ];
    if let Some(c) = body.condition_number {
        facts.push(format!("condition number {}", apa::significant(c, 3)));
    }
    let theme = cx.theme();
    let mut column = v_flex()
        .gap_1()
        .child(solution_grid(body).render(format!("{id}-solution"), cx));
    if !body.plug_back.is_empty() {
        column = column.child(plug_back_grid(body).render(format!("{id}-plug-back"), cx));
    }
    column = column.child(
        div()
            .text_xs()
            .text_color(if body.converged {
                theme.muted_foreground
            } else {
                theme.danger
            })
            .child(SharedString::from(facts.join(" · "))),
    );
    if let Some(spec) = curve_plot(body) {
        column = column.child(plots(id, vec![("curve", spec)], window, cx));
    }
    column.into_any_element()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use domain::computed::{Curve, PlugBack, SolveMethod, SolvedUnknown};

    pub(crate) fn body() -> EquationsBody {
        EquationsBody {
            method: SolveMethod::BracketScan,
            converged: true,
            unknowns: vec![SolvedUnknown {
                name: "x".into(),
                value: -std::f64::consts::SQRT_2,
            }],
            roots: vec![-std::f64::consts::SQRT_2, std::f64::consts::SQRT_2],
            plug_back: vec![PlugBack {
                equation: "x^2 = 2".into(),
                lhs: 2.0,
                rhs: 2.0,
                residual: 4e-16,
            }],
            residual_norm: 4e-16,
            iterations: 104,
            tolerance: 1e-12,
            condition_number: None,
            curve: Some(Curve {
                x: vec![-3.0, 0.0, 3.0],
                y: vec![7.0, -2.0, 7.0],
            }),
        }
    }

    #[test]
    fn every_root_is_listed_and_marked() {
        let b = body();
        let grid = solution_grid(&b);
        assert_eq!(grid.rows.len(), 2);
        assert_eq!(grid.rows[1][0].text, "x (root 2)");
        assert_eq!(plug_back_grid(&b).rows[0][1].text, "2");
        let plot = curve_plot(&b).unwrap();
        assert_eq!(plot.validate(), Ok(()));
        assert_eq!(plot.series[2].x.len(), 2);
    }
}
