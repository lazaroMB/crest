//! A function plot with sliders (`show_plot`).
//!
//! Functions are expressions the UI evaluates itself, so a slider can
//! re-plot without a model turn: explicit `y = f(x)`, parametric
//! `(x(t), y(t))` and implicit `f(x, y) = 0`. Parameters are named
//! variables the expressions use, each with a slider. Numeric series (from
//! an engine, e.g. residuals vs fitted) are drawn as points or lines on the
//! same axes.

use std::collections::HashSet;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::block::BlockError;

/// Variable names a parameter cannot take, since expressions use them.
pub const RESERVED_VARIABLES: [&str; 3] = ["x", "y", "t"];

/// A plot of functions and series.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PlotSpec {
    /// Title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Visible x range; the UI fits the data when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x_range: Option<PlotRange>,
    /// Visible y range; the UI fits the data when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y_range: Option<PlotRange>,
    /// Label of the x axis.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x_label: Option<String>,
    /// Label of the y axis.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y_label: Option<String>,
    /// Functions to draw.
    #[serde(default)]
    pub functions: Vec<PlotFunction>,
    /// Parameters the functions use, each with a slider.
    #[serde(default)]
    pub params: Vec<PlotParam>,
    /// Numeric series.
    #[serde(default)]
    pub series: Vec<XySeries>,
}

/// A closed interval of an axis or of `t`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PlotRange {
    /// Lower end.
    pub min: f64,
    /// Upper end, greater than `min`.
    pub max: f64,
}

impl PlotRange {
    fn is_valid(self) -> bool {
        self.min.is_finite() && self.max.is_finite() && self.min < self.max
    }
}

/// A function to draw.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PlotFunction {
    /// How `expr` is read.
    pub kind: FunctionKind,
    /// `f(x)` for explicit, `x(t)` for parametric, `f(x, y)` (drawn where
    /// it is 0) for implicit.
    pub expr: String,
    /// `y(t)`, for parametric functions only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y_expr: Option<String>,
    /// Range of `t`, for parametric functions; `0..1` when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub t_range: Option<PlotRange>,
    /// Legend label; the expression when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// How a [`PlotFunction`] is defined.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FunctionKind {
    /// `y = f(x)`.
    Explicit,
    /// `(x(t), y(t))`.
    Parametric,
    /// `f(x, y) = 0`.
    Implicit,
}

/// A parameter with a slider.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PlotParam {
    /// Variable name used in the expressions.
    pub name: String,
    /// Slider minimum.
    pub min: f64,
    /// Slider maximum.
    pub max: f64,
    /// Slider step; continuous when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<f64>,
    /// Current value.
    pub value: f64,
}

/// A numeric series.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct XySeries {
    /// Legend name.
    pub name: String,
    /// x coordinates.
    pub x: Vec<f64>,
    /// y coordinates, one per x.
    pub y: Vec<f64>,
    /// Points or a line.
    #[serde(default)]
    pub style: SeriesStyle,
}

/// How an [`XySeries`] is drawn.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SeriesStyle {
    /// Points.
    #[default]
    Scatter,
    /// A line through the points in order.
    Line,
}

fn invalid(reason: String) -> BlockError {
    BlockError::Invalid {
        kind: "plot",
        reason,
    }
}

impl PlotSpec {
    /// Checks that there is something to draw, ranges are ordered,
    /// parametric functions have `y_expr`, parameters have distinct
    /// non-reserved names and values within their range, and series have
    /// one finite y per finite x.
    ///
    /// # Errors
    ///
    /// [`BlockError::Invalid`] describing the first problem.
    pub fn validate(&self) -> Result<(), BlockError> {
        if self.functions.is_empty() && self.series.is_empty() {
            return Err(invalid("a plot needs a function or a series".into()));
        }
        for (axis, range) in [("x", self.x_range), ("y", self.y_range)] {
            if range.is_some_and(|r| !r.is_valid()) {
                return Err(invalid(format!(
                    "the {axis} range must be finite with min < max"
                )));
            }
        }
        for f in &self.functions {
            if f.expr.trim().is_empty() {
                return Err(invalid("a function has an empty expression".into()));
            }
            let parametric = f.kind == FunctionKind::Parametric;
            if parametric && f.y_expr.as_deref().is_none_or(|e| e.trim().is_empty()) {
                return Err(invalid(format!("parametric {:?} needs y_expr", f.expr)));
            }
            if f.t_range.is_some_and(|r| !r.is_valid()) {
                return Err(invalid(format!("{:?} has a bad t_range", f.expr)));
            }
        }
        let mut names = HashSet::new();
        for p in &self.params {
            let identifier = p.name.chars().next().is_some_and(char::is_alphabetic)
                && p.name.chars().all(|c| c.is_alphanumeric() || c == '_');
            if !identifier || RESERVED_VARIABLES.contains(&p.name.as_str()) {
                return Err(invalid(format!("{:?} cannot be a parameter name", p.name)));
            }
            if !names.insert(p.name.as_str()) {
                return Err(invalid(format!("parameter {:?} appears twice", p.name)));
            }
            let range = PlotRange {
                min: p.min,
                max: p.max,
            };
            let step_ok = p.step.is_none_or(|s| s.is_finite() && s > 0.0);
            if !range.is_valid() || !step_ok || !(p.min..=p.max).contains(&p.value) {
                return Err(invalid(format!(
                    "parameter {:?} needs min < max, a positive step and a value in range",
                    p.name
                )));
            }
        }
        for s in &self.series {
            if s.x.len() != s.y.len() {
                return Err(BlockError::SeriesLength {
                    series: s.name.clone(),
                    expected: s.x.len(),
                    actual: s.y.len(),
                });
            }
            if let Some(index) =
                s.x.iter()
                    .zip(&s.y)
                    .position(|(x, y)| !x.is_finite() || !y.is_finite())
            {
                return Err(BlockError::NonFiniteValue {
                    series: s.name.clone(),
                    index,
                });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn plot() -> PlotSpec {
        serde_json::from_value(json!({
            "title": "Damped wave",
            "x_range": {"min": 0, "max": 10},
            "functions": [
                {"kind": "explicit", "expr": "a * exp(-k * x) * sin(x)", "label": "f"},
                {"kind": "parametric", "expr": "cos(t)", "y_expr": "sin(t)",
                 "t_range": {"min": 0, "max": 6}},
                {"kind": "implicit", "expr": "x^2 + y^2 - r^2"}
            ],
            "params": [
                {"name": "a", "min": 0, "max": 2, "value": 1},
                {"name": "k", "min": 0, "max": 1, "step": 0.01, "value": 0.2},
                {"name": "r", "min": 0.5, "max": 3, "value": 1}
            ],
            "series": [{"name": "data", "x": [1, 2], "y": [0.5, 0.2], "style": "line"}]
        }))
        .unwrap_or_else(|e| panic!("{e}"))
    }

    #[test]
    fn plot_roundtrips_and_validates() {
        let spec = plot();
        assert_eq!(spec.validate(), Ok(()));
        let value = serde_json::to_value(&spec).unwrap();
        assert_eq!(value["functions"][1]["kind"], "parametric");
        assert_eq!(value["series"][0]["style"], "line");
        let back: PlotSpec = serde_json::from_value(value).unwrap();
        assert_eq!(back, spec);
    }

    #[test]
    fn plot_validate_rejects_bad_specs() {
        let mut empty = plot();
        empty.functions.clear();
        empty.series.clear();
        assert!(empty.validate().is_err());

        let mut reversed = plot();
        reversed.x_range = Some(PlotRange { min: 1.0, max: 1.0 });
        assert!(reversed.validate().is_err());

        let mut no_y = plot();
        no_y.functions[1].y_expr = None;
        assert!(no_y.validate().is_err());

        let mut reserved = plot();
        reserved.params[0].name = "x".into();
        assert!(reserved.validate().is_err());

        let mut out_of_range = plot();
        out_of_range.params[1].value = 5.0;
        assert!(out_of_range.validate().is_err());

        let mut short = plot();
        short.series[0].y.pop();
        assert!(matches!(
            short.validate(),
            Err(BlockError::SeriesLength { .. })
        ));
    }
}
