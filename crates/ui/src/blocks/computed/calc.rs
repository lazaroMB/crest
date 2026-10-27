//! The calc sheet and the quantity card.
//!
//! A sheet reads like a Mathcad worksheet: one row per line, the line
//! typeset as math (`E = 0.5 · m · v² = 462.963 kJ`) when the engine gave
//! its TeX, else `name = expr → value` in the monospace font; the
//! dimension at the right; an error in red under the line it belongs to;
//! chips saying how ambiguous units were read (`ton` → tonne).
//!
//! The quantity card shows `from → to`, the factor with an exact or
//! rounded badge, the dimension and the chain through SI units.

use domain::computed::{CalcLine, CalcSheetBody, QuantityBody};
use gpui::{
    AnyElement, App, Hsla, InteractiveElement as _, IntoElement as _, ParentElement as _,
    SharedString, Styled as _, Window, div,
};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};

use crate::blocks::math;

/// Code of the warning the engine adds per failed line; the sheet shows
/// those errors on their lines.
pub const LINE_ERROR: &str = "line_error";

/// `x` with at most ten significant digits, without trailing zeros, in
/// scientific notation when very small or large.
pub fn plain(x: f64) -> String {
    if !x.is_finite() {
        return x.to_string();
    }
    let rounded: f64 = format!("{x:.9e}").parse().unwrap_or(x);
    let magnitude = rounded.abs();
    let text = if magnitude != 0.0 && !(1e-4..1e15).contains(&magnitude) {
        format!("{rounded:e}")
    } else {
        rounded.to_string()
    };
    text.replace('-', "−")
}

/// A small rounded label.
fn chip(text: impl Into<SharedString>, color: Hsla) -> gpui::Div {
    div()
        .flex_none()
        .px_1()
        .rounded_sm()
        .text_xs()
        .bg(color.opacity(0.12))
        .text_color(color)
        .child(text.into())
}

/// `name = expr`, or the bare expression.
fn written(line: &CalcLine) -> String {
    match &line.name {
        Some(name) => format!("{name} = {}", line.expr),
        None => line.expr.clone(),
    }
}

fn sheet_line(line: &CalcLine, window: &mut Window, cx: &mut App) -> AnyElement {
    let theme = cx.theme();
    let (mono, muted, danger, warning) = (
        theme.mono_font_family.clone(),
        theme.muted_foreground,
        theme.danger,
        theme.warning,
    );
    let main = match (&line.error, &line.latex) {
        (None, Some(latex)) => div()
            .debug_selector(|| "calc-math".to_owned())
            .child(math::element(latex, false, window, cx))
            .into_any_element(),
        (None, None) => {
            let value = line.display.clone().unwrap_or_default();
            div()
                .font_family(mono.clone())
                .child(SharedString::from(format!("{}  →  {value}", written(line))))
                .into_any_element()
        }
        (Some(_), _) => div()
            .font_family(mono.clone())
            .child(SharedString::from(written(line)))
            .into_any_element(),
    };
    let mut row = h_flex()
        .w_full()
        .gap_3()
        .items_center()
        .child(div().flex_1().min_w_0().child(main));
    if let Some(dimension) = &line.dimension {
        row = row.child(
            div()
                .flex_none()
                .text_xs()
                .text_color(muted)
                .child(SharedString::from(dimension.clone())),
        );
    }
    let mut block = v_flex()
        .debug_selector(|| "calc-line".to_owned())
        .gap_0p5()
        .py_0p5()
        .child(row);
    if let Some(error) = &line.error {
        block = block.child(
            div()
                .debug_selector(|| "calc-error".to_owned())
                .pl_4()
                .text_sm()
                .text_color(danger)
                .child(SharedString::from(format!("⚠ {error}"))),
        );
    }
    if !line.notes.is_empty() {
        block = block.child(
            h_flex().pl_4().gap_1().flex_wrap().children(
                line.notes.iter().map(|note| {
                    chip(note.clone(), warning).debug_selector(|| "calc-note".to_owned())
                }),
            ),
        );
    }
    block.into_any_element()
}

/// Draws a calc sheet.
pub fn calc_sheet(body: &CalcSheetBody, window: &mut Window, cx: &mut App) -> AnyElement {
    let border = cx.theme().border;
    let mut sheet = v_flex()
        .debug_selector(|| "calc-sheet".to_owned())
        .w_full()
        .gap_0p5();
    for (ix, line) in body.lines.iter().enumerate() {
        let row = sheet_line(line, window, cx);
        sheet = sheet.child(if ix == 0 {
            div().child(row)
        } else {
            div()
                .border_t_1()
                .border_color(border.opacity(0.5))
                .child(row)
        });
    }
    sheet.into_any_element()
}

/// `value unit`, with `± u` when there is an uncertainty.
fn amount(value: f64, unit: &str, uncertainty: Option<f64>) -> String {
    let mut text = plain(value);
    if let Some(u) = uncertainty {
        text = format!("{text} ± {}", plain(u));
    }
    if unit.is_empty() {
        text
    } else {
        format!("{text} {unit}")
    }
}

/// Draws a conversion: from → to, the factor with its exactness, the
/// dimension and the chain.
pub fn quantity(body: &QuantityBody, result: Option<String>, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let to = result.unwrap_or_else(|| amount(body.to.value, &body.to.unit, body.to.uncertainty));
    let to = match body.to.uncertainty {
        Some(u) if !to.contains('±') => format!("{to} (± {})", plain(u)),
        _ => to,
    };
    let headline = h_flex()
        .gap_3()
        .items_baseline()
        .font_family(theme.mono_font_family.clone())
        .text_lg()
        .child(SharedString::from(amount(
            body.from.value,
            &body.from.unit,
            body.from.uncertainty,
        )))
        .child(div().text_color(theme.muted_foreground).child("→"))
        .child(
            div()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child(SharedString::from(to)),
        );
    let (label, color, selector) = if body.exact {
        ("exact", theme.success, "quantity-exact")
    } else {
        ("rounded", theme.warning, "quantity-rounded")
    };
    let factor = h_flex()
        .gap_2()
        .items_center()
        .text_sm()
        .child(SharedString::from(format!(
            "1 {} = {} {}",
            body.from.unit,
            plain(body.factor),
            body.to.unit
        )))
        .child(chip(label, color).debug_selector(move || selector.to_owned()))
        .child(
            div()
                .text_color(theme.muted_foreground)
                .child(SharedString::from(body.dimension.clone())),
        );
    let mut card = v_flex().gap_1().child(headline).child(factor);
    if body.chain.len() > 1 || body.chain.iter().any(|s| s.note.is_some()) {
        let mut chain = h_flex()
            .debug_selector(|| "quantity-chain".to_owned())
            .gap_1()
            .flex_wrap()
            .items_center()
            .text_xs()
            .font_family(theme.mono_font_family.clone())
            .text_color(theme.muted_foreground);
        if let Some(first) = body.chain.first() {
            chain = chain.child(SharedString::from(first.from_unit.clone()));
        }
        for step in &body.chain {
            let note = step
                .note
                .as_deref()
                .map(|n| format!(", {n}"))
                .unwrap_or_default();
            chain = chain
                .child(SharedString::from(format!(
                    "—×{}{note}→",
                    plain(step.factor)
                )))
                .child(SharedString::from(step.to_unit.clone()));
        }
        card = card.child(chain);
    }
    card.into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_numbers_drop_float_noise() {
        assert_eq!(plain(30.479_999_999_999_997), "30.48");
        assert_eq!(plain(1.609_344), "1.609344");
        assert_eq!(plain(1.602_176_634e-19), "1.602176634e−19");
        assert_eq!(plain(-3.0), "−3");
        assert_eq!(amount(2.0, "in", Some(0.1)), "2 ± 0.1 in");
    }
}
