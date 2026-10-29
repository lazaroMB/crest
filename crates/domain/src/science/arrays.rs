//! An HDF5 or NetCDF file's structure with bounded previews
//! (`show_array_file`).
//!
//! The adapter lists the groups and variables (datasets) with their
//! types, shapes, dimension names, chunking, filters and attributes, and
//! reads a bounded preview of a few numeric variables: the first values
//! of a 1D variable, the first 2D slice of a higher one, never the whole
//! variable. Each preview carries statistics of the preview alone in a
//! [`Computed`] envelope, which says so.

use std::fmt::Write as _;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::block::BlockError;
use crate::computed::Computed;
use crate::widget::tree::{BadgeTone, ColumnKind, TreeColumn, TreeNode, TreeSpec};

/// Most groups and variables listed.
pub const MAX_NODES: usize = 5_000;
/// Most attributes listed per node.
pub const MAX_ATTRS: usize = 64;
/// Most variables previewed.
pub const MAX_PREVIEWS: usize = 6;
/// Most values of a 1D preview.
pub const MAX_LINE: usize = 5_000;
/// Most rows and columns of a 2D preview.
pub const MAX_SIDE: usize = 128;
/// Most strings of a text preview.
pub const MAX_TEXTS: usize = 50;

/// The container format.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ArrayFormat {
    /// HDF5.
    #[default]
    Hdf5,
    /// NetCDF-4 (HDF5 underneath).
    Netcdf4,
    /// NetCDF-3 classic (CDF-1).
    Netcdf3Classic,
    /// NetCDF-3 64-bit offset (CDF-2).
    Netcdf3Offset64,
    /// NetCDF-3 64-bit data (CDF-5).
    Netcdf3Data64,
}

impl ArrayFormat {
    /// Its name for people.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Hdf5 => "HDF5",
            Self::Netcdf4 => "NetCDF-4 (HDF5)",
            Self::Netcdf3Classic => "NetCDF-3 classic",
            Self::Netcdf3Offset64 => "NetCDF-3 64-bit offset",
            Self::Netcdf3Data64 => "NetCDF-3 64-bit data (CDF-5)",
        }
    }
}

/// A group or a variable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ArrayNodeKind {
    /// A group (the root is `/`).
    Group,
    /// A dataset or variable.
    Variable,
}

/// An attribute, its value as text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ArrayAttr {
    /// Its name.
    pub name: String,
    /// Its value, written out (arrays cut after a few values).
    pub value: String,
}

/// One group or variable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ArrayNode {
    /// Full path (`/sim/field`; `/` for the root).
    pub path: String,
    /// Nesting depth (root 0).
    pub depth: u32,
    /// Group or variable.
    pub kind: ArrayNodeKind,
    /// Element type (`f32`, `i16`, `string`, `compound{...}`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dtype: Option<String>,
    /// Extent per dimension (empty for a scalar or a group).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shape: Vec<u64>,
    /// Dimension names (NetCDF), one per extent.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dims: Vec<String>,
    /// Chunk shape, for a chunked variable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chunks: Option<Vec<u64>>,
    /// Filters in pipeline order (`shuffle`, `deflate`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub filters: Vec<String>,
    /// Attributes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attrs: Vec<ArrayAttr>,
}

impl ArrayNode {
    /// The last part of the path (`/` for the root).
    #[must_use]
    pub fn name(&self) -> &str {
        if self.path == "/" {
            "/"
        } else {
            self.path.rsplit('/').next().unwrap_or(&self.path)
        }
    }

    /// Elements in the variable (1 for a scalar).
    #[must_use]
    pub fn len(&self) -> u64 {
        self.shape.iter().product()
    }

    /// Whether it holds no elements.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.kind == ArrayNodeKind::Group || self.len() == 0
    }

    /// `40 × 30`, or `scalar`.
    #[must_use]
    pub fn shape_text(&self) -> String {
        if self.shape.is_empty() {
            return if self.kind == ArrayNodeKind::Group {
                String::new()
            } else {
                "scalar".into()
            };
        }
        let parts: Vec<String> = self
            .shape
            .iter()
            .enumerate()
            .map(|(i, n)| match self.dims.get(i) {
                Some(d) => format!("{d}={n}"),
                None => n.to_string(),
            })
            .collect();
        parts.join(" × ")
    }

    /// The attribute `name`'s value.
    #[must_use]
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|a| a.name == name)
            .map(|a| a.value.as_str())
    }
}

/// How a preview is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PreviewKind {
    /// One value.
    Scalar,
    /// The first values of a 1D variable, as a line.
    Line,
    /// A 2D slice, as a heatmap.
    Heatmap,
    /// Strings, as a list.
    Text,
}

/// A bounded preview of one variable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ArrayPreview {
    /// The variable's path.
    pub path: String,
    /// How it is drawn.
    pub kind: PreviewKind,
    /// What was read, as index ranges (`[0:2000]`, `[0, 0:8, 0:6]`).
    pub selection: String,
    /// Rows of a heatmap (1 otherwise).
    pub rows: usize,
    /// Columns of a heatmap, or the number of values.
    pub cols: usize,
    /// The values row by row; `None` for a fill or missing value.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<Option<f64>>,
    /// The strings of a text preview.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub texts: Vec<String>,
    /// How the stored numbers were turned into values (CF
    /// `scale_factor`/`add_offset`, the fill value left out).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decoding: Option<String>,
    /// Statistics of this preview only.
    pub stats: Computed,
}

impl ArrayPreview {
    /// The value at row `r`, column `c`.
    #[must_use]
    pub fn at(&self, r: usize, c: usize) -> Option<f64> {
        self.values.get(r * self.cols + c).copied().flatten()
    }

    /// The range of the present values.
    #[must_use]
    pub fn range(&self) -> Option<(f64, f64)> {
        self.values
            .iter()
            .flatten()
            .fold(None, |acc, &v| match acc {
                None => Some((v, v)),
                Some((lo, hi)) => Some((lo.min(v), hi.max(v))),
            })
    }

    /// The values as CSV: `index,value` for a line, a grid for a heatmap.
    #[must_use]
    pub fn to_csv(&self) -> String {
        let cell = |v: Option<f64>| v.map(|v| v.to_string()).unwrap_or_default();
        let mut out = String::new();
        match self.kind {
            PreviewKind::Heatmap => {
                for r in 0..self.rows {
                    let row: Vec<String> = (0..self.cols).map(|c| cell(self.at(r, c))).collect();
                    out.push_str(&row.join(","));
                    out.push('\n');
                }
            }
            PreviewKind::Text => {
                for t in &self.texts {
                    out.push_str(&csv_field(t));
                    out.push('\n');
                }
            }
            PreviewKind::Line | PreviewKind::Scalar => {
                out.push_str("index,value\n");
                for (i, v) in self.values.iter().enumerate() {
                    let _ = writeln!(out, "{i},{}", cell(*v));
                }
            }
        }
        out
    }
}

fn csv_field(text: &str) -> String {
    if text.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text.to_owned()
    }
}

/// An array file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ArrayFileSpec {
    /// Title shown above the inspector.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The workspace file read.
    pub path: String,
    /// Its format.
    pub format: ArrayFormat,
    /// Its size in bytes.
    pub size: u64,
    /// Groups and variables, depth first (the root group first).
    pub nodes: Vec<ArrayNode>,
    /// Whether the listing stopped at [`MAX_NODES`].
    #[serde(default)]
    pub truncated: bool,
    /// Bounded previews, at most [`MAX_PREVIEWS`].
    #[serde(default)]
    pub previews: Vec<ArrayPreview>,
    /// Reader notes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

fn invalid(reason: impl Into<String>) -> BlockError {
    BlockError::Invalid {
        kind: "array_file",
        reason: reason.into(),
    }
}

impl ArrayFileSpec {
    /// Checks sizes, previews and their envelopes.
    ///
    /// # Errors
    ///
    /// [`BlockError::Invalid`] naming the first problem.
    pub fn validate(&self) -> Result<(), BlockError> {
        if self.nodes.len() > MAX_NODES {
            return Err(invalid(format!(
                "{} nodes; at most {MAX_NODES}",
                self.nodes.len()
            )));
        }
        if self.previews.len() > MAX_PREVIEWS {
            return Err(invalid(format!(
                "{} previews; at most {MAX_PREVIEWS}",
                self.previews.len()
            )));
        }
        for p in &self.previews {
            let expected = match p.kind {
                PreviewKind::Heatmap => {
                    if p.rows > MAX_SIDE || p.cols > MAX_SIDE {
                        return Err(invalid(format!("preview of {} is too large", p.path)));
                    }
                    p.rows * p.cols
                }
                PreviewKind::Line => {
                    if p.cols > MAX_LINE {
                        return Err(invalid(format!("preview of {} is too long", p.path)));
                    }
                    p.cols
                }
                PreviewKind::Scalar => 1,
                PreviewKind::Text => 0,
            };
            if p.kind != PreviewKind::Text && p.values.len() != expected {
                return Err(invalid(format!(
                    "preview of {} has {} values for {expected} cells",
                    p.path,
                    p.values.len()
                )));
            }
            if p.values.iter().flatten().any(|v| !v.is_finite()) {
                return Err(invalid(format!(
                    "preview of {} has a non-finite value",
                    p.path
                )));
            }
            p.stats
                .validate()
                .map_err(|e| invalid(format!("preview envelope of {}: {e}", p.path)))?;
        }
        Ok(())
    }

    /// The variables.
    pub fn variables(&self) -> impl Iterator<Item = &ArrayNode> {
        self.nodes
            .iter()
            .filter(|n| n.kind == ArrayNodeKind::Variable)
    }

    /// The node at `path`.
    #[must_use]
    pub fn node(&self, path: &str) -> Option<&ArrayNode> {
        self.nodes.iter().find(|n| n.path == path)
    }

    /// The title or the file name.
    #[must_use]
    pub fn display_title(&self) -> String {
        if let Some(t) = self.title.as_ref().filter(|t| !t.trim().is_empty()) {
            return t.clone();
        }
        self.path
            .rsplit('/')
            .next()
            .unwrap_or(&self.path)
            .to_owned()
    }

    /// The structure on the tree table: groups holding their variables,
    /// columns type, shape, chunks and filters, attributes as children.
    #[must_use]
    pub fn to_tree(&self) -> TreeSpec {
        let columns = ["type", "shape", "chunks", "filters"]
            .into_iter()
            .map(|name| TreeColumn {
                name: name.to_owned(),
                kind: ColumnKind::Text,
                unit: None,
            })
            .collect();
        // Nodes are depth first: build children by depth with a stack.
        let mut stack: Vec<(u32, TreeNode)> = Vec::new();
        let mut roots: Vec<TreeNode> = Vec::new();
        let previewed = |path: &str| self.previews.iter().any(|p| p.path == path);
        for n in &self.nodes {
            let mut values = Map::new();
            if let Some(t) = &n.dtype {
                values.insert("type".into(), json!(t));
            }
            values.insert("shape".into(), json!(n.shape_text()));
            if let Some(c) = &n.chunks {
                let c: Vec<String> = c.iter().map(u64::to_string).collect();
                values.insert("chunks".into(), json!(c.join(" × ")));
            }
            if !n.filters.is_empty() {
                values.insert("filters".into(), json!(n.filters.join(", ")));
            }
            let attrs = n
                .attrs
                .iter()
                .map(|a| {
                    let mut values = Map::new();
                    values.insert("type".into(), Value::String(a.value.clone()));
                    TreeNode {
                        label: format!("@{}", a.name),
                        badge: None,
                        tone: None,
                        values,
                        children: Vec::new(),
                    }
                })
                .collect();
            let node = TreeNode {
                label: n.name().to_owned(),
                badge: match n.kind {
                    ArrayNodeKind::Group => Some("group".into()),
                    ArrayNodeKind::Variable if previewed(&n.path) => Some("preview".into()),
                    ArrayNodeKind::Variable => None,
                },
                tone: (n.kind == ArrayNodeKind::Variable && previewed(&n.path))
                    .then_some(BadgeTone::Info),
                values,
                children: attrs,
            };
            while stack.last().is_some_and(|(d, _)| *d >= n.depth) {
                pop(&mut stack, &mut roots);
            }
            stack.push((n.depth, node));
        }
        while !stack.is_empty() {
            pop(&mut stack, &mut roots);
        }
        TreeSpec {
            title: Some(self.display_title()),
            columns,
            nodes: roots,
            expand_depth: 2,
        }
    }

    /// Groups and variables as CSV: path, kind, type, shape, chunks,
    /// filters.
    #[must_use]
    pub fn nodes_csv(&self) -> String {
        let mut out = String::from("path,kind,type,shape,chunks,filters\n");
        for n in &self.nodes {
            let chunks = n
                .chunks
                .as_ref()
                .map(|c| c.iter().map(u64::to_string).collect::<Vec<_>>().join("x"));
            let shape: Vec<String> = n.shape.iter().map(u64::to_string).collect();
            let _ = writeln!(
                out,
                "{},{},{},{},{},{}",
                csv_field(&n.path),
                match n.kind {
                    ArrayNodeKind::Group => "group",
                    ArrayNodeKind::Variable => "variable",
                },
                csv_field(n.dtype.as_deref().unwrap_or("")),
                shape.join("x"),
                chunks.unwrap_or_default(),
                csv_field(&n.filters.join(";"))
            );
        }
        out
    }
}

/// Moves the top of `stack` into its parent (or the roots).
fn pop(stack: &mut Vec<(u32, TreeNode)>, roots: &mut Vec<TreeNode>) {
    if let Some((_, node)) = stack.pop() {
        match stack.last_mut() {
            // The parent's attributes come first, then its members.
            Some((_, parent)) => parent.children.push(node),
            None => roots.push(node),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::computed::Engine;

    fn node(path: &str, depth: u32, kind: ArrayNodeKind, shape: Vec<u64>) -> ArrayNode {
        ArrayNode {
            path: path.into(),
            depth,
            kind,
            dtype: (kind == ArrayNodeKind::Variable).then(|| "f32".into()),
            shape,
            dims: Vec::new(),
            chunks: None,
            filters: Vec::new(),
            attrs: Vec::new(),
        }
    }

    fn spec() -> ArrayFileSpec {
        let mut field = node("/sim/field", 2, ArrayNodeKind::Variable, vec![2, 3]);
        field.chunks = Some(vec![1, 3]);
        field.filters = vec!["deflate".into()];
        field.attrs = vec![ArrayAttr {
            name: "units".into(),
            value: "K".into(),
        }];
        ArrayFileSpec {
            title: None,
            path: "data/run.h5".into(),
            format: ArrayFormat::Hdf5,
            size: 100,
            nodes: vec![
                node("/", 0, ArrayNodeKind::Group, vec![]),
                node("/sim", 1, ArrayNodeKind::Group, vec![]),
                field,
                node("/series", 1, ArrayNodeKind::Variable, vec![4]),
            ],
            truncated: false,
            previews: vec![ArrayPreview {
                path: "/sim/field".into(),
                kind: PreviewKind::Heatmap,
                selection: "[0:2, 0:3]".into(),
                rows: 2,
                cols: 3,
                values: vec![Some(1.0), Some(2.0), None, Some(4.0), Some(5.0), Some(6.0)],
                texts: Vec::new(),
                decoding: None,
                stats: Computed::new(
                    "ar_1",
                    "array.preview_stats",
                    "Preview statistics",
                    Engine {
                        name: "crest-array".into(),
                        version: "0".into(),
                        backend: None,
                        platform: None,
                    },
                ),
            }],
            warnings: Vec::new(),
        }
    }

    #[test]
    fn tree_nests_by_depth_with_attributes() {
        let s = spec();
        s.validate().unwrap();
        let tree = s.to_tree();
        assert_eq!(tree.nodes.len(), 1, "one root");
        let root = &tree.nodes[0];
        assert_eq!(root.label, "/");
        assert_eq!(root.children.len(), 2, "sim and series");
        let field = &root.children[0].children[0];
        assert_eq!(field.label, "field");
        assert_eq!(field.badge.as_deref(), Some("preview"));
        assert_eq!(field.values["shape"], "2 × 3");
        assert_eq!(field.values["chunks"], "1 × 3");
        assert_eq!(field.children[0].label, "@units");
        assert_eq!(s.previews[0].at(1, 2), Some(6.0));
        assert_eq!(s.previews[0].range(), Some((1.0, 6.0)));
        assert_eq!(s.previews[0].to_csv(), "1,2,\n4,5,6\n");
        assert!(
            s.nodes_csv()
                .contains("/sim/field,variable,f32,2x3,1x3,deflate\n")
        );
        assert_eq!(s.nodes[2].name(), "field");
        let mut bad = spec();
        bad.previews[0].values.pop();
        assert!(bad.validate().is_err());
    }
}
