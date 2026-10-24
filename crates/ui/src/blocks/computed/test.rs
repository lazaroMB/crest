//! Results of a hypothesis test: the headline values with their
//! intervals, the groups compared and, for χ², the observed counts with
//! the expected ones.

use domain::computed::{Computed, TestBody, TestKind, apa};
use gpui::{AnyElement, App, IntoElement as _, ParentElement as _, Styled as _};
use gpui_component::v_flex;

use super::grid::Grid;
use super::{headline_grid, number};

/// The headline keys of a test and how the card labels them.
fn headline(test: TestKind) -> Vec<(&'static str, String)> {
    let symbol = test.statistic_symbol();
    let mut keys = vec![
        ("statistic", format!("{symbol} statistic")),
        ("df", "df".into()),
        ("z", "z (normal approximation)".into()),
        ("p_value", "p".into()),
    ];
    match test {
        TestKind::WelchT | TestKind::StudentT | TestKind::PairedT => {
            keys.push(("mean_diff", "Mean difference".into()));
            keys.push(("cohens_d", "Cohen's d".into()));
            keys.push(("hedges_g", "Hedges' g".into()));
        }
        TestKind::OneSampleT => {
            keys.push(("mean", "Mean".into()));
            keys.push(("cohens_d", "Cohen's d".into()));
            keys.push(("hedges_g", "Hedges' g".into()));
        }
        TestKind::MannWhitney | TestKind::Wilcoxon => {
            keys.push(("rank_biserial", "Rank-biserial r".into()));
        }
        TestKind::ChiSquare => keys.push(("cramers_v", "Cramér's V".into())),
        TestKind::Fisher => keys.push(("odds_ratio", "Odds ratio".into())),
    }
    keys.push(("n", "N".into()));
    keys
}

/// Draws the results of `body`.
pub fn render(id: &str, computed: &Computed, body: &TestBody, cx: &App) -> AnyElement {
    let keys = headline(body.test);
    let keys: Vec<(&str, &str)> = keys.iter().map(|(k, l)| (*k, l.as_str())).collect();
    let mut column = v_flex()
        .gap_2()
        .child(headline_grid(computed, &keys).render(format!("{id}-results"), cx));
    if !body.groups.is_empty() {
        let mut groups = Grid::new(["Group", "n", "Mean", "SD", "Median"]);
        for g in &body.groups {
            let opt = |x: Option<f64>| x.map(|x| number(x, 2)).unwrap_or_default();
            groups.push([
                g.name.clone(),
                g.n.to_string(),
                opt(g.mean),
                opt(g.sd),
                opt(g.median),
            ]);
        }
        column = column.child(groups.render(format!("{id}-groups"), cx));
    }
    if let Some(observed) = &body.observed {
        let width = observed.first().map(Vec::len).unwrap_or_default();
        let mut headers = vec!["Observed (expected)".to_owned()];
        headers.extend((1..=width).map(|c| format!("col {c}")));
        let mut table = Grid::new(headers);
        for (r, row) in observed.iter().enumerate() {
            let mut cells = vec![format!("row {}", r + 1)];
            for (c, count) in row.iter().enumerate() {
                let expected = body
                    .expected
                    .as_ref()
                    .and_then(|e| e.get(r))
                    .and_then(|row| row.get(c));
                cells.push(match expected {
                    Some(e) => format!("{} ({})", apa::number(*count, 0), apa::number(*e, 1)),
                    None => apa::number(*count, 0),
                });
            }
            table.push(cells);
        }
        column = column.child(table.render(format!("{id}-observed"), cx));
    }
    column.into_any_element()
}
