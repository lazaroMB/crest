//! A correlation matrix as a heatmap: r in each cell on a diverging
//! scale (blue positive, red negative, stronger is more opaque), stars
//! for the adjusted p, and the method, adjustment and pairwise n below.

use domain::computed::{CorrelationBody, CorrelationMethod, PAdjust, apa};
use gpui::{
    AnyElement, App, Hsla, IntoElement as _, ParentElement as _, SharedString, Styled as _, div,
};
use gpui_component::{ActiveTheme as _, v_flex};

use super::grid::{Grid, GridCell};

/// Stars of an adjusted p-value.
pub fn stars(p: f64) -> &'static str {
    match p {
        p if p < 0.001 => "***",
        p if p < 0.01 => "**",
        p if p < 0.05 => "*",
        _ => "",
    }
}

/// The heatmap color of `r`: `positive` or `negative` with an opacity
/// that grows with |r|.
pub fn cell_color(r: f64, positive: Hsla, negative: Hsla) -> Hsla {
    #[allow(clippy::cast_possible_truncation)]
    let strength = r.abs().clamp(0.0, 1.0) as f32;
    let base = if r >= 0.0 { positive } else { negative };
    base.opacity(0.08 + 0.6 * strength)
}

fn method_name(method: CorrelationMethod) -> &'static str {
    match method {
        CorrelationMethod::Pearson => "Pearson r",
        CorrelationMethod::Spearman => "Spearman ρ",
        CorrelationMethod::Kendall => "Kendall τ-b",
    }
}

fn adjustment_name(adjustment: PAdjust) -> &'static str {
    match adjustment {
        PAdjust::None => "no adjustment",
        PAdjust::Holm => "Holm-adjusted",
        PAdjust::Bh => "Benjamini-Hochberg-adjusted",
        PAdjust::Bonferroni => "Bonferroni-adjusted",
    }
}

/// Draws `body`.
pub fn render(id: &str, body: &CorrelationBody, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let (positive, negative) = (theme.blue, theme.red);
    let mut headers = vec![String::new()];
    headers.extend(body.variables.iter().cloned());
    let mut grid = Grid::new(headers);
    for (i, name) in body.variables.iter().enumerate() {
        let mut row = vec![GridCell::from(name.as_str())];
        for j in 0..body.variables.len() {
            let r = body.r.get(i).and_then(|row| row.get(j)).copied().flatten();
            let p = body.p.get(i).and_then(|row| row.get(j)).copied().flatten();
            row.push(match (i == j, r) {
                (true, _) => GridCell {
                    text: "—".into(),
                    fg: Some(theme.muted_foreground),
                    ..GridCell::default()
                },
                (false, Some(r)) => GridCell {
                    text: format!("{}{}", apa::bounded(r, 2), p.map(stars).unwrap_or("")),
                    bg: Some(cell_color(r, positive, negative)),
                    fg: None,
                },
                (false, None) => GridCell {
                    text: "n/a".into(),
                    fg: Some(theme.muted_foreground),
                    ..GridCell::default()
                },
            });
        }
        grid.rows.push(row);
    }
    let pairs: Vec<u64> = body
        .n
        .iter()
        .enumerate()
        .flat_map(|(i, row)| row.iter().skip(i + 1).copied())
        .collect();
    let n = match (pairs.iter().min(), pairs.iter().max()) {
        (Some(lo), Some(hi)) if lo == hi => format!("n = {lo} per pair"),
        (Some(lo), Some(hi)) => format!("pairwise n {lo}–{hi}"),
        _ => String::new(),
    };
    let legend = format!(
        "{} · {} p: * < .05, ** < .01, *** < .001 · {n}",
        method_name(body.method),
        adjustment_name(body.adjustment)
    );
    v_flex()
        .gap_1()
        .child(grid.render(format!("{id}-matrix"), cx))
        .child(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(SharedString::from(legend)),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stars_and_colors_follow_p_and_r() {
        assert_eq!(stars(0.0004), "***");
        assert_eq!(stars(0.02), "*");
        assert_eq!(stars(0.2), "");
        let blue = gpui::hsla(0.6, 0.5, 0.5, 1.0);
        let red = gpui::hsla(0.0, 0.5, 0.5, 1.0);
        let strong = cell_color(0.9, blue, red);
        let weak = cell_color(0.1, blue, red);
        assert!(strong.a > weak.a);
        assert_eq!(cell_color(-0.5, blue, red).h, red.h);
    }
}
