//! Descriptive statistics: one row per numeric variable (and group) in the
//! gtsummary layout (with a histogram sparkline when the engine binned
//! the values), then the counts of the categorical ones.

use domain::computed::{DescriptiveBody, Histogram, VariableKind, VariableSummary};
use gpui::{AnyElement, App, IntoElement as _, ParentElement as _, Styled as _};
use gpui_component::v_flex;

use super::grid::Grid;
use super::{auto_decimals, number};

fn label(v: &VariableSummary) -> String {
    let name = match &v.unit {
        Some(unit) if !unit.is_empty() => format!("{} ({unit})", v.name),
        _ => v.name.clone(),
    };
    match &v.group {
        Some(group) => format!("{name} · {group}"),
        None => name,
    }
}

/// Bar heights of a sparkline, lowest first.
const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// `histogram` as one bar character per bin, scaled to the fullest bin;
/// an empty bin is a space.
pub fn sparkline(histogram: &Histogram) -> String {
    let most = histogram.counts.iter().copied().max().unwrap_or(0);
    histogram
        .counts
        .iter()
        .map(|&count| {
            if count == 0 || most == 0 {
                ' '
            } else {
                let level = (count as f64 / most as f64 * (BARS.len() - 1) as f64).round() as usize;
                BARS[level.min(BARS.len() - 1)]
            }
        })
        .collect()
}

/// Draws `body`.
pub fn render(id: &str, body: &DescriptiveBody, cx: &App) -> AnyElement {
    let distribution = body
        .variables
        .iter()
        .any(|v| v.kind == VariableKind::Numeric && v.histogram.is_some());
    let mut headers = vec![
        "Variable", "n", "Missing", "Mean", "SD", "Median", "Q1", "Q3", "Min", "Max", "Skew",
        "Kurtosis",
    ];
    if distribution {
        headers.push("Distribution");
    }
    let mut numeric = Grid::new(headers);
    let mut levels = Grid::new(["Variable · level", "Count", "%"]);
    for v in &body.variables {
        match v.kind {
            VariableKind::Numeric => {
                let decimals = auto_decimals(v.sd.unwrap_or_default());
                let opt = |x: Option<f64>| x.map(|x| number(x, decimals)).unwrap_or_default();
                let shape = |x: Option<f64>| x.map(|x| number(x, 2)).unwrap_or_default();
                let mut row = vec![
                    label(v),
                    v.n.to_string(),
                    v.missing.to_string(),
                    opt(v.mean),
                    opt(v.sd),
                    opt(v.median),
                    opt(v.q1),
                    opt(v.q3),
                    opt(v.min),
                    opt(v.max),
                    shape(v.skew),
                    shape(v.kurtosis),
                ];
                if distribution {
                    row.push(v.histogram.as_ref().map(sparkline).unwrap_or_default());
                }
                numeric.push(row);
            }
            VariableKind::Categorical => {
                for level in &v.levels {
                    levels.push([
                        format!("{} · {}", label(v), level.level),
                        level.count.to_string(),
                        number(level.percent, 1),
                    ]);
                }
                if v.missing > 0 {
                    levels.push([
                        format!("{} · (missing)", label(v)),
                        v.missing.to_string(),
                        String::new(),
                    ]);
                }
            }
        }
    }
    let mut column = v_flex().gap_2();
    if !numeric.is_empty() {
        column = column.child(numeric.render(format!("{id}-numeric"), cx));
    }
    if !levels.is_empty() {
        column = column.child(levels.render(format!("{id}-levels"), cx));
    }
    column.into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparklines_scale_to_the_fullest_bin() {
        let histogram = Histogram {
            edges: vec![0.0, 1.0, 2.0, 3.0, 4.0],
            counts: vec![1, 8, 0, 4],
        };
        assert_eq!(sparkline(&histogram), "▂█ ▅");
        let empty = Histogram {
            edges: vec![0.0, 1.0],
            counts: vec![0],
        };
        assert_eq!(sparkline(&empty), " ");
    }
}
