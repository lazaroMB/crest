//! Our own NetCDF-3 reader: classic (CDF-1), 64-bit offset (CDF-2) and
//! 64-bit data (CDF-5) headers, and rows of variables read straight from
//! their offsets.
//!
//! The `netcdf3` crate (0.6.1) only reads whole variables or whole
//! records, so a preview of a large fixed-size variable would decode all
//! of it; this reader seeks to the rows it needs instead. A record
//! variable's row `r` is at `begin + r × recsize`; a fixed one is
//! contiguous from `begin`. Values are big-endian.

use domain::science::arrays::{ArrayAttr, ArrayFormat, ArrayNode, ArrayNodeKind, MAX_ATTRS};

use super::{ArrayReader, Raw};

const NC_DIMENSION: u64 = 0x0A;
const NC_VARIABLE: u64 = 0x0B;
const NC_ATTRIBUTE: u64 = 0x0C;
/// Attribute values shown before cutting.
const ATTR_VALUES: usize = 8;

/// An external type: its name and size in bytes.
fn type_info(code: u32) -> Option<(&'static str, usize)> {
    Some(match code {
        1 => ("i8", 1),
        2 => ("char", 1),
        3 => ("i16", 2),
        4 => ("i32", 4),
        5 => ("f32", 4),
        6 => ("f64", 8),
        7 => ("u8", 1),
        8 => ("u16", 2),
        9 => ("u32", 4),
        10 => ("i64", 8),
        11 => ("u64", 8),
        _ => return None,
    })
}

/// Element `i` of type `code` in `bytes`.
#[allow(clippy::cast_precision_loss)]
fn value(code: u32, bytes: &[u8]) -> f64 {
    let n = |len: usize| -> [u8; 8] {
        let mut out = [0u8; 8];
        out[8 - len..].copy_from_slice(&bytes[..len]);
        out
    };
    match code {
        1 => f64::from(i8::from_be_bytes([bytes[0]])),
        2 | 7 => f64::from(bytes[0]),
        3 => f64::from(i16::from_be_bytes([bytes[0], bytes[1]])),
        8 => f64::from(u16::from_be_bytes([bytes[0], bytes[1]])),
        4 => f64::from(i32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])),
        9 => f64::from(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])),
        5 => f64::from(f32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])),
        6 => f64::from_be_bytes(n(8)),
        10 => i64::from_be_bytes(n(8)) as f64,
        11 => u64::from_be_bytes(n(8)) as f64,
        _ => f64::NAN,
    }
}

/// A cursor over the header.
struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
    /// CDF-5: counts are 64-bit.
    wide: bool,
}

impl Cursor<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], String> {
        let end = self
            .at
            .checked_add(n)
            .filter(|end| *end <= self.bytes.len())
            .ok_or("the NetCDF header is cut short")?;
        let out = &self.bytes[self.at..end];
        self.at = end;
        Ok(out)
    }

    fn u32(&mut self) -> Result<u32, String> {
        let b = self.take(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn u64(&mut self) -> Result<u64, String> {
        let b = self.take(8)?;
        let mut a = [0u8; 8];
        a.copy_from_slice(b);
        Ok(u64::from_be_bytes(a))
    }

    /// A non-negative count: 32-bit, or 64-bit in CDF-5.
    fn count(&mut self) -> Result<u64, String> {
        if self.wide {
            self.u64()
        } else {
            self.u32().map(u64::from)
        }
    }

    fn padded(&mut self, n: usize) -> Result<&[u8], String> {
        let pad = (4 - n % 4) % 4;
        let start = self.at;
        self.take(n + pad)?;
        Ok(&self.bytes[start..start + n])
    }

    fn name(&mut self) -> Result<String, String> {
        let n = usize::try_from(self.count()?).map_err(|_| "a name is too long")?;
        if n > 4096 {
            return Err("a name is longer than 4096 bytes".into());
        }
        let b = self.padded(n)?;
        Ok(String::from_utf8_lossy(b).into_owned())
    }

    /// An attribute list.
    fn attrs(&mut self) -> Result<Vec<ArrayAttr>, String> {
        let tag = self.u32()?;
        let n = self.count()?;
        if tag == 0 && n == 0 {
            return Ok(Vec::new());
        }
        if u64::from(tag) != NC_ATTRIBUTE {
            return Err(format!("expected an attribute list, found tag {tag}"));
        }
        let mut out = Vec::new();
        for _ in 0..n {
            let name = self.name()?;
            let code = self.u32()?;
            let count = usize::try_from(self.count()?).map_err(|_| "an attribute is too long")?;
            let (_, size) = type_info(code).ok_or(format!("attribute {name} has type {code}"))?;
            let data = self.padded(count.checked_mul(size).ok_or("an attribute is too long")?)?;
            let value = if code == 2 {
                String::from_utf8_lossy(data)
                    .trim_end_matches('\0')
                    .to_owned()
            } else {
                let values: Vec<String> = data
                    .chunks(size)
                    .take(ATTR_VALUES)
                    .map(|c| value(code, c).to_string())
                    .collect();
                let mut text = values.join(", ");
                if count > ATTR_VALUES {
                    text.push_str(&format!(", … {count} values"));
                }
                if count == 1 {
                    text
                } else {
                    format!("[{text}]")
                }
            };
            if out.len() < MAX_ATTRS {
                out.push(ArrayAttr { name, value });
            }
        }
        Ok(out)
    }
}

/// A variable as the header lists it: name, dimension ids, attributes,
/// type, vsize and begin.
type VarHeader = (String, Vec<u64>, Vec<ArrayAttr>, u32, u64, u64);

/// Where a variable's data is.
struct Layout {
    code: u32,
    size: usize,
    begin: u64,
    record: bool,
    /// Elements in one leading row (one record for a record variable).
    row: u64,
}

/// A parsed NetCDF-3 file over its bytes.
pub struct Reader {
    bytes: Vec<u8>,
    format: ArrayFormat,
    nodes: Vec<ArrayNode>,
    layouts: Vec<Option<Layout>>,
    recsize: u64,
    warnings: Vec<String>,
}

impl Reader {
    /// Parses the header of `bytes`.
    ///
    /// # Errors
    ///
    /// Why the header cannot be read.
    pub fn new(bytes: &[u8]) -> Result<Self, String> {
        let version = bytes.get(3).copied().ok_or("not a NetCDF file")?;
        let (format, wide, offset64) = match version {
            1 => (ArrayFormat::Netcdf3Classic, false, false),
            2 => (ArrayFormat::Netcdf3Offset64, false, true),
            5 => (ArrayFormat::Netcdf3Data64, true, true),
            v => return Err(format!("NetCDF-3 version byte {v} is not 1, 2 or 5")),
        };
        let mut c = Cursor { bytes, at: 4, wide };
        let numrecs = c.count()?;
        let mut warnings = Vec::new();
        // Dimensions.
        let tag = c.u32()?;
        let ndims = c.count()?;
        let mut dims: Vec<(String, u64)> = Vec::new();
        if !(tag == 0 && ndims == 0) {
            if u64::from(tag) != NC_DIMENSION {
                return Err(format!("expected the dimension list, found tag {tag}"));
            }
            for _ in 0..ndims.min(10_000) {
                let name = c.name()?;
                let len = c.count()?;
                dims.push((name, len));
            }
        }
        let global = c.attrs()?;
        // Variables.
        let tag = c.u32()?;
        let nvars = c.count()?;
        let mut vars: Vec<VarHeader> = Vec::new();
        if !(tag == 0 && nvars == 0) {
            if u64::from(tag) != NC_VARIABLE {
                return Err(format!("expected the variable list, found tag {tag}"));
            }
            for _ in 0..nvars.min(100_000) {
                let name = c.name()?;
                let rank = c.count()?;
                let mut ids = Vec::new();
                for _ in 0..rank.min(1024) {
                    ids.push(c.count()?);
                }
                let attrs = c.attrs()?;
                let code = c.u32()?;
                let vsize = c.count()?;
                let begin = if offset64 {
                    c.u64()?
                } else {
                    u64::from(c.u32()?)
                };
                vars.push((name, ids, attrs, code, vsize, begin));
            }
        }
        // A streamed file leaves numrecs at all ones: count the records.
        let record_vars: Vec<&VarHeader> = vars
            .iter()
            .filter(|v| {
                v.1.first()
                    .and_then(|d| usize::try_from(*d).ok())
                    .and_then(|d| dims.get(d))
                    .is_some_and(|(_, len)| *len == 0)
            })
            .collect();
        let mut recsize: u64 = record_vars.iter().map(|v| v.4).sum();
        if record_vars.len() == 1 {
            // A lone record variable is not padded.
            let v = record_vars[0];
            let per: u64 =
                v.1.iter()
                    .skip(1)
                    .filter_map(|d| dims.get(usize::try_from(*d).ok()?).map(|x| x.1))
                    .product();
            recsize = per * type_info(v.3).map_or(1, |t| t.1 as u64);
        }
        let numrecs = if numrecs == u64::from(u32::MAX) || numrecs == u64::MAX {
            let first = record_vars.iter().map(|v| v.5).min().unwrap_or(0);
            let n = (bytes.len() as u64).saturating_sub(first) / recsize.max(1);
            warnings.push(format!("streaming file: {n} records counted from its size"));
            n
        } else {
            numrecs
        };
        let mut nodes = vec![ArrayNode {
            path: "/".into(),
            depth: 0,
            kind: ArrayNodeKind::Group,
            dtype: None,
            shape: Vec::new(),
            dims: Vec::new(),
            chunks: None,
            filters: Vec::new(),
            attrs: global,
        }];
        let mut layouts = vec![None];
        for (name, ids, attrs, code, _vsize, begin) in vars {
            let mut shape = Vec::new();
            let mut names = Vec::new();
            for id in &ids {
                let (dname, len) = usize::try_from(*id)
                    .ok()
                    .and_then(|i| dims.get(i))
                    .ok_or_else(|| format!("variable {name} names dimension {id}"))?;
                names.push(dname.clone());
                shape.push(if *len == 0 { numrecs } else { *len });
            }
            let record = ids
                .first()
                .and_then(|d| dims.get(usize::try_from(*d).ok()?))
                .is_some_and(|(_, len)| *len == 0);
            let info = type_info(code);
            if info.is_none() {
                warnings.push(format!("variable {name} has unknown type {code}"));
            }
            let row = shape.iter().skip(1).product::<u64>();
            layouts.push(info.map(|(_, size)| Layout {
                code,
                size,
                begin,
                record,
                row: if shape.is_empty() { 1 } else { row },
            }));
            nodes.push(ArrayNode {
                path: format!("/{name}"),
                depth: 1,
                kind: ArrayNodeKind::Variable,
                dtype: info.map(|(t, _)| t.to_owned()),
                shape,
                dims: names,
                chunks: None,
                filters: Vec::new(),
                attrs,
            });
        }
        Ok(Self {
            bytes: bytes.to_vec(),
            format,
            nodes,
            layouts,
            recsize,
            warnings,
        })
    }
}

impl ArrayReader for Reader {
    fn format(&self) -> ArrayFormat {
        self.format
    }

    fn nodes(&self) -> &[ArrayNode] {
        &self.nodes
    }

    fn warnings(&self) -> Vec<String> {
        self.warnings.clone()
    }

    fn rows(&self, node: &ArrayNode, start: u64, count: u64) -> Result<Raw, String> {
        let ix = self
            .nodes
            .iter()
            .position(|n| n.path == node.path)
            .ok_or("no such variable")?;
        let layout = self.layouts[ix]
            .as_ref()
            .ok_or("the variable's type is not read")?;
        let rows = node.shape.first().copied().unwrap_or(1);
        let count = count.min(rows.saturating_sub(start));
        let row_bytes = layout.row * layout.size as u64;
        let mut raw = Vec::new();
        for r in start..start + count {
            let at = if layout.record {
                layout.begin + r * self.recsize
            } else {
                layout.begin + r * row_bytes
            };
            let (at, len) = (
                usize::try_from(at).map_err(|_| "offset too large")?,
                usize::try_from(row_bytes).map_err(|_| "row too large")?,
            );
            let slice = self
                .bytes
                .get(at..at + len)
                .ok_or("the variable's data lies past the end of the file")?;
            raw.extend_from_slice(slice);
        }
        if node.dtype.as_deref() == Some("char") {
            // Rows of characters are strings (the last dimension's length).
            let width = usize::try_from(node.shape.last().copied().unwrap_or(1).max(1))
                .map_err(|_| "row too large")?;
            let texts = if node.shape.len() <= 1 {
                vec![
                    String::from_utf8_lossy(&raw)
                        .trim_end_matches('\0')
                        .to_owned(),
                ]
            } else {
                raw.chunks(width)
                    .map(|c| String::from_utf8_lossy(c).trim_end_matches('\0').to_owned())
                    .collect()
            };
            return Ok(Raw::Texts(texts));
        }
        Ok(Raw::Numbers(
            raw.chunks_exact(layout.size)
                .map(|c| value(layout.code, c))
                .collect(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_are_big_endian() {
        assert_eq!(value(3, &[0xFF, 0xFE]), -2.0);
        assert_eq!(value(5, &1.5f32.to_be_bytes()), 1.5);
        assert_eq!(value(10, &(-3i64).to_be_bytes()), -3.0);
        assert!(Reader::new(b"CDF\x07").is_err());
        assert!(Reader::new(b"CDF\x01\0\0").is_err());
    }
}
