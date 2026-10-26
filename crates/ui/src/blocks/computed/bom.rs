//! The BOM costing card: the totals (per unit, for the units built, the
//! line count and the gaps), the totals per category, and every line
//! with its quantity, unit cost and extended cost, unpriced lines and
//! lines without an MPN marked in the warning color.

use domain::computed::{BomBody, Computed};
use gpui::{AnyElement, App, IntoElement as _, ParentElement as _, Styled as _};
use gpui_component::{ActiveTheme as _, v_flex};

use super::grid::{Grid, GridCell};

/// Lines drawn; the copy-as CSV has them all.
pub const MAX_ROWS: usize = 500;

/// The totals as the engine displays them.
pub fn totals_grid(computed: &Computed, body: &BomBody) -> Grid {
    let mut grid = Grid::new(["Result", "Value"]);
    let shown = |key: &str| {
        computed.value(key).map(|v| match &v.unit {
            Some(u) => format!("{} {u}", v.display_text()),
            None => v.display_text(),
        })
    };
    if let Some(t) = shown("total") {
        grid.push(["Cost per unit".to_owned(), t]);
    }
    if let Some(t) = shown("total_builds") {
        grid.push([format!("Cost of {} units", body.builds), t]);
    }
    if let Some(n) = shown("lines") {
        grid.push(["Lines".to_owned(), n]);
    }
    if let Some(n) = shown("missing_prices") {
        grid.push(["Lines without a price".to_owned(), n]);
    }
    if let Some(n) = shown("missing_mpns") {
        grid.push(["Lines without an MPN".to_owned(), n]);
    }
    grid
}

/// Totals per category.
pub fn categories_grid(body: &BomBody) -> Grid {
    let mut grid = Grid::new(["Category", "Lines", "Quantity", "Total", "Unpriced"]);
    for c in &body.categories {
        grid.push([
            c.name.clone(),
            c.lines.to_string(),
            c.quantity.clone(),
            c.total.clone(),
            c.missing_prices.to_string(),
        ]);
    }
    grid
}

/// The lines, gaps marked with `warning`.
pub fn lines_grid(body: &BomBody, warning: gpui::Hsla) -> Grid {
    let mut grid = Grid::new([
        "Part",
        "Row",
        "References",
        "MPN",
        "Category",
        "Qty",
        "Unit cost",
        "Extended",
    ]);
    for l in body.lines.iter().take(MAX_ROWS) {
        let flag = |text: String, gap: bool| GridCell {
            text,
            bg: gap.then(|| warning.opacity(0.12)),
            fg: gap.then_some(warning),
        };
        let qty = if l.quantity_from_references {
            format!("{} (refs)", l.quantity)
        } else {
            l.quantity.clone()
        };
        grid.rows.push(vec![
            GridCell::from(l.part.clone()),
            GridCell::from(l.row.to_string()),
            GridCell::from(l.reference.clone().unwrap_or_default()),
            flag(
                l.mpn
                    .clone()
                    .unwrap_or_else(|| if body.has_mpn { "missing" } else { "" }.to_owned()),
                body.has_mpn && l.mpn.is_none(),
            ),
            GridCell::from(l.category.clone()),
            GridCell::from(qty),
            flag(
                l.unit_cost.clone().unwrap_or_else(|| "no price".to_owned()),
                l.unit_cost.is_none(),
            ),
            GridCell::from(l.extended.clone().unwrap_or_default()),
        ]);
    }
    grid
}

/// Draws `body`.
pub fn render(id: &str, computed: &Computed, body: &BomBody, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let mut column = v_flex()
        .gap_1()
        .child(totals_grid(computed, body).render(format!("{id}-totals"), cx))
        .child(categories_grid(body).render(format!("{id}-categories"), cx))
        .child(lines_grid(body, theme.warning).render(format!("{id}-lines"), cx));
    if body.lines.len() > MAX_ROWS {
        column = column.child(
            gpui::div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(format!(
                    "… {} more lines (copy as CSV for all)",
                    body.lines.len() - MAX_ROWS
                )),
        );
    }
    column.into_any_element()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use domain::computed::{BomCategory, BomLine};

    pub(crate) fn body() -> BomBody {
        let line = |row, part: &str, qty: &str, unit: Option<&str>, ext: Option<&str>| BomLine {
            row,
            part: part.into(),
            reference: None,
            mpn: None,
            description: None,
            category: "passives".into(),
            quantity: qty.into(),
            unit_cost: unit.map(str::to_owned),
            extended: ext.map(str::to_owned),
            quantity_from_references: false,
        };
        BomBody {
            currency: Some("USD".into()),
            builds: "10".into(),
            lines: vec![
                line(1, "R 10k", "6", Some("0.0021"), Some("0.0126")),
                line(2, "switch", "2", None, None),
            ],
            categories: vec![BomCategory {
                name: "passives".into(),
                lines: 2,
                quantity: "8".into(),
                total: "0.0126".into(),
                missing_prices: 1,
            }],
            total: "0.0126".into(),
            total_builds: "0.126".into(),
            has_mpn: true,
        }
    }

    #[test]
    fn gaps_are_marked() {
        let grid = lines_grid(&body(), gpui::red());
        assert_eq!(grid.rows[1][6].text, "no price");
        assert!(grid.rows[1][6].fg.is_some());
        assert_eq!(grid.rows[0][3].text, "missing");
        assert_eq!(categories_grid(&body()).rows[0][4].text, "1");
    }
}
