//! Descriptive statistics: one row per numeric variable (and group) in the
//! gtsummary layout, then the counts of the categorical ones.

use domain::computed::{DescriptiveBody, VariableKind, VariableSummary};
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

/// Draws `body`.
pub fn render(id: &str, body: &DescriptiveBody, cx: &App) -> AnyElement {
    let mut numeric = Grid::new([
        "Variable", "n", "Missing", "Mean", "SD", "Median", "Q1", "Q3", "Min", "Max", "Skew",
        "Kurtosis",
    ]);
    let mut levels = Grid::new(["Variable · level", "Count", "%"]);
    for v in &body.variables {
        match v.kind {
            VariableKind::Numeric => {
                let decimals = auto_decimals(v.sd.unwrap_or_default());
                let opt = |x: Option<f64>| x.map(|x| number(x, decimals)).unwrap_or_default();
                let shape = |x: Option<f64>| x.map(|x| number(x, 2)).unwrap_or_default();
                numeric.push([
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
                ]);
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
