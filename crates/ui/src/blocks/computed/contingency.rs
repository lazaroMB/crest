//! The contingency-table card: observed counts with the expected ones,
//! row and column percentages, and the adjusted standardised residuals as
//! a diverging heatmap (|r| > 2 marks a cell that departs from
//! independence), all from the engine.

use domain::computed::ContingencyBody;
use gpui::{AnyElement, App, IntoElement as _, ParentElement as _, SharedString, Styled as _, div};
use gpui_component::{ActiveTheme as _, v_flex};

use super::correlation::cell_color;
use super::grid::{Grid, GridCell};
use super::number;

/// Residuals at or beyond this size are drawn at full strength.
const RESIDUAL_SCALE: f64 = 4.0;

fn header(body: &ContingencyBody) -> Vec<String> {
    let corner = match (&body.row_variable, &body.column_variable) {
        (Some(r), Some(c)) => format!("{r} \\ {c}"),
        _ => String::new(),
    };
    let mut headers = vec![corner];
    headers.extend(body.column_levels.iter().cloned());
    headers
}

/// The counts: `observed (expected)` per cell and the row totals.
pub fn counts_grid(body: &ContingencyBody) -> Grid {
    let mut headers = header(body);
    headers.push("Total".into());
    let mut grid = Grid::new(headers);
    let smallest = body
        .expected
        .iter()
        .flatten()
        .copied()
        .fold(f64::INFINITY, f64::min);
    let expected_decimals = if smallest < 1.0 { 2 } else { 1 };
    let whole = |x: f64| {
        if x.fract() == 0.0 {
            number(x, 0)
        } else {
            number(x, 2)
        }
    };
    for (i, label) in body.row_levels.iter().enumerate() {
        let mut row = vec![label.clone()];
        for (o, e) in body.observed[i].iter().zip(&body.expected[i]) {
            row.push(format!("{} ({})", whole(*o), number(*e, expected_decimals)));
        }
        row.push(whole(body.observed[i].iter().sum()));
        grid.push(row);
    }
    let mut totals = vec!["Total".to_owned()];
    let cols = body.column_levels.len();
    let col_sums: Vec<f64> = (0..cols)
        .map(|j| body.observed.iter().map(|r| r[j]).sum())
        .collect();
    totals.extend(col_sums.iter().map(|x| whole(*x)));
    totals.push(whole(col_sums.iter().sum()));
    grid.push(totals);
    grid
}

/// Row and column percentages as `row% / col%`.
pub fn percent_grid(body: &ContingencyBody) -> Grid {
    let mut grid = Grid::new(header(body));
    for (i, label) in body.row_levels.iter().enumerate() {
        let mut row = vec![label.clone()];
        for (r, c) in body.row_percent[i].iter().zip(&body.column_percent[i]) {
            row.push(format!("{} / {}", number(*r, 1), number(*c, 1)));
        }
        grid.push(row);
    }
    grid
}

/// The residual heatmap: `positive` above independence, `negative`
/// below, stronger is more opaque.
pub fn residual_grid(body: &ContingencyBody, positive: gpui::Hsla, negative: gpui::Hsla) -> Grid {
    let mut grid = Grid::new(header(body));
    for (i, label) in body.row_levels.iter().enumerate() {
        let mut row = vec![GridCell::from(label.as_str())];
        for &r in &body.std_residuals[i] {
            row.push(GridCell {
                text: number(r, 2),
                bg: Some(cell_color(r / RESIDUAL_SCALE, positive, negative)),
                fg: None,
            });
        }
        grid.rows.push(row);
    }
    grid
}

/// Draws `body`.
pub fn render(id: &str, body: &ContingencyBody, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    let caption = |text: String| {
        div()
            .text_xs()
            .text_color(muted)
            .child(SharedString::from(text))
    };
    let large = body
        .std_residuals
        .iter()
        .flatten()
        .filter(|r| r.abs() > 2.0)
        .count();
    v_flex()
        .gap_1()
        .child(caption(format!(
            "Observed (expected) counts · {}",
            body.test.label()
        )))
        .child(counts_grid(body).render(format!("{id}-counts"), cx))
        .child(caption("Row % / column %".into()))
        .child(percent_grid(body).render(format!("{id}-percent"), cx))
        .child(caption(format!(
            "Adjusted standardised residuals: {large} cell{} beyond ±2 (blue above, red below \
             independence)",
            if large == 1 { "" } else { "s" }
        )))
        .child(residual_grid(body, theme.blue, theme.red).render(format!("{id}-residuals"), cx))
        .into_any_element()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use domain::computed::ContingencyTest;

    pub(crate) fn body() -> ContingencyBody {
        ContingencyBody {
            row_variable: Some("gender".into()),
            column_variable: Some("party".into()),
            row_levels: vec!["F".into(), "M".into()],
            column_levels: vec!["D".into(), "R".into()],
            observed: vec![vec![762.0, 468.0], vec![484.0, 477.0]],
            expected: vec![vec![699.8, 530.2], vec![546.2, 414.8]],
            row_percent: vec![vec![62.0, 38.0], vec![50.4, 49.6]],
            column_percent: vec![vec![61.2, 49.5], vec![38.8, 50.5]],
            std_residuals: vec![vec![5.5, -5.5], vec![-5.5, 5.5]],
            test: ContingencyTest::ChiSquare,
        }
    }

    #[test]
    fn grids_show_engine_counts_and_totals() {
        let b = body();
        let counts = counts_grid(&b);
        assert_eq!(counts.headers[0], "gender \\ party");
        assert_eq!(counts.rows[0][1].text, "762 (699.8)");
        assert_eq!(counts.rows[2][3].text, "2191");
        assert_eq!(percent_grid(&b).rows[1][2].text, "49.6 / 50.5");
        let res = residual_grid(&b, gpui::red(), gpui::blue());
        assert_eq!(res.rows[0][2].text, "−5.50");
        assert!(res.rows[0][1].bg.is_some());
    }
}
