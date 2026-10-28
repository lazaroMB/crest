//! A tree table: nested rows with typed columns, badges and inline bars.
//!
//! Query plans, Terraform plans, span attributes and file sizes are trees
//! whose rows carry numbers. Each node has a label, an optional badge and
//! a value per column; columns are text, numbers, durations or bars (a
//! number drawn as a bar scaled to the column's largest value, like the
//! "time %" of an `EXPLAIN ANALYZE` viewer).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::list::value_text;
use super::timeline::{TimeUnit, format_duration};
use super::{invalid, unique};
use crate::block::BlockError;

/// Nodes (all levels together) a tree may have.
pub const MAX_NODES: usize = 100_000;
/// Deepest nesting.
pub const MAX_DEPTH: usize = 128;
/// Columns a tree may have.
pub const MAX_COLUMNS: usize = 24;

const KIND: &str = "tree";

/// What a column holds and how it is drawn.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ColumnKind {
    /// Any value, as text, left-aligned.
    #[default]
    Text,
    /// A number, right-aligned.
    Number,
    /// A time span in the column's unit (`ms` by default), right-aligned.
    Duration,
    /// A number with a bar scaled to the column's largest value.
    Bar,
}

impl ColumnKind {
    /// Whether values must be numbers.
    #[must_use]
    pub fn numeric(self) -> bool {
        !matches!(self, Self::Text)
    }
}

/// One column.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TreeColumn {
    /// Header, and the key of the nodes' `values`.
    pub name: String,
    /// What it holds.
    #[serde(default)]
    pub kind: ColumnKind,
    /// For a duration, its time unit (`ns`, `us`, `ms`, `s`); for a
    /// number or a bar, a suffix (`rows`, `%`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
}

impl TreeColumn {
    /// The time unit of a duration column (`ms` when unset or unknown).
    #[must_use]
    pub fn time_unit(&self) -> TimeUnit {
        self.unit
            .as_deref()
            .and_then(parse_time_unit)
            .unwrap_or_default()
    }

    /// `value` as this column shows it: durations in a readable unit,
    /// numbers compact with the suffix, anything else as text. Empty for
    /// a missing value.
    #[must_use]
    pub fn format(&self, value: Option<&Value>) -> String {
        let Some(value) = value.filter(|v| !v.is_null()) else {
            return String::new();
        };
        match (self.kind, value.as_f64()) {
            (ColumnKind::Duration, Some(n)) => format_duration(n, self.time_unit()),
            (ColumnKind::Number | ColumnKind::Bar, Some(n)) => {
                let text = format_number(n);
                match self.unit.as_deref().filter(|u| !u.is_empty()) {
                    Some("%") => format!("{text}%"),
                    Some(unit) => format!("{text} {unit}"),
                    None => text,
                }
            }
            _ => value_text(value),
        }
    }
}

fn parse_time_unit(unit: &str) -> Option<TimeUnit> {
    match unit {
        "ns" => Some(TimeUnit::Ns),
        "us" | "µs" => Some(TimeUnit::Us),
        "ms" => Some(TimeUnit::Ms),
        "s" => Some(TimeUnit::S),
        _ => None,
    }
}

/// A number for a cell: integers as they are, others with at most three
/// decimals (fewer for large values), trailing zeros dropped.
#[must_use]
pub fn format_number(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        return format!("{n:.0}");
    }
    let decimals = match n.abs() {
        a if a >= 1000.0 => 1,
        a if a >= 10.0 => 2,
        _ => 3,
    };
    let text = format!("{n:.decimals$}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text == "-0" {
        "0".to_owned()
    } else {
        text.to_owned()
    }
}

/// Color of a badge.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BadgeTone {
    /// Muted.
    #[default]
    Neutral,
    /// Informational (accent).
    Info,
    /// Good (success color).
    Ok,
    /// Worth a look (warning color).
    Warning,
    /// Bad (danger color): a hot plan node, a resource destroyed.
    Error,
}

/// One row, with its children.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TreeNode {
    /// Text of the first column.
    pub label: String,
    /// A short tag after the label (`hot`, `seq scan`, `+create`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub badge: Option<String>,
    /// Color of the badge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tone: Option<BadgeTone>,
    /// Value of each column, by column name.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    #[schemars(with = "std::collections::BTreeMap<String, Value>")]
    pub values: Map<String, Value>,
    /// Rows nested under this one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<TreeNode>,
}

fn default_expand_depth() -> u32 {
    2
}

/// Nested rows with typed columns.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TreeSpec {
    /// Caption above the tree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Columns after the label.
    #[serde(default)]
    pub columns: Vec<TreeColumn>,
    /// Top-level rows.
    pub nodes: Vec<TreeNode>,
    /// Levels open at first (2: the top rows and their children).
    #[serde(default = "default_expand_depth")]
    pub expand_depth: u32,
}

/// A node in depth-first order, as [`TreeSpec::flatten`] lists it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FlatNode<'a> {
    /// Nesting level, 0 for top-level rows.
    pub depth: usize,
    /// Index (in the flat list) just past this node's subtree.
    pub end: usize,
    /// The node.
    pub node: &'a TreeNode,
}

impl TreeSpec {
    /// Checks columns (named, distinct, not too many, known duration
    /// units), the size and depth, and that numeric columns hold numbers.
    ///
    /// # Errors
    ///
    /// [`BlockError::Invalid`] naming the first problem.
    pub fn validate(&self) -> Result<(), BlockError> {
        if self.columns.len() > MAX_COLUMNS {
            return Err(invalid(
                KIND,
                format!("{} columns; at most {MAX_COLUMNS}", self.columns.len()),
            ));
        }
        if self.columns.iter().any(|c| c.name.trim().is_empty()) {
            return Err(invalid(KIND, "a column name is empty"));
        }
        unique(KIND, "column", self.columns.iter().map(|c| c.name.as_str()))?;
        for column in &self.columns {
            if column.kind == ColumnKind::Duration
                && let Some(unit) = &column.unit
                && parse_time_unit(unit).is_none()
            {
                return Err(invalid(
                    KIND,
                    format!(
                        "column {:?} has unknown time unit {unit:?} (ns, us, ms or s)",
                        column.name
                    ),
                ));
            }
        }
        let mut count = 0_usize;
        let mut stack: Vec<(&TreeNode, usize)> = self.nodes.iter().rev().map(|n| (n, 0)).collect();
        while let Some((node, depth)) = stack.pop() {
            count += 1;
            if count > MAX_NODES {
                return Err(invalid(KIND, format!("more than {MAX_NODES} nodes")));
            }
            if depth >= MAX_DEPTH {
                return Err(invalid(
                    KIND,
                    format!("{:?} is nested deeper than {MAX_DEPTH}", node.label),
                ));
            }
            for column in self.columns.iter().filter(|c| c.kind.numeric()) {
                if let Some(value) = node.values.get(&column.name)
                    && !value.is_null()
                    && !value.is_number()
                {
                    return Err(invalid(
                        KIND,
                        format!(
                            "{:?}: column {:?} needs a number, got {value}",
                            node.label, column.name
                        ),
                    ));
                }
            }
            stack.extend(node.children.iter().rev().map(|n| (n, depth + 1)));
        }
        Ok(())
    }

    /// Every node in depth-first order, with its depth and the end of its
    /// subtree.
    #[must_use]
    pub fn flatten(&self) -> Vec<FlatNode<'_>> {
        fn visit<'a>(node: &'a TreeNode, depth: usize, out: &mut Vec<FlatNode<'a>>) {
            let at = out.len();
            out.push(FlatNode {
                depth,
                end: at + 1,
                node,
            });
            for child in &node.children {
                visit(child, depth + 1, out);
            }
            let end = out.len();
            out[at].end = end;
        }
        let mut out = Vec::new();
        for node in &self.nodes {
            visit(node, 0, &mut out);
        }
        out
    }

    /// Largest absolute value of each column over all nodes (0 for text
    /// columns and columns without numbers), the scale of its bars.
    #[must_use]
    pub fn column_max(&self) -> Vec<f64> {
        let flat = self.flatten();
        self.columns
            .iter()
            .map(|column| {
                if !column.kind.numeric() {
                    return 0.0;
                }
                flat.iter()
                    .filter_map(|f| f.node.values.get(&column.name)?.as_f64())
                    .map(f64::abs)
                    .fold(0.0, f64::max)
            })
            .collect()
    }

    /// The tree as text: a header line, then one line per node indented
    /// two spaces per level, the badge in brackets and the formatted
    /// values separated by tabs.
    #[must_use]
    pub fn to_text(&self) -> String {
        let mut out = String::from("name");
        for column in &self.columns {
            out.push('\t');
            out.push_str(&column.name);
        }
        out.push('\n');
        for flat in self.flatten() {
            out.push_str(&"  ".repeat(flat.depth));
            out.push_str(&flat.node.label);
            if let Some(badge) = &flat.node.badge {
                out.push_str(&format!(" [{badge}]"));
            }
            for column in &self.columns {
                out.push('\t');
                out.push_str(&column.format(flat.node.values.get(&column.name)));
            }
            out.push('\n');
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn plan() -> TreeSpec {
        serde_json::from_value(json!({
            "title": "plan",
            "columns": [
                { "name": "time", "kind": "duration" },
                { "name": "rows", "kind": "number" },
                { "name": "share", "kind": "bar", "unit": "%" },
                { "name": "filter" }
            ],
            "nodes": [{
                "label": "Hash Join", "values": { "time": 12.5, "rows": 1000, "share": 100 },
                "children": [
                    { "label": "Seq Scan users", "badge": "hot", "tone": "error",
                      "values": { "time": 9.75, "rows": 120000, "share": 78, "filter": "age > 30" } },
                    { "label": "Index Scan orders", "values": { "time": 0.25, "rows": 3 } }
                ]
            }]
        }))
        .unwrap()
    }

    #[test]
    fn parses_flattens_and_scales() {
        let spec = plan();
        spec.validate().unwrap();
        assert_eq!(spec.expand_depth, 2);
        let flat = spec.flatten();
        let summary: Vec<_> = flat.iter().map(|f| (f.depth, f.end)).collect();
        assert_eq!(summary, [(0, 3), (1, 2), (1, 3)]);
        assert_eq!(spec.column_max(), [12.5, 120_000.0, 100.0, 0.0]);
    }

    #[test]
    fn cells_are_formatted_by_kind() {
        let spec = plan();
        let node = &spec.nodes[0].children[0];
        let cells: Vec<String> = spec
            .columns
            .iter()
            .map(|c| c.format(node.values.get(&c.name)))
            .collect();
        assert_eq!(cells, ["9.75 ms", "120000", "78%", "age > 30"]);
        assert_eq!(spec.columns[0].format(None), "");
        assert_eq!(format_number(0.123_456), "0.123");
        assert_eq!(format_number(1234.56), "1234.6");
        assert_eq!(format_number(-0.0001), "0");
    }

    #[test]
    fn text_is_indented_with_badges() {
        assert_eq!(
            plan().to_text(),
            "name\ttime\trows\tshare\tfilter\n\
             Hash Join\t12.5 ms\t1000\t100%\t\n  \
             Seq Scan users [hot]\t9.75 ms\t120000\t78%\tage > 30\n  \
             Index Scan orders\t250 µs\t3\t\t\n"
        );
    }

    #[test]
    fn rejects_bad_columns_and_values() {
        let mut spec = plan();
        spec.nodes[0].values.insert("rows".into(), json!("many"));
        let err = spec.validate().unwrap_err().to_string();
        assert!(err.contains("needs a number"), "{err}");
        let mut spec = plan();
        spec.columns[0].unit = Some("days".into());
        assert!(
            spec.validate()
                .unwrap_err()
                .to_string()
                .contains("time unit")
        );
        let mut spec = plan();
        spec.columns[1].name = "time".into();
        assert!(
            spec.validate()
                .unwrap_err()
                .to_string()
                .contains("appears twice")
        );
        // Deep chains are refused without recursion.
        let mut deep = TreeNode {
            label: "leaf".into(),
            badge: None,
            tone: None,
            values: Map::new(),
            children: Vec::new(),
        };
        for _ in 0..MAX_DEPTH {
            deep = TreeNode {
                label: "n".into(),
                children: vec![deep],
                ..plan().nodes[0].children[1].clone()
            };
        }
        let spec = TreeSpec {
            title: None,
            columns: Vec::new(),
            nodes: vec![deep],
            expand_depth: 1,
        };
        assert!(spec.validate().unwrap_err().to_string().contains("deeper"));
    }
}
