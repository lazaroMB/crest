//! An OLS regression: the coefficient table, the fit line and the
//! statsmodels-style diagnostics footer.

use domain::computed::{RegressionBody, apa};
use gpui::{AnyElement, App, IntoElement as _, ParentElement as _, SharedString, Styled as _, div};
use gpui_component::{ActiveTheme as _, v_flex};

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

/// Draws `body`.
pub fn render(id: &str, body: &RegressionBody, cx: &App) -> AnyElement {
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
    // TODO(piece 7): residuals-vs-fitted and QQ plots from `body.fitted`
    // and `body.residuals`, drawn with the plot painter.
    column.into_any_element()
}
