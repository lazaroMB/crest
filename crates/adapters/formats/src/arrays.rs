//! Array files for `show_array_file`: HDF5 (and NetCDF-4) through
//! `hdf5-pure`, NetCDF-3 through our own reader ([`netcdf3`]).
//!
//! The readers list the groups and variables; this module plans the
//! previews and computes their statistics. A preview never decodes more
//! than [`READ_LIMIT`] elements: the first [`MAX_LINE`] values of a 1D
//! variable, the first rows (at most [`MAX_SIDE`], cropped to
//! [`MAX_SIDE`] columns) of a 2D one, and the `[0, …, 0, :, :]` slice of a
//! higher one, read as one leading-dimension row. Stored numbers are
//! decoded with the CF attributes when present (`_FillValue` and
//! `missing_value` left out, then `scale_factor` and `add_offset`). The
//! statistics (count, missing, min, max, mean, standard deviation) are of
//! the preview alone, and their envelope says which slice.

pub mod hdf5;
pub mod netcdf3;

use std::collections::BTreeMap;

use application::ports::formats::ArrayRequest;
use domain::computed::{Computed, ComputedValue};
use domain::science::arrays::{
    ArrayFileSpec, ArrayFormat, ArrayNode, ArrayNodeKind, ArrayPreview, MAX_LINE, MAX_PREVIEWS,
    MAX_SIDE, MAX_TEXTS, PreviewKind,
};
use serde_json::Value;

use crate::{chem, sci};

/// Most elements decoded for one preview.
pub const READ_LIMIT: u64 = 1_000_000;
/// Envelope kind of a preview's statistics.
pub const KIND: &str = "array.preview_stats";
/// Engine name of the array previews.
pub const ARRAY_ENGINE_NAME: &str = "crest-array";

/// What a reader decodes.
pub enum Raw {
    /// Numbers, as stored (before CF decoding).
    Numbers(Vec<f64>),
    /// Strings.
    Texts(Vec<String>),
}

/// A file's structure and a way to read rows of its variables.
pub trait ArrayReader {
    /// The format read.
    fn format(&self) -> ArrayFormat;
    /// Groups and variables, depth first, the root first.
    fn nodes(&self) -> &[ArrayNode];
    /// Reader notes.
    fn warnings(&self) -> Vec<String>;
    /// Elements of leading-dimension rows `start..start + count` of the
    /// variable at `path` (the one value of a scalar for `0, 1`).
    ///
    /// # Errors
    ///
    /// Why the rows cannot be read.
    fn rows(&self, node: &ArrayNode, start: u64, count: u64) -> Result<Raw, String>;
}

/// The array file of `bytes`.
///
/// # Errors
///
/// A message for a file that is neither HDF5 nor NetCDF-3, is damaged, or
/// has no variable `request.variable`.
pub fn inspect(bytes: &[u8], request: &ArrayRequest) -> Result<ArrayFileSpec, String> {
    let reader: Box<dyn ArrayReader> = if bytes.starts_with(b"CDF") {
        Box::new(netcdf3::Reader::new(bytes)?)
    } else if hdf5::is_hdf5(bytes) {
        Box::new(hdf5::Reader::new(bytes)?)
    } else {
        return Err("not an HDF5, NetCDF-4 or NetCDF-3 file (no signature)".into());
    };
    let nodes = reader.nodes().to_vec();
    let mut warnings = reader.warnings();
    let wanted: Vec<&ArrayNode> = match request.variable.as_deref().map(str::trim) {
        Some(name) if !name.is_empty() => {
            let found = nodes
                .iter()
                .filter(|n| n.kind == ArrayNodeKind::Variable)
                .find(|n| {
                    n.path == name
                        || n.path.trim_start_matches('/') == name.trim_start_matches('/')
                        || n.name() == name
                })
                .ok_or_else(|| {
                    let names: Vec<&str> = nodes
                        .iter()
                        .filter(|n| n.kind == ArrayNodeKind::Variable)
                        .take(20)
                        .map(|n| n.path.as_str())
                        .collect();
                    format!("no variable {name:?}; the file has {}", names.join(", "))
                })?;
            vec![found]
        }
        _ => default_previews(&nodes),
    };
    let mut previews = Vec::new();
    for node in wanted {
        match preview(reader.as_ref(), node, bytes, request) {
            Ok(Some(p)) => previews.push(p),
            Ok(None) => {}
            Err(e) => warnings.push(format!("no preview of {}: {e}", node.path)),
        }
    }
    let truncated = nodes.len() >= domain::science::arrays::MAX_NODES;
    Ok(ArrayFileSpec {
        title: None,
        path: request.path.clone(),
        format: reader.format(),
        size: bytes.len() as u64,
        nodes,
        truncated,
        previews,
        warnings,
    })
}

fn numeric(node: &ArrayNode) -> bool {
    node.kind == ArrayNodeKind::Variable
        && node.dtype.as_deref().is_some_and(|t| {
            matches!(
                t,
                "i8" | "u8" | "i16" | "u16" | "i32" | "u32" | "i64" | "u64" | "f32" | "f64"
            )
        })
        && !node.is_empty()
}

/// The variables previewed when none is asked for: numeric ones, 2D and
/// higher first (then 1D, then scalars), in file order.
fn default_previews(nodes: &[ArrayNode]) -> Vec<&ArrayNode> {
    let mut picked: Vec<&ArrayNode> = Vec::new();
    for pass in [2usize, 1, 0] {
        for n in nodes.iter().filter(|n| numeric(n)) {
            let rank = n.shape.len().min(2);
            if rank == pass && picked.len() < MAX_PREVIEWS {
                picked.push(n);
            }
        }
    }
    picked
}

/// The first number in an attribute value (`0.5`, `[0.5, 1]`).
fn attr_number(node: &ArrayNode, name: &str) -> Option<f64> {
    let text = node.attr(name)?;
    text.trim_matches(|c: char| c == '[' || c == ']' || c.is_whitespace())
        .split([',', ' '])
        .find(|s| !s.is_empty())?
        .parse()
        .ok()
}

/// CF decoding of `node`: fill and missing values, scale and offset.
struct Decoding {
    fill: Vec<f64>,
    scale: f64,
    offset: f64,
}

impl Decoding {
    fn of(node: &ArrayNode) -> Self {
        let fill = ["_FillValue", "missing_value"]
            .into_iter()
            .filter_map(|n| attr_number(node, n))
            .collect();
        Self {
            fill,
            scale: attr_number(node, "scale_factor").unwrap_or(1.0),
            offset: attr_number(node, "add_offset").unwrap_or(0.0),
        }
    }

    fn apply(&self, raw: f64) -> Option<f64> {
        if !raw.is_finite() || self.fill.contains(&raw) {
            return None;
        }
        let v = raw * self.scale + self.offset;
        v.is_finite().then_some(v)
    }

    fn text(&self) -> Option<String> {
        let mut parts = Vec::new();
        if !self.fill.is_empty() {
            let f: Vec<String> = self.fill.iter().map(|v| v.to_string()).collect();
            parts.push(format!("fill {} left out", f.join(", ")));
        }
        if self.scale != 1.0 || self.offset != 0.0 {
            parts.push(format!(
                "value = stored × {} + {} (scale_factor, add_offset)",
                self.scale, self.offset
            ));
        }
        (!parts.is_empty()).then(|| parts.join("; "))
    }
}

fn range(lo: u64, hi: u64) -> String {
    format!("{lo}:{hi}")
}

/// The preview of `node`, or `None` for one that cannot be shown.
fn preview(
    reader: &dyn ArrayReader,
    node: &ArrayNode,
    bytes: &[u8],
    request: &ArrayRequest,
) -> Result<Option<ArrayPreview>, String> {
    let shape = &node.shape;
    if node.is_empty() {
        return Ok(None);
    }
    let text = node.dtype.as_deref() == Some("string") || node.dtype.as_deref() == Some("char");
    let (kind, selection, rows, cols, raw) = match shape.len() {
        _ if text => {
            // Strings (or rows of NetCDF characters): the first few.
            let n = shape.first().map_or(1, |n| (*n).min(MAX_TEXTS as u64));
            let selection = if shape.is_empty() {
                "[]".to_owned()
            } else {
                format!("[{}]", range(0, n))
            };
            (PreviewKind::Text, selection, 1, 0, reader.rows(node, 0, n)?)
        }
        0 => (
            PreviewKind::Scalar,
            "[]".to_owned(),
            1,
            1,
            reader.rows(node, 0, 1)?,
        ),
        1 => {
            let n = shape[0].min(MAX_LINE as u64);
            #[allow(clippy::cast_possible_truncation)]
            let cols = n as usize;
            (
                PreviewKind::Line,
                format!("[{}]", range(0, n)),
                1,
                cols,
                reader.rows(node, 0, n)?,
            )
        }
        rank => {
            let per_row: u64 = shape[1..].iter().product();
            if per_row > READ_LIMIT {
                return Err(format!(
                    "one leading row holds {per_row} elements, more than the {READ_LIMIT} a \
                     preview decodes"
                ));
            }
            if rank == 2 {
                let want = shape[0]
                    .min(MAX_SIDE as u64)
                    .min((READ_LIMIT / per_row).max(1));
                let raw = reader.rows(node, 0, want)?;
                let c = shape[1].min(MAX_SIDE as u64);
                #[allow(clippy::cast_possible_truncation)]
                (
                    PreviewKind::Heatmap,
                    format!("[{}, {}]", range(0, want), range(0, c)),
                    want as usize,
                    c as usize,
                    crop(raw, shape[1], want, c),
                )
            } else {
                let (y, x) = (shape[rank - 2], shape[rank - 1]);
                let raw = reader.rows(node, 0, 1)?;
                // The row holds [0, …, 0, :, :] first.
                let raw = match raw {
                    Raw::Numbers(v) => Raw::Numbers(v.into_iter().take((y * x) as usize).collect()),
                    texts => texts,
                };
                let (r, c) = (y.min(MAX_SIDE as u64), x.min(MAX_SIDE as u64));
                let zeros = vec!["0"; rank - 2].join(", ");
                #[allow(clippy::cast_possible_truncation)]
                (
                    PreviewKind::Heatmap,
                    format!("[{zeros}, {}, {}]", range(0, r), range(0, c)),
                    r as usize,
                    c as usize,
                    crop(raw, x, r, c),
                )
            }
        }
    };
    let decoding = Decoding::of(node);
    let mut out = ArrayPreview {
        path: node.path.clone(),
        kind,
        selection: selection.clone(),
        rows,
        cols,
        values: Vec::new(),
        texts: Vec::new(),
        decoding: None,
        stats: Computed::new("ar_pending", KIND, "Preview statistics", engine(reader)),
    };
    match raw {
        Raw::Texts(texts) => {
            out.kind = PreviewKind::Text;
            out.texts = texts.into_iter().take(MAX_TEXTS).collect();
            out.rows = 1;
            out.cols = out.texts.len();
        }
        Raw::Numbers(numbers) => {
            if numbers.len() != rows * cols {
                return Err(format!(
                    "read {} values for a {rows} × {cols} preview",
                    numbers.len()
                ));
            }
            out.values = numbers.into_iter().map(|v| decoding.apply(v)).collect();
            out.decoding = decoding.text();
        }
    }
    out.stats = stats(&out, node, bytes, request, reader);
    Ok(Some(out))
}

/// Keeps the first `cols` of every row of `width` elements (`rows` rows).
fn crop(raw: Raw, width: u64, rows: u64, cols: u64) -> Raw {
    match raw {
        Raw::Numbers(v) if width != cols => {
            #[allow(clippy::cast_possible_truncation)]
            let (width, rows, cols) = (width as usize, rows as usize, cols as usize);
            Raw::Numbers(
                (0..rows)
                    .flat_map(|r| {
                        v.iter()
                            .skip(r * width)
                            .take(cols)
                            .copied()
                            .collect::<Vec<_>>()
                    })
                    .collect(),
            )
        }
        other => other,
    }
}

fn engine(reader: &dyn ArrayReader) -> domain::computed::Engine {
    let backend = match reader.format() {
        ArrayFormat::Hdf5 | ArrayFormat::Netcdf4 => "hdf5-pure 0.47.0",
        _ => "crest NetCDF-3 reader",
    };
    sci::engine(ARRAY_ENGINE_NAME, backend)
}

fn stats(
    preview: &ArrayPreview,
    node: &ArrayNode,
    bytes: &[u8],
    request: &ArrayRequest,
    reader: &dyn ArrayReader,
) -> Computed {
    let mut computed = Computed::new(
        sci::derive_id(
            "ar",
            &[
                KIND.as_bytes(),
                node.path.as_bytes(),
                preview.selection.as_bytes(),
                chem::hash(bytes).as_bytes(),
            ],
        ),
        KIND,
        "Descriptive statistics of the preview",
        engine(reader),
    );
    computed.method.why = Some(format!(
        "Over the preview only: {} of the {} variable {} ({} values read), not over the whole \
         variable; sd is the sample standard deviation (n − 1)",
        preview.selection,
        node.shape_text(),
        node.path,
        preview.values.len()
    ));
    let mut params = BTreeMap::new();
    params.insert("variable".into(), Value::String(node.path.clone()));
    params.insert("selection".into(), Value::String(preview.selection.clone()));
    if let Some(d) = &preview.decoding {
        params.insert("decoding".into(), Value::String(d.clone()));
    }
    computed.method.params = params;
    let mut input = chem::input("file", "data", bytes, Some(&request.path));
    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
    {
        input.rows = Some(preview.values.len() as u64);
    }
    computed.inputs = vec![input];
    let present: Vec<f64> = preview.values.iter().flatten().copied().collect();
    let v = &mut computed.values;
    #[allow(clippy::cast_precision_loss)]
    {
        v.insert("n".into(), ComputedValue::new(present.len() as f64));
        v.insert(
            "missing".into(),
            ComputedValue::new((preview.values.len() - present.len()) as f64),
        );
    }
    if !present.is_empty() {
        #[allow(clippy::cast_precision_loss)]
        let n = present.len() as f64;
        let mean = present.iter().sum::<f64>() / n;
        let min = present.iter().copied().fold(f64::INFINITY, f64::min);
        let max = present.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        v.insert("min".into(), ComputedValue::new(min));
        v.insert("max".into(), ComputedValue::new(max));
        v.insert("mean".into(), ComputedValue::new(mean));
        if present.len() > 1 {
            let var = present.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (n - 1.0);
            v.insert("sd".into(), ComputedValue::new(var.sqrt()));
        }
    }
    computed
}
