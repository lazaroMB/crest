//! An OLS regression: the coefficient table, the fit line, the
//! statsmodels-style diagnostics footer and, when the engine returned the
//! fitted values and residuals, residuals-vs-fitted and normal QQ plots
//! drawn with the plot painter.

use domain::computed::{RegressionBody, apa};
use domain::plot::qq::{normal_qq, qq_line};
use domain::plot::{PlotSpec, SeriesStyle, XySeries};
use gpui::{
    AnyElement, App, InteractiveElement as _, IntoElement as _, ParentElement as _, SharedString,
    Styled as _, Window, div, px,
};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};

use super::grid::Grid;
use super::{auto_decimals, number};

/// The fit statistics as one line: `R² = .76, adj. R² = .75, F(1, 30) =
/// 91.38, p < .001, σ = 3.05, AIC 166.0, BIC 170.4, n = 32`.
pub fn fit_line(body: &RegressionBody) -> String {
    let fit = &body.fit;
    let mut parts = vec![
        format!("R² = {}", apa::bounded(fit.r_squared, 2)),
        format!("adj. R² = {}", apa::bounded(fit.adj_r_squared, 2)),
        format!(
            "F({}, {}) = {}",
            apa::df_text(fit.df_model),
            apa::df_text(fit.df_resid),
            apa::number(fit.f, 2)
        ),
        format!("p {}", {
            let (relation, value) = apa::p_value_parts(fit.f_p);
            format!("{relation} {value}")
        }),
    ];
    if let Some(sigma) = fit.sigma {
        parts.push(format!("σ = {}", apa::significant(sigma, 3)));
    }
    if let Some(aic) = fit.aic {
        parts.push(format!("AIC {}", apa::number(aic, 1)));
    }
    if let Some(bic) = fit.bic {
        parts.push(format!("BIC {}", apa::number(bic, 1)));
    }
    parts.push(format!("n = {}", fit.n));
    parts.join(", ")
}

/// The diagnostics footer, or `None` when the engine computed none.
pub fn diagnostics_line(body: &RegressionBody) -> Option<String> {
    let d = &body.diagnostics;
    let with_p = |name: &str, stat: Option<f64>, p: Option<f64>| {
        stat.map(|s| match p {
            Some(p) => format!("{name} {} (p {})", apa::number(s, 2), {
                let (relation, value) = apa::p_value_parts(p);
                format!("{relation} {value}")
            }),
            None => format!("{name} {}", apa::number(s, 2)),
        })
    };
    let parts: Vec<String> = [
        d.durbin_watson
            .map(|v| format!("Durbin-Watson {}", apa::number(v, 2))),
        with_p("Jarque-Bera", d.jarque_bera, d.jarque_bera_p),
        with_p("Breusch-Pagan", d.breusch_pagan, d.breusch_pagan_p),
        d.condition_number
            .map(|v| format!("Cond. no. {}", apa::significant(v, 3))),
        d.max_vif.map(|v| format!("max VIF {}", apa::number(v, 2))),
        d.max_cooks_d
            .map(|v| format!("max Cook's D {}", apa::number(v, 2))),
    ]
    .into_iter()
    .flatten()
    .collect();
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// Residual plots need at least this many points.
pub const MIN_RESIDUALS: usize = 3;
/// Narrowest a residual plot gets before the two wrap.
const RESIDUAL_PLOT_MIN: f32 = 260.0;

/// The finite (fitted, residual) pairs of `body`.
fn residual_pairs(body: &RegressionBody) -> Vec<(f64, f64)> {
    body.fitted
        .iter()
        .zip(&body.residuals)
        .map(|(&f, &r)| (f, r))
        .filter(|(f, r)| f.is_finite() && r.is_finite())
        .collect()
}

fn series(name: &str, points: &[(f64, f64)], style: SeriesStyle) -> XySeries {
    XySeries {
        name: name.to_owned(),
        x: points.iter().map(|p| p.0).collect(),
        y: points.iter().map(|p| p.1).collect(),
        style,
    }
}

/// Residuals against fitted values, around the zero line the axes draw;
/// `None` with fewer than [`MIN_RESIDUALS`] points.
pub fn residuals_plot(body: &RegressionBody) -> Option<PlotSpec> {
    let pairs = residual_pairs(body);
    (pairs.len() >= MIN_RESIDUALS).then(|| PlotSpec {
        title: Some("Residuals vs fitted".into()),
        x_range: None,
        y_range: None,
        x_label: Some(format!("Fitted {}", body.dependent)),
        y_label: Some("Residual".into()),
        functions: Vec::new(),
        params: Vec::new(),
        series: vec![series("residuals", &pairs, SeriesStyle::Scatter)],
    })
}

/// The normal QQ plot of the residuals with the line through their
/// quartiles; `None` with fewer than [`MIN_RESIDUALS`] points.
pub fn qq_plot(body: &RegressionBody) -> Option<PlotSpec> {
    let residuals: Vec<f64> = residual_pairs(body).into_iter().map(|p| p.1).collect();
    if residuals.len() < MIN_RESIDUALS {
        return None;
    }
    let points = normal_qq(&residuals);
    let mut all = vec![series("residuals", &points, SeriesStyle::Scatter)];
    if let (Some((intercept, slope)), Some(first), Some(last)) =
        (qq_line(&points), points.first(), points.last())
    {
        let ends = [first.0, last.0].map(|x| (x, intercept + slope * x));
        all.push(series("normal reference", &ends, SeriesStyle::Line));
    }
    Some(PlotSpec {
        title: Some("Normal QQ".into()),
        x_range: None,
        y_range: None,
        x_label: Some("Theoretical quantile".into()),
        y_label: Some("Residual".into()),
        functions: Vec::new(),
        params: Vec::new(),
        series: all,
    })
}

/// The two residual plots side by side (wrapping when narrow).
fn residual_plots(
    id: &str,
    body: &RegressionBody,
    window: &Window,
    cx: &App,
) -> Option<AnyElement> {
    let plots: Vec<(&str, PlotSpec)> = [("residuals", residuals_plot(body)), ("qq", qq_plot(body))]
        .into_iter()
        .filter_map(|(name, spec)| spec.map(|s| (name, s)))
        .collect();
    if plots.is_empty() {
        return None;
    }
    let muted = cx.theme().muted_foreground;
    Some(
        h_flex()
            .debug_selector(|| "regression-plots".to_owned())
            .w_full()
            .flex_wrap()
            .gap_3()
            .items_start()
            .children(plots.into_iter().map(|(name, spec)| {
                let title = spec.title.clone().unwrap_or_default();
                v_flex()
                    .flex_grow_1()
                    .flex_shrink_1()
                    .flex_basis(px(0.))
                    .min_w(px(RESIDUAL_PLOT_MIN))
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
            .into_any_element(),
    )
}

/// Draws `body`.
pub fn render(id: &str, body: &RegressionBody, window: &Window, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let level = apa::number(body.ci_level * 100.0, 0);
    let mut table = Grid::new([
        format!("{} ~", body.dependent),
        "Estimate".into(),
        "SE".into(),
        "t".into(),
        "p".into(),
        format!("{level}% CI lo"),
        format!("{level}% CI hi"),
        "β".into(),
        "VIF".into(),
    ]);
    for c in &body.coefficients {
        let decimals = auto_decimals(c.se);
        table.push([
            c.term.clone(),
            number(c.estimate, decimals),
            number(c.se, decimals),
            number(c.t, 2),
            apa::p_value_text(c.p),
            number(c.ci_lo, decimals),
            number(c.ci_hi, decimals),
            c.std_beta.map(|b| number(b, 2)).unwrap_or_default(),
            c.vif.map(|v| number(v, 2)).unwrap_or_default(),
        ]);
    }
    let mut column = v_flex()
        .gap_1()
        .child(table.render(format!("{id}-coefficients"), cx))
        .child(div().text_sm().child(SharedString::from(fit_line(body))));
    if let Some(line) = diagnostics_line(body) {
        column = column.child(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(SharedString::from(line)),
        );
    }
    if let Some(plots) = residual_plots(id, body, window, cx) {
        column = column.child(plots);
    }
    column.into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::computed::{Diagnostics, FitStats};

    fn body(fitted: Vec<f64>, residuals: Vec<f64>) -> RegressionBody {
        RegressionBody {
            dependent: "mpg".into(),
            ci_level: 0.95,
            coefficients: Vec::new(),
            fit: FitStats {
                n: fitted.len() as u64,
                r_squared: 0.5,
                adj_r_squared: 0.5,
                f: 1.0,
                df_model: 1.0,
                df_resid: 1.0,
                f_p: 0.5,
                sigma: None,
                aic: None,
                bic: None,
            },
            diagnostics: Diagnostics::default(),
            fitted,
            residuals,
        }
    }

    #[test]
    fn residual_plots_come_from_the_engine_values() {
        let b = body(vec![1.0, 2.0, 3.0, f64::NAN], vec![0.5, -1.0, 0.25, 9.0]);
        let residuals = residuals_plot(&b).unwrap();
        assert_eq!(residuals.series[0].x, [1.0, 2.0, 3.0]);
        assert_eq!(residuals.series[0].y, [0.5, -1.0, 0.25]);
        assert_eq!(residuals.validate(), Ok(()));
        let qq = qq_plot(&b).unwrap();
        assert_eq!(qq.series[0].y, [-1.0, 0.25, 0.5]);
        assert_eq!(qq.series[1].style, SeriesStyle::Line);
        assert_eq!(qq.validate(), Ok(()));
        assert_eq!(residuals_plot(&body(vec![1.0], vec![0.0])), None);
        assert_eq!(qq_plot(&body(Vec::new(), Vec::new())), None);
    }
}
