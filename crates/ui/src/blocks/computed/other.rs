//! Simple renderings of the calc sheet and the quantity card, until their
//! own cards land (piece 5): one monospace line per sheet line, and the
//! conversion with its factor.

use domain::computed::{CalcSheetBody, QuantityBody, apa};
use gpui::{AnyElement, App, IntoElement as _, ParentElement as _, SharedString, Styled as _, div};
use gpui_component::{ActiveTheme as _, v_flex};

/// Draws a calc sheet: `name = expr → value`, errors in red.
pub fn calc_sheet(body: &CalcSheetBody, cx: &App) -> AnyElement {
    let theme = cx.theme();
    v_flex()
        .font_family(theme.mono_font_family.clone())
        .text_sm()
        .children(body.lines.iter().map(|line| {
            let lhs = match &line.name {
                Some(name) => format!("{name} = {}", line.expr),
                None => line.expr.clone(),
            };
            match &line.error {
                Some(error) => div()
                    .text_color(theme.danger)
                    .child(SharedString::from(format!("{lhs}  ⚠ {error}"))),
                None => {
                    let value = line.display.clone().unwrap_or_else(|| {
                        let number = line
                            .value
                            .map(|v| apa::significant(v, 6))
                            .unwrap_or_default();
                        match &line.unit {
                            Some(unit) => format!("{number} {unit}"),
                            None => number,
                        }
                    });
                    div().child(SharedString::from(format!("{lhs}  →  {value}")))
                }
            }
        }))
        .into_any_element()
}

/// Draws a conversion: `from = to`, the factor and whether it is exact.
pub fn quantity(body: &QuantityBody, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let exact = if body.exact { "exact" } else { "rounded" };
    v_flex()
        .gap_0p5()
        .child(
            div()
                .font_family(theme.mono_font_family.clone())
                .child(SharedString::from(format!(
                    "{} {} = {} {}",
                    apa::significant(body.from.value, 6),
                    body.from.unit,
                    apa::significant(body.to.value, 6),
                    body.to.unit
                ))),
        )
        .child(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(SharedString::from(format!(
                    "factor {} ({exact}) · {}",
                    apa::significant(body.factor, 8),
                    body.dimension
                ))),
        )
        .into_any_element()
}
