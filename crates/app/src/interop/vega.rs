//! Simple Vega-Lite specs as crest charts and plots.
//!
//! Only single views with inline data are mapped: mark `bar`, `line`,
//! `area`, `point`, `circle` or `square`; `x` and `y` fields of type
//! `quantitative`, `ordinal`, `nominal` or `temporal`; an optional `color`
//! field splitting the series; aggregates `sum`, `mean`, `count`, `min`
//! and `max`. Bars and lines over categories or dates become a chart;
//! lines and points over two quantities become a plot. Anything else
//! (layers, concatenation, facets, transforms, parameters, data from a
//! URL) is refused with the reason, and the caller shows the JSON.

use std::collections::BTreeMap;

use domain::block::{ChartKind, ChartSpec, CustomNode, Series};
use domain::plot::{PlotSpec, SeriesStyle, XySeries};
use serde_json::Value;

/// Most rows read from `data.values`.
const MAX_ROWS: usize = 50_000;
/// Most categories on a chart.
const MAX_LABELS: usize = 500;
/// Most series (colours).
const MAX_SERIES: usize = 20;

/// Keys that make a spec more than one simple view.
const COMPOSITE: [&str; 9] = [
    "layer",
    "hconcat",
    "vconcat",
    "concat",
    "facet",
    "repeat",
    "transform",
    "params",
    "selection",
];

/// One encoding channel.
struct Channel<'a> {
    field: Option<&'a str>,
    kind: &'a str,
    aggregate: Option<&'a str>,
    title: Option<String>,
}

fn channel<'a>(encoding: &'a Value, name: &str) -> Option<Channel<'a>> {
    let c = encoding.get(name)?;
    Some(Channel {
        field: c.get("field").and_then(Value::as_str),
        kind: c.get("type").and_then(Value::as_str).unwrap_or("nominal"),
        aggregate: c.get("aggregate").and_then(Value::as_str),
        title: c
            .get("title")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| c.get("field").and_then(Value::as_str).map(str::to_owned)),
    })
}

/// The value of `field` in `row`; dots reach into nested objects.
fn get<'a>(row: &'a Value, field: &str) -> Option<&'a Value> {
    row.get(field)
        .or_else(|| field.split('.').try_fold(row, |value, key| value.get(key)))
}

fn number(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
    .filter(|v: &f64| v.is_finite())
}

fn label(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => "(none)".into(),
        Some(other) => other.to_string(),
    }
}

/// Folds values with an aggregate.
#[derive(Default, Clone, Copy)]
struct Acc {
    sum: f64,
    n: usize,
    min: f64,
    max: f64,
}

impl Acc {
    fn add(&mut self, v: f64) {
        if self.n == 0 {
            self.min = v;
            self.max = v;
        }
        self.sum += v;
        self.n += 1;
        self.min = self.min.min(v);
        self.max = self.max.max(v);
    }

    fn get(self, aggregate: &str) -> f64 {
        match aggregate {
            "mean" | "average" if self.n > 0 => self.sum / self.n as f64,
            "count" => self.n as f64,
            "min" if self.n > 0 => self.min,
            "max" if self.n > 0 => self.max,
            "mean" | "average" | "min" | "max" => 0.0,
            _ => self.sum,
        }
    }
}

/// The node for `spec` (`datasets` resolve `data.name`), or why it is not
/// mapped.
///
/// # Errors
///
/// The reason the spec is not a simple chart.
pub fn chart(spec: &Value) -> Result<CustomNode, String> {
    let Some(map) = spec.as_object() else {
        return Err("the spec is not an object".into());
    };
    if let Some(key) = COMPOSITE.iter().find(|k| map.contains_key(**k)) {
        return Err(format!("`{key}` views are not mapped"));
    }
    let data = map.get("data").ok_or("the spec has no data")?;
    if data.get("url").is_some() {
        return Err("data from a URL is not fetched".into());
    }
    let rows = match (data.get("values"), data.get("name").and_then(Value::as_str)) {
        (Some(Value::Array(rows)), _) => rows,
        (_, Some(name)) => map
            .get("datasets")
            .and_then(|d| d.get(name))
            .and_then(Value::as_array)
            .ok_or_else(|| format!("dataset {name:?} is not in the spec"))?,
        _ => return Err("only inline `data.values` are mapped".into()),
    };
    if rows.len() > MAX_ROWS {
        return Err(format!("{} rows, at most {MAX_ROWS}", rows.len()));
    }
    let mark = match map.get("mark") {
        Some(Value::String(m)) => m.as_str(),
        Some(m) => m.get("type").and_then(Value::as_str).unwrap_or_default(),
        None => "",
    };
    let encoding = map.get("encoding").ok_or("the spec has no encoding")?;
    let (Some(x), Some(y)) = (channel(encoding, "x"), channel(encoding, "y")) else {
        return Err("both `x` and `y` must be encoded".into());
    };
    if let Some(other) = encoding
        .as_object()
        .into_iter()
        .flat_map(|e| e.keys())
        .find(|k| !matches!(k.as_str(), "x" | "y" | "color" | "tooltip"))
    {
        return Err(format!("the `{other}` channel is not mapped"));
    }
    let color = channel(encoding, "color").and_then(|c| c.field);
    let title = match map.get("title") {
        Some(Value::String(t)) => t.clone(),
        Some(t) => t
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        None => String::new(),
    };
    let quantitative = |c: &Channel<'_>| c.kind == "quantitative";
    match mark {
        "bar" | "line" | "area" => {
            let (cat, val) = if quantitative(&y) && !quantitative(&x) {
                (&x, &y)
            } else if quantitative(&x) && !quantitative(&y) && mark == "bar" {
                (&y, &x)
            } else if quantitative(&x) && quantitative(&y) && mark != "bar" {
                return plot(rows, &x, &y, color, SeriesStyle::Line, title);
            } else {
                return Err(format!("a {mark} needs one quantitative axis"));
            };
            categories(rows, cat, val, color, mark, title)
        }
        "point" | "circle" | "square" => {
            if !quantitative(&x) || !quantitative(&y) {
                return Err("points need two quantitative axes".into());
            }
            plot(rows, &x, &y, color, SeriesStyle::Scatter, title)
        }
        other => Err(format!("mark {other:?} is not mapped")),
    }
}

fn check_aggregate(c: &Channel<'_>) -> Result<&'static str, String> {
    match c.aggregate {
        None | Some("sum") => Ok("sum"),
        Some("mean" | "average") => Ok("mean"),
        Some("count") => Ok("count"),
        Some("min") => Ok("min"),
        Some("max") => Ok("max"),
        Some(other) => Err(format!("aggregate {other:?} is not mapped")),
    }
}

fn categories(
    rows: &[Value],
    cat: &Channel<'_>,
    val: &Channel<'_>,
    color: Option<&str>,
    mark: &str,
    title: String,
) -> Result<CustomNode, String> {
    let aggregate = check_aggregate(val)?;
    let cat_field = cat.field.ok_or("the category axis has no field")?;
    if val.field.is_none() && aggregate != "count" {
        return Err("the value axis has no field".into());
    }
    let mut labels: Vec<String> = Vec::new();
    let mut names: Vec<String> = Vec::new();
    let mut cells: BTreeMap<(usize, usize), Acc> = BTreeMap::new();
    for row in rows {
        let value = match val.field {
            Some(field) => match number(get(row, field)) {
                Some(v) => v,
                None => continue,
            },
            None => 1.0,
        };
        let l = label(get(row, cat_field));
        let li = match labels.iter().position(|x| *x == l) {
            Some(i) => i,
            None if labels.len() < MAX_LABELS => {
                labels.push(l);
                labels.len() - 1
            }
            None => return Err(format!("more than {MAX_LABELS} categories")),
        };
        let s = color.map_or_else(
            || val.title.clone().unwrap_or_else(|| "count".into()),
            |c| label(get(row, c)),
        );
        let si = match names.iter().position(|x| *x == s) {
            Some(i) => i,
            None if names.len() < MAX_SERIES => {
                names.push(s);
                names.len() - 1
            }
            None => return Err(format!("more than {MAX_SERIES} series")),
        };
        cells.entry((li, si)).or_default().add(value);
    }
    if labels.is_empty() {
        return Err("no row has a value".into());
    }
    // Dates in ISO form sort as text; keep the given order otherwise.
    let mut order: Vec<usize> = (0..labels.len()).collect();
    if cat.kind == "temporal" {
        order.sort_by(|a, b| labels[*a].cmp(&labels[*b]));
    }
    let series = names
        .iter()
        .enumerate()
        .map(|(si, name)| Series {
            name: name.clone(),
            values: order
                .iter()
                .map(|li| cells.get(&(*li, si)).map_or(0.0, |acc| acc.get(aggregate)))
                .collect(),
        })
        .collect();
    let kind = match mark {
        "line" => ChartKind::Line,
        "area" => ChartKind::Area,
        _ => ChartKind::Bar,
    };
    let spec = ChartSpec {
        title,
        kind,
        labels: order.iter().map(|i| labels[*i].clone()).collect(),
        series,
    };
    spec.validate().map_err(|e| e.to_string())?;
    Ok(CustomNode::Chart(spec))
}

fn plot(
    rows: &[Value],
    x: &Channel<'_>,
    y: &Channel<'_>,
    color: Option<&str>,
    style: SeriesStyle,
    title: String,
) -> Result<CustomNode, String> {
    if x.aggregate.is_some() || y.aggregate.is_some() {
        return Err("aggregates over two quantities are not mapped".into());
    }
    let (Some(xf), Some(yf)) = (x.field, y.field) else {
        return Err("both axes need a field".into());
    };
    let mut series: Vec<XySeries> = Vec::new();
    for row in rows {
        let (Some(xv), Some(yv)) = (number(get(row, xf)), number(get(row, yf))) else {
            continue;
        };
        let name = color.map_or_else(
            || y.title.clone().unwrap_or_default(),
            |c| label(get(row, c)),
        );
        let ix = match series.iter().position(|s| s.name == name) {
            Some(ix) => ix,
            None if series.len() < MAX_SERIES => {
                series.push(XySeries {
                    name,
                    x: Vec::new(),
                    y: Vec::new(),
                    style,
                });
                series.len() - 1
            }
            None => return Err(format!("more than {MAX_SERIES} series")),
        };
        series[ix].x.push(xv);
        series[ix].y.push(yv);
    }
    if series.is_empty() {
        return Err("no row has two numbers".into());
    }
    if style == SeriesStyle::Line {
        for s in &mut series {
            let mut pairs: Vec<(f64, f64)> = s.x.iter().copied().zip(s.y.iter().copied()).collect();
            pairs.sort_by(|a, b| a.0.total_cmp(&b.0));
            (s.x, s.y) = pairs.into_iter().unzip();
        }
    }
    let spec = PlotSpec {
        title: Some(title).filter(|t| !t.is_empty()),
        x_label: x.title.clone(),
        y_label: y.title.clone(),
        series,
        ..PlotSpec::default()
    };
    spec.validate().map_err(|e| e.to_string())?;
    Ok(CustomNode::Plot(Box::new(spec)))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn grouped_bars_aggregate_by_category() {
        let spec = json!({
            "$schema": "https://vega.github.io/schema/vega-lite/v5.json",
            "title": "Sales",
            "data": {"values": [
                {"q": "Q1", "region": "N", "v": 2}, {"q": "Q1", "region": "S", "v": 3},
                {"q": "Q2", "region": "N", "v": 4}, {"q": "Q1", "region": "N", "v": 1}
            ]},
            "mark": "bar",
            "encoding": {
                "x": {"field": "q", "type": "nominal"},
                "y": {"field": "v", "type": "quantitative", "aggregate": "sum"},
                "color": {"field": "region", "type": "nominal"}
            }
        });
        let CustomNode::Chart(chart) = chart(&spec).unwrap() else {
            panic!()
        };
        assert_eq!(chart.title, "Sales");
        assert_eq!(chart.labels, ["Q1", "Q2"]);
        assert_eq!(chart.series[0].name, "N");
        assert_eq!(chart.series[0].values, [3.0, 4.0]);
        assert_eq!(chart.series[1].values, [3.0, 0.0]);
    }

    #[test]
    fn temporal_lines_sort_and_quantities_plot() {
        let line = json!({
            "data": {"values": [{"d": "2026-02-01", "n": 5}, {"d": "2026-01-01", "n": 3}]},
            "mark": {"type": "line", "point": true},
            "encoding": {"x": {"field": "d", "type": "temporal"}, "y": {"field": "n", "type": "quantitative"}}
        });
        let CustomNode::Chart(lines) = chart(&line).unwrap() else {
            panic!()
        };
        assert_eq!(lines.kind, ChartKind::Line);
        assert_eq!(lines.labels, ["2026-01-01", "2026-02-01"]);
        let scatter = json!({
            "data": {"values": [{"a": 1, "b": 2}, {"a": "3", "b": 4}, {"a": null, "b": 1}]},
            "mark": "point",
            "encoding": {"x": {"field": "a", "type": "quantitative"}, "y": {"field": "b", "type": "quantitative"}}
        });
        let CustomNode::Plot(plot) = chart(&scatter).unwrap() else {
            panic!()
        };
        assert_eq!(plot.series[0].x, [1.0, 3.0]);
        assert_eq!(plot.series[0].style, SeriesStyle::Scatter);
        assert_eq!(plot.x_label.as_deref(), Some("a"));
    }

    #[test]
    fn counts_and_horizontal_bars() {
        let spec = json!({
            "data": {"values": [{"k": "a"}, {"k": "a"}, {"k": "b"}]},
            "mark": "bar",
            "encoding": {"y": {"field": "k", "type": "nominal"}, "x": {"aggregate": "count", "type": "quantitative"}}
        });
        let CustomNode::Chart(chart) = chart(&spec).unwrap() else {
            panic!()
        };
        assert_eq!(chart.series[0].values, [2.0, 1.0]);
    }

    #[test]
    fn everything_else_is_refused_with_a_reason() {
        let base = json!({"data": {"values": [{"a": 1, "b": 2}]}, "mark": "bar",
            "encoding": {"x": {"field": "a", "type": "nominal"}, "y": {"field": "b", "type": "quantitative"}}});
        for (patch, why) in [
            (json!({"layer": []}), "layer"),
            (json!({"transform": [{"filter": "true"}]}), "transform"),
            (json!({"data": {"url": "https://x.test/d.csv"}}), "URL"),
            (json!({"mark": "geoshape"}), "geoshape"),
            (
                json!({"encoding": {"x": {"field": "a"}, "y": {"field": "b", "type": "quantitative"}, "size": {"field": "b"}}}),
                "size",
            ),
            (
                json!({"encoding": {"x": {"field": "a", "type": "nominal"}, "y": {"field": "b", "type": "quantitative", "aggregate": "variance"}}}),
                "variance",
            ),
        ] {
            let mut spec = base.clone();
            for (k, v) in patch.as_object().unwrap() {
                spec[k] = v.clone();
            }
            let err = chart(&spec).unwrap_err();
            assert!(err.contains(why), "{why}: {err}");
        }
        let many: Vec<Value> = (0..=MAX_LABELS)
            .map(|i| json!({"a": i.to_string(), "b": 1}))
            .collect();
        let mut spec = base;
        spec["data"] = json!({"values": many});
        assert!(chart(&spec).unwrap_err().contains("categories"));
    }
}
