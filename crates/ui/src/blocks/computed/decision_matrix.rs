//! The decision-matrix card: the options against the criteria (normalised
//! score with the raw one), the weighted totals and ranks with first place
//! highlighted, and the weight sensitivity table: for each criterion, the
//! nearest weights below and above at which another option takes the
//! lead.

use domain::computed::{CriterionDirection, DecisionMatrixBody, WeightThreshold, apa};
use gpui::{AnyElement, App, IntoElement as _, ParentElement as _, SharedString, Styled as _, div};
use gpui_component::{ActiveTheme as _, v_flex};

use super::grid::{Grid, GridCell};

fn trim(text: String) -> String {
    if text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.').to_owned()
    } else {
        text
    }
}

/// Options × criteria, then the total and rank.
pub fn matrix_grid(body: &DecisionMatrixBody, accent: gpui::Hsla) -> Grid {
    let mut headers = vec!["Option".to_owned()];
    for c in &body.criteria {
        let arrow = match c.direction {
            CriterionDirection::Benefit => "↑",
            CriterionDirection::Cost => "↓",
        };
        headers.push(format!(
            "{} {arrow} {}%",
            c.name,
            apa::number(c.weight * 100.0, 0)
        ));
    }
    headers.extend(["Total".to_owned(), "Rank".to_owned()]);
    let mut grid = Grid::new(headers);
    for o in &body.options {
        let bg = (o.rank == 1).then(|| accent.opacity(0.15));
        let cell = |text: String| GridCell { text, bg, fg: None };
        let mut row = vec![cell(o.name.clone())];
        for (raw, norm) in o.scores.iter().zip(&o.normalized) {
            row.push(cell(format!(
                "{} ({})",
                apa::number(*norm, 2),
                trim(apa::number(*raw, 4))
            )));
        }
        row.push(cell(apa::number(o.total, 3)));
        row.push(cell(o.rank.to_string()));
        grid.rows.push(row);
    }
    grid
}

fn threshold(t: Option<&WeightThreshold>) -> String {
    t.map_or_else(
        || "—".to_owned(),
        |t| format!("{}% → {}", apa::number(t.weight * 100.0, 1), t.new_leader),
    )
}

/// The weight sensitivity table.
pub fn sensitivity_grid(body: &DecisionMatrixBody) -> Grid {
    let mut grid = Grid::new([
        "Criterion",
        "Weight",
        "Lead changes below",
        "Lead changes above",
    ]);
    for s in &body.sensitivity {
        grid.push([
            s.criterion.clone(),
            format!("{}%", apa::number(s.weight * 100.0, 1)),
            threshold(s.lower.as_ref()),
            threshold(s.upper.as_ref()),
        ]);
    }
    grid
}

/// Draws `body`.
pub fn render(id: &str, body: &DecisionMatrixBody, cx: &App) -> AnyElement {
    let theme = cx.theme();
    v_flex()
        .gap_1()
        .child(matrix_grid(body, theme.success).render(format!("{id}-matrix"), cx))
        .child(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(SharedString::from(format!(
                    "Scores: {} normalised (raw in brackets); ↑ benefit, ↓ cost; weights rescaled \
                     to 100%. Sensitivity moves one weight, the others in proportion.",
                    body.normalization.label()
                ))),
        )
        .child(sensitivity_grid(body).render(format!("{id}-sensitivity"), cx))
        .into_any_element()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use domain::computed::{DecisionCriterion, DecisionOption, Normalization, WeightSensitivity};

    pub(crate) fn body() -> DecisionMatrixBody {
        let criterion = |name: &str, weight, direction| DecisionCriterion {
            name: name.into(),
            raw_weight: weight * 100.0,
            weight,
            direction,
        };
        let option =
            |name: &str, scores: Vec<f64>, normalized: Vec<f64>, total, rank| DecisionOption {
                name: name.into(),
                scores,
                normalized,
                total,
                rank,
            };
        DecisionMatrixBody {
            normalization: Normalization::MinMax,
            criteria: vec![
                criterion("price", 0.6, CriterionDirection::Cost),
                criterion("quality", 0.4, CriterionDirection::Benefit),
            ],
            options: vec![
                option("A", vec![100.0, 7.0], vec![1.0, 0.0], 0.6, 1),
                option("B", vec![120.0, 9.0], vec![0.0, 1.0], 0.4, 2),
            ],
            sensitivity: vec![
                WeightSensitivity {
                    criterion: "price".into(),
                    weight: 0.6,
                    lower: Some(WeightThreshold {
                        weight: 0.5,
                        new_leader: "B".into(),
                    }),
                    upper: None,
                },
                WeightSensitivity {
                    criterion: "quality".into(),
                    weight: 0.4,
                    lower: None,
                    upper: Some(WeightThreshold {
                        weight: 0.5,
                        new_leader: "B".into(),
                    }),
                },
            ],
        }
    }

    #[test]
    fn the_leader_is_marked_and_thresholds_named() {
        let b = body();
        let grid = matrix_grid(&b, gpui::green());
        assert_eq!(grid.headers[1], "price ↓ 60%");
        assert_eq!(grid.rows[0][1].text, "1.00 (100)");
        assert!(grid.rows[0][0].bg.is_some() && grid.rows[1][0].bg.is_none());
        let sens = sensitivity_grid(&b);
        assert_eq!(sens.rows[0][2].text, "50.0% → B");
        assert_eq!(sens.rows[0][3].text, "—");
    }
}
