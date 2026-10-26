//! HDF5 (and NetCDF-4, which is HDF5 underneath) through `hdf5-pure`.
//!
//! The file is opened from its bytes; groups are walked depth first
//! (members in name order) up to [`MAX_NODES`]. A dataset's rows are
//! decoded with `read_f64_rows` (or `read_string_rows`), which touches
//! only the chunks the window overlaps. NetCDF-4's own bookkeeping
//! attributes (`DIMENSION_LIST`, `REFERENCE_LIST`, `_Netcdf4Dimid`...) are
//! not listed.

use std::fmt::Write as _;

use domain::science::arrays::{
    ArrayAttr, ArrayFormat, ArrayNode, ArrayNodeKind, MAX_ATTRS, MAX_NODES,
};
use hdf5_pure::{AttrValue, DType, File};

use super::{ArrayReader, Raw};

/// Attributes NetCDF-4 keeps for itself.
const HIDDEN: &[&str] = &[
    "DIMENSION_LIST",
    "REFERENCE_LIST",
    "_Netcdf4Dimid",
    "_Netcdf4Coordinates",
    "_nc3_strict",
];
/// Array attributes are cut after this many values.
const ATTR_VALUES: usize = 8;

/// Whether `bytes` start like an HDF5 file (the signature at 0 or after a
/// user block of 512, 1024, … bytes).
#[must_use]
pub fn is_hdf5(bytes: &[u8]) -> bool {
    hdf5_pure::is_hdf5_bytes(bytes)
}

/// An open HDF5 file.
pub struct Reader {
    file: File,
    nodes: Vec<ArrayNode>,
    netcdf4: bool,
    warnings: Vec<String>,
}

fn list<T: ToString>(values: &[T]) -> String {
    let mut out = String::new();
    if values.len() != 1 {
        out.push('[');
    }
    for (i, v) in values.iter().take(ATTR_VALUES).enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str(&v.to_string());
    }
    if values.len() > ATTR_VALUES {
        let _ = write!(out, ", … {} values", values.len());
    }
    if values.len() != 1 {
        out.push(']');
    }
    out
}

/// An attribute's value as text.
fn attr_text(value: &AttrValue) -> String {
    match value {
        AttrValue::F32(v) => v.to_string(),
        AttrValue::F64(v) => v.to_string(),
        AttrValue::I8(v) => v.to_string(),
        AttrValue::I16(v) => v.to_string(),
        AttrValue::I32(v) => v.to_string(),
        AttrValue::I64(v) => v.to_string(),
        AttrValue::U8(v) => v.to_string(),
        AttrValue::U16(v) => v.to_string(),
        AttrValue::U32(v) => v.to_string(),
        AttrValue::U64(v) => v.to_string(),
        AttrValue::F32Array(v) => list(v),
        AttrValue::F64Array(v) => list(v),
        AttrValue::I8Array(v) => list(v),
        AttrValue::I16Array(v) => list(v),
        AttrValue::I32Array(v) => list(v),
        AttrValue::I64Array(v) => list(v),
        AttrValue::U8Array(v) => list(v),
        AttrValue::U16Array(v) => list(v),
        AttrValue::U32Array(v) => list(v),
        AttrValue::U64Array(v) => list(v),
        AttrValue::String(s) | AttrValue::AsciiString(s) => s.clone(),
        AttrValue::StringArray(v) | AttrValue::AsciiStringArray(v) => list(v),
        other => {
            let text = format!("{other:?}");
            text.chars().take(120).collect()
        }
    }
}

fn dtype_text(dtype: &DType) -> String {
    match dtype {
        DType::VariableLengthString | DType::String => "string".into(),
        other => {
            let text = other.to_string();
            text.chars().take(80).collect()
        }
    }
}

fn attrs(map: std::collections::HashMap<String, AttrValue>) -> Vec<ArrayAttr> {
    let mut attrs: Vec<ArrayAttr> = map
        .into_iter()
        .filter(|(name, _)| !HIDDEN.contains(&name.as_str()))
        .map(|(name, value)| ArrayAttr {
            value: attr_text(&value),
            name,
        })
        .collect();
    attrs.sort_by(|a, b| a.name.cmp(&b.name));
    attrs.truncate(MAX_ATTRS);
    attrs
}

/// The lookup path of a node path (`/sim/field` → `sim/field`).
fn lookup(path: &str) -> &str {
    path.trim_start_matches('/')
}

impl Reader {
    /// Opens `bytes` and lists its groups and datasets.
    ///
    /// # Errors
    ///
    /// The reader's message for a damaged or unsupported file.
    pub fn new(bytes: &[u8]) -> Result<Self, String> {
        let file = File::from_bytes(bytes.to_vec()).map_err(|e| format!("HDF5: {e}"))?;
        let mut reader = Self {
            file,
            nodes: Vec::new(),
            netcdf4: false,
            warnings: Vec::new(),
        };
        reader.walk("/", 0)?;
        reader.netcdf4 = reader
            .nodes
            .first()
            .is_some_and(|root| root.attrs.iter().any(|a| a.name == "_NCProperties"));
        Ok(reader)
    }

    fn walk(&mut self, path: &str, depth: u32) -> Result<(), String> {
        if self.nodes.len() >= MAX_NODES {
            return Ok(());
        }
        let group = if path == "/" {
            self.file.root()
        } else {
            self.file
                .group(lookup(path))
                .map_err(|e| format!("group {path}: {e}"))?
        };
        self.nodes.push(ArrayNode {
            path: path.to_owned(),
            depth,
            kind: ArrayNodeKind::Group,
            dtype: None,
            shape: Vec::new(),
            dims: Vec::new(),
            chunks: None,
            filters: Vec::new(),
            attrs: group.attrs().map(attrs).unwrap_or_default(),
        });
        let child = |name: &str| {
            if path == "/" {
                format!("/{name}")
            } else {
                format!("{path}/{name}")
            }
        };
        let mut datasets = group.datasets().map_err(|e| format!("group {path}: {e}"))?;
        datasets.sort();
        let mut groups = group.groups().map_err(|e| format!("group {path}: {e}"))?;
        groups.sort();
        for name in datasets {
            if self.nodes.len() >= MAX_NODES {
                self.warnings.push(format!(
                    "only the first {MAX_NODES} groups and variables are listed"
                ));
                return Ok(());
            }
            let full = child(&name);
            match self.file.dataset(lookup(&full)) {
                Ok(ds) => {
                    let filters = ds
                        .filter_pipeline()
                        .iter()
                        .map(ToString::to_string)
                        .collect();
                    self.nodes.push(ArrayNode {
                        dtype: ds.dtype().ok().map(|t| dtype_text(&t)),
                        shape: ds.shape().unwrap_or_default(),
                        dims: Vec::new(),
                        chunks: ds.chunk_shape().ok().flatten(),
                        filters,
                        attrs: ds.attrs().map(attrs).unwrap_or_default(),
                        path: full,
                        depth: depth + 1,
                        kind: ArrayNodeKind::Variable,
                    });
                }
                Err(e) => self.warnings.push(format!("dataset {full} not read: {e}")),
            }
        }
        for name in groups {
            self.walk(&child(&name), depth + 1)?;
        }
        Ok(())
    }
}

impl ArrayReader for Reader {
    fn format(&self) -> ArrayFormat {
        if self.netcdf4 {
            ArrayFormat::Netcdf4
        } else {
            ArrayFormat::Hdf5
        }
    }

    fn nodes(&self) -> &[ArrayNode] {
        &self.nodes
    }

    fn warnings(&self) -> Vec<String> {
        let mut out = self.warnings.clone();
        if self.netcdf4 {
            out.push("NetCDF-4 dimension names are not read; shapes are HDF5 extents".into());
        }
        out
    }

    fn rows(&self, node: &ArrayNode, start: u64, count: u64) -> Result<Raw, String> {
        let ds = self
            .file
            .dataset(lookup(&node.path))
            .map_err(|e| e.to_string())?;
        if node.dtype.as_deref() == Some("string") {
            let texts = if node.shape.is_empty() {
                ds.read_string()
            } else {
                ds.read_string_rows(start, count)
            };
            return texts.map(Raw::Texts).map_err(|e| e.to_string());
        }
        let numbers = if node.shape.is_empty() {
            ds.read_f64()
        } else {
            ds.read_f64_rows(start, count)
        };
        numbers.map(Raw::Numbers).map_err(|e| e.to_string())
    }
}
