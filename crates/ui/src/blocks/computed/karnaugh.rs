//! The Karnaugh-map card: the map (or two or four 4×4 maps for five and
//! six variables) in Gray-code order with 1, 0 and × cells, a colored
//! stripe in each cell per chosen group covering it, the legend of the
//! groups (essential ones marked), the minimal SOP and POS, the prime
//! implicants and, up to four variables, the truth table.

use domain::computed::karnaugh::{CellValue, KMap};
use domain::computed::{Implicant, KarnaughBody};
use gpui::{
    AnyElement, App, FontWeight, Hsla, IntoElement as _, ParentElement as _, SharedString,
    Styled as _, div, prelude::FluentBuilder as _, px,
};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};

use super::grid::Grid;
use crate::widgets::palette;

/// Side of a map cell.
const CELL: f32 = 34.0;
/// Most variables whose truth table the card draws.
pub const TABLE_VARIABLES: usize = 4;

/// The chosen SOP groups covering minterm `m`, by index.
#[must_use]
pub fn groups_at(body: &KarnaughBody, m: u32) -> Vec<usize> {
    body.sop
        .iter()
        .enumerate()
        .filter(|(_, i)| i.covers_minterm(m))
        .map(|(ix, _)| ix)
        .collect()
}

/// The text of a cell: `1`, `0` or `×`.
#[must_use]
pub fn cell_text(value: CellValue) -> &'static str {
    match value {
        CellValue::One => "1",
        CellValue::Zero => "0",
        CellValue::DontCare => "×",
    }
}

/// The prime implicants table.
pub fn primes_grid(body: &KarnaughBody) -> Grid {
    let mut grid = Grid::new(["Prime implicant", "Covers", "Essential", "Chosen"]);
    for p in &body.prime_implicants {
        let chosen = body
            .sop
            .iter()
            .any(|s| s.care == p.care && s.value == p.value);
        grid.push([
            p.term.clone(),
            p.covers
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(","),
            if p.essential { "yes" } else { "" }.to_owned(),
            if chosen { "✓" } else { "" }.to_owned(),
        ]);
    }
    grid
}

/// The truth table (variables, F).
pub fn truth_grid(body: &KarnaughBody) -> Grid {
    let n = body.variables.len();
    let mut headers: Vec<String> = vec!["m".to_owned()];
    headers.extend(body.variables.iter().cloned());
    headers.push("F".to_owned());
    let mut grid = Grid::new(headers);
    for m in 0..body.size() {
        let mut row = vec![m.to_string()];
        for b in (0..n).rev() {
            row.push(((m >> b) & 1).to_string());
        }
        row.push(cell_text(body.value(m)).to_owned());
        grid.push(row);
    }
    grid
}

fn map_view(map: &KMap, body: &KarnaughBody, colors: &[Hsla], cx: &App) -> AnyElement {
    let theme = cx.theme();
    let mono = theme.mono_font_family.clone();
    let header = h_flex()
        .child(
            div()
                .w(px(CELL * 1.4))
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(SharedString::from(format!(
                    "{}\\{}",
                    map.row_vars, map.col_vars
                ))),
        )
        .children(map.cols.iter().map(|c| {
            div()
                .w(px(CELL))
                .text_xs()
                .text_center()
                .text_color(theme.muted_foreground)
                .font_family(mono.clone())
                .child(SharedString::from(c.clone()))
        }));
    let rows = map.rows.iter().zip(&map.cells).map(|(label, cells)| {
        h_flex()
            .child(
                div()
                    .w(px(CELL * 1.4))
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .font_family(mono.clone())
                    .child(SharedString::from(label.clone())),
            )
            .children(cells.iter().map(|m| {
                let value = body.value(*m);
                let groups = groups_at(body, *m);
                let bg = match value {
                    CellValue::One => theme.success.opacity(0.12),
                    CellValue::DontCare => theme.muted.opacity(0.6),
                    CellValue::Zero => theme.background,
                };
                v_flex()
                    .w(px(CELL))
                    .h(px(CELL))
                    .border_1()
                    .border_color(theme.border)
                    .bg(bg)
                    .child(
                        div()
                            .flex_1()
                            .text_center()
                            .font_family(mono.clone())
                            .font_weight(if value == CellValue::One {
                                FontWeight::SEMIBOLD
                            } else {
                                FontWeight::NORMAL
                            })
                            .child(cell_text(value)),
                    )
                    .child(
                        h_flex().h(px(4.0)).gap_px().children(
                            groups
                                .iter()
                                .map(|g| div().flex_1().h_full().bg(colors[*g % colors.len()])),
                        ),
                    )
                    .child(
                        div()
                            .text_size(px(8.0))
                            .text_color(theme.muted_foreground)
                            .text_right()
                            .pr_0p5()
                            .child(SharedString::from(m.to_string())),
                    )
            }))
    });
    v_flex()
        .gap_0p5()
        .when(!map.label.is_empty(), |d| {
            d.child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(SharedString::from(map.label.clone())),
            )
        })
        .child(header)
        .children(rows)
        .into_any_element()
}

fn legend(groups: &[Implicant], colors: &[Hsla], cx: &App) -> AnyElement {
    let theme = cx.theme();
    h_flex()
        .flex_wrap()
        .gap_3()
        .text_xs()
        .children(groups.iter().enumerate().map(|(ix, g)| {
            h_flex()
                .gap_1()
                .child(div().w(px(10.0)).h(px(10.0)).bg(colors[ix % colors.len()]))
                .child(SharedString::from(g.term.clone()))
                .child(
                    div()
                        .text_color(theme.muted_foreground)
                        .child(if g.essential { "essential" } else { "" }),
                )
        }))
        .into_any_element()
}

/// Draws `body`.
pub fn render(id: &str, body: &KarnaughBody, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let colors: Vec<Hsla> = (0..8).map(|slot| palette(slot, cx)).collect();
    let mono = theme.mono_font_family.clone();
    let expression = |label: &str, text: &str| {
        h_flex()
            .gap_2()
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .w(px(36.0))
                    .child(SharedString::from(label.to_owned())),
            )
            .child(
                div()
                    .font_family(mono.clone())
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(SharedString::from(format!("F = {text}"))),
            )
    };
    let mut column = v_flex()
        .gap_2()
        .child(
            h_flex()
                .flex_wrap()
                .gap_4()
                .items_start()
                .children(body.maps().iter().map(|m| map_view(m, body, &colors, cx))),
        )
        .child(legend(&body.sop, &colors, cx))
        .child(expression("SOP", &body.sop_expression))
        .child(expression("POS", &body.pos_expression))
        .child(primes_grid(body).render(format!("{id}-primes"), cx));
    if body.variables.len() <= TABLE_VARIABLES {
        column = column.child(truth_grid(body).render(format!("{id}-truth"), cx));
    }
    column.into_any_element()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use domain::computed::CoverMethod;

    pub(crate) fn body() -> KarnaughBody {
        let imp = |care, value, term: &str, covers: Vec<u32>, essential| Implicant {
            care,
            value,
            term: term.into(),
            covers,
            essential,
        };
        KarnaughBody {
            variables: vec!["A".into(), "B".into(), "C".into()],
            minterms: vec![1, 3, 4, 5, 7],
            dont_cares: vec![],
            prime_implicants: vec![
                imp(0b110, 0b100, "AB'", vec![4, 5], true),
                imp(0b001, 0b001, "C", vec![1, 3, 5, 7], true),
            ],
            sop: vec![
                imp(0b110, 0b100, "AB'", vec![4, 5], true),
                imp(0b001, 0b001, "C", vec![1, 3, 5, 7], true),
            ],
            sop_expression: "AB' + C".into(),
            pos: vec![imp(0b101, 0b000, "(A + C)", vec![0, 2], true)],
            pos_expression: "(A + C)(B' + C)".into(),
            method: CoverMethod::Exact,
        }
    }

    #[test]
    fn cells_know_their_groups_and_tables_fill() {
        let b = body();
        assert_eq!(groups_at(&b, 5), [0, 1]);
        assert_eq!(groups_at(&b, 2), Vec::<usize>::new());
        assert_eq!(cell_text(b.value(4)), "1");
        let truth = truth_grid(&b);
        assert_eq!(truth.rows.len(), 8);
        assert_eq!(truth.rows[4][4].text, "1");
        let primes = primes_grid(&b);
        assert_eq!(primes.rows[1][3].text, "✓");
    }
}
