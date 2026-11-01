//! Stanford PLY, ASCII or binary (either endianness). The `vertex`
//! element gives the positions (`x`, `y`, `z`; other properties are
//! skipped) and the `face` element the polygons (`vertex_indices` or
//! `vertex_index`), triangulated as fans. Other elements are skipped.

use domain::model3d::{ModelMesh, UpAxis};

use super::{ModelError, check_triangles, index};

/// Largest capacity reserved up front from a count in the header.
const MAX_RESERVE: usize = 1 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Ascii,
    LittleEndian,
    BigEndian,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scalar {
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    F32,
    F64,
}

impl Scalar {
    fn parse(name: &str) -> Result<Self, ModelError> {
        Ok(match name {
            "char" | "int8" => Self::I8,
            "uchar" | "uint8" => Self::U8,
            "short" | "int16" => Self::I16,
            "ushort" | "uint16" => Self::U16,
            "int" | "int32" => Self::I32,
            "uint" | "uint32" => Self::U32,
            "float" | "float32" => Self::F32,
            "double" | "float64" => Self::F64,
            other => return Err(error(format!("unknown property type {other:?}"))),
        })
    }

    fn size(self) -> usize {
        match self {
            Self::I8 | Self::U8 => 1,
            Self::I16 | Self::U16 => 2,
            Self::I32 | Self::U32 | Self::F32 => 4,
            Self::F64 => 8,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Property {
    Scalar {
        name: String,
        ty: Scalar,
    },
    List {
        name: String,
        count: Scalar,
        item: Scalar,
    },
}

impl Property {
    fn name(&self) -> &str {
        match self {
            Self::Scalar { name, .. } | Self::List { name, .. } => name,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Element {
    name: String,
    count: usize,
    properties: Vec<Property>,
}

fn error(message: impl Into<String>) -> ModelError {
    ModelError::Parse(format!("PLY: {}", message.into()))
}

pub(super) fn parse(bytes: &[u8]) -> Result<ModelMesh, ModelError> {
    let (format, elements, body) = header(bytes)?;
    let mut reader = match format {
        Format::Ascii => Reader::Ascii(
            std::str::from_utf8(body)
                .map_err(|_| error("the ASCII body is not text"))?
                .split_ascii_whitespace(),
        ),
        Format::LittleEndian | Format::BigEndian => Reader::Binary {
            bytes: body,
            at: 0,
            little: format == Format::LittleEndian,
        },
    };
    let mut mesh = ModelMesh {
        up: UpAxis::Z,
        ..ModelMesh::default()
    };
    for element in &elements {
        match element.name.as_str() {
            "vertex" => vertices(&mut reader, element, &mut mesh.positions)?,
            "face" => faces(&mut reader, element, &mut mesh.triangles)?,
            _ => skip(&mut reader, element)?,
        }
    }
    Ok(mesh)
}

/// The format, the elements and the body after `end_header`.
fn header(bytes: &[u8]) -> Result<(Format, Vec<Element>, &[u8]), ModelError> {
    let mut rest = bytes;
    let mut next_line = || -> Result<&str, ModelError> {
        let end = rest
            .iter()
            .position(|&b| b == b'\n')
            .ok_or_else(|| error("the header has no end_header line"))?;
        let line =
            std::str::from_utf8(&rest[..end]).map_err(|_| error("the header is not text"))?;
        rest = &rest[end + 1..];
        Ok(line.trim_end_matches('\r').trim())
    };
    if next_line()? != "ply" {
        return Err(error("the file does not start with \"ply\""));
    }
    let mut format = None;
    let mut elements: Vec<Element> = Vec::new();
    loop {
        let line = next_line()?;
        let words: Vec<&str> = line.split_ascii_whitespace().collect();
        match words.as_slice() {
            ["end_header"] => break,
            [] | ["comment" | "obj_info", ..] => {}
            ["format", name, _version] => {
                format = Some(match *name {
                    "ascii" => Format::Ascii,
                    "binary_little_endian" => Format::LittleEndian,
                    "binary_big_endian" => Format::BigEndian,
                    other => return Err(error(format!("unknown format {other:?}"))),
                });
            }
            ["element", name, count] => elements.push(Element {
                name: (*name).to_owned(),
                count: count
                    .parse()
                    .map_err(|_| error(format!("bad element count {count:?}")))?,
                properties: Vec::new(),
            }),
            ["property", "list", count, item, name] => {
                let element = elements
                    .last_mut()
                    .ok_or_else(|| error("a property before any element"))?;
                element.properties.push(Property::List {
                    name: (*name).to_owned(),
                    count: Scalar::parse(count)?,
                    item: Scalar::parse(item)?,
                });
            }
            ["property", ty, name] => {
                let element = elements
                    .last_mut()
                    .ok_or_else(|| error("a property before any element"))?;
                element.properties.push(Property::Scalar {
                    name: (*name).to_owned(),
                    ty: Scalar::parse(ty)?,
                });
            }
            _ => return Err(error(format!("unexpected header line {line:?}"))),
        }
    }
    let format = format.ok_or_else(|| error("the header has no format line"))?;
    Ok((format, elements, rest))
}

/// Reads the values of the body, in order.
enum Reader<'a> {
    Ascii(std::str::SplitAsciiWhitespace<'a>),
    Binary {
        bytes: &'a [u8],
        at: usize,
        little: bool,
    },
}

impl Reader<'_> {
    fn value(&mut self, ty: Scalar) -> Result<f64, ModelError> {
        match self {
            Self::Ascii(words) => {
                let word = words.next().ok_or_else(|| error("the body ends early"))?;
                word.parse()
                    .map_err(|_| error(format!("bad number {word:?}")))
            }
            Self::Binary { bytes, at, little } => {
                let size = ty.size();
                let raw = bytes
                    .get(*at..*at + size)
                    .ok_or_else(|| error("the body ends early"))?;
                *at += size;
                let mut buf = [0_u8; 8];
                buf[..size].copy_from_slice(raw);
                if !*little {
                    buf[..size].reverse();
                }
                Ok(match ty {
                    Scalar::I8 => f64::from(i8::from_le_bytes([buf[0]])),
                    Scalar::U8 => f64::from(buf[0]),
                    Scalar::I16 => f64::from(i16::from_le_bytes([buf[0], buf[1]])),
                    Scalar::U16 => f64::from(u16::from_le_bytes([buf[0], buf[1]])),
                    Scalar::I32 => f64::from(i32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]])),
                    Scalar::U32 => f64::from(u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]])),
                    Scalar::F32 => f64::from(f32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]])),
                    Scalar::F64 => f64::from_le_bytes(buf),
                })
            }
        }
    }

    /// A list length or vertex index: a whole, non-negative number.
    fn whole(&mut self, ty: Scalar) -> Result<usize, ModelError> {
        let value = self.value(ty)?;
        if value < 0.0 || value.fract() != 0.0 || value > f64::from(u32::MAX) {
            return Err(error(format!("{value} is not a valid count or index")));
        }
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        Ok(value as usize)
    }

    /// Reads one property and drops it.
    fn skip(&mut self, property: &Property) -> Result<(), ModelError> {
        match property {
            Property::Scalar { ty, .. } => self.value(*ty).map(drop),
            Property::List { count, item, .. } => {
                let n = self.whole(*count)?;
                (0..n).try_for_each(|_| self.value(*item).map(drop))
            }
        }
    }
}

fn vertices(
    reader: &mut Reader<'_>,
    element: &Element,
    positions: &mut Vec<[f32; 3]>,
) -> Result<(), ModelError> {
    let axis = |name: &str| {
        element
            .properties
            .iter()
            .position(|p| matches!(p, Property::Scalar { name: n, .. } if n == name))
            .ok_or_else(|| error(format!("the vertex element has no {name} property")))
    };
    let (x, y, z) = (axis("x")?, axis("y")?, axis("z")?);
    positions.reserve(element.count.min(MAX_RESERVE));
    for _ in 0..element.count {
        let mut p = [0.0_f32; 3];
        for (i, property) in element.properties.iter().enumerate() {
            let slot = [x, y, z].iter().position(|&axis| axis == i);
            match (slot, property) {
                (Some(slot), Property::Scalar { ty, .. }) => {
                    #[allow(clippy::cast_possible_truncation)]
                    let value = reader.value(*ty)? as f32;
                    p[slot] = value;
                }
                _ => reader.skip(property)?,
            }
        }
        positions.push(p);
    }
    Ok(())
}

fn faces(
    reader: &mut Reader<'_>,
    element: &Element,
    triangles: &mut Vec<[u32; 3]>,
) -> Result<(), ModelError> {
    let list = element
        .properties
        .iter()
        .position(|p| {
            matches!(p, Property::List { .. })
                && matches!(p.name(), "vertex_indices" | "vertex_index")
        })
        .ok_or_else(|| error("the face element has no vertex_indices list"))?;
    triangles.reserve(element.count.min(MAX_RESERVE));
    let mut polygon = Vec::new();
    for _ in 0..element.count {
        for (i, property) in element.properties.iter().enumerate() {
            match property {
                Property::List { count, item, .. } if i == list => {
                    let n = reader.whole(*count)?;
                    polygon.clear();
                    for _ in 0..n {
                        polygon.push(index(reader.whole(*item)?)?);
                    }
                }
                _ => reader.skip(property)?,
            }
        }
        for k in 1..polygon.len().saturating_sub(1) {
            triangles.push([polygon[0], polygon[k], polygon[k + 1]]);
        }
        check_triangles(triangles.len())?;
    }
    Ok(())
}

fn skip(reader: &mut Reader<'_>, element: &Element) -> Result<(), ModelError> {
    for _ in 0..element.count {
        element
            .properties
            .iter()
            .try_for_each(|property| reader.skip(property))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str = "element vertex 4\nproperty float x\nproperty float y\n\
                          property float z\nproperty uchar red\n\
                          element face 1\nproperty uchar flags\n\
                          property list uchar int vertex_indices\nend_header\n";

    #[test]
    fn ascii_quads_become_two_triangles_and_extra_properties_are_skipped() {
        let text = format!(
            "ply\nformat ascii 1.0\ncomment made by hand\n{HEADER}\
             0 0 0 255\n1 0 0 255\n1 1 0 255\n0 1 0 255\n7 4 0 1 2 3\n"
        );
        let mesh = parse(text.as_bytes()).unwrap();
        assert_eq!(mesh.positions[2], [1.0, 1.0, 0.0]);
        assert_eq!(mesh.triangles, [[0, 1, 2], [0, 2, 3]]);
        assert_eq!(mesh.up, UpAxis::Z);
    }

    fn binary(little: bool) -> Vec<u8> {
        let format = if little {
            "binary_little_endian"
        } else {
            "binary_big_endian"
        };
        let mut out = format!("ply\nformat {format} 1.0\n{HEADER}").into_bytes();
        let f32s = |out: &mut Vec<u8>, v: f32| {
            out.extend(if little {
                v.to_le_bytes()
            } else {
                v.to_be_bytes()
            });
        };
        for p in [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 2.5],
        ] {
            for c in p {
                f32s(&mut out, c);
            }
            out.push(255);
        }
        out.extend([7, 4]);
        for i in [0_i32, 1, 2, 3] {
            out.extend(if little {
                i.to_le_bytes()
            } else {
                i.to_be_bytes()
            });
        }
        out
    }

    #[test]
    fn binary_files_of_either_endianness_are_read() {
        for little in [true, false] {
            let mesh = parse(&binary(little)).unwrap();
            assert_eq!(mesh.positions[3], [0.0, 1.0, 2.5], "little: {little}");
            assert_eq!(mesh.triangles, [[0, 1, 2], [0, 2, 3]], "little: {little}");
        }
    }

    #[test]
    fn broken_files_are_explained() {
        let cases: [&[u8]; 5] = [
            b"",
            b"obj\n",
            b"ply\nformat ascii 1.0\nelement vertex 1\nproperty float x\nend_header\n0\n",
            b"ply\nformat ascii 1.0\nelement vertex 1\nproperty float x\nproperty float y\n\
              property float z\nend_header\n0 0\n",
            b"ply\nformat ascii 1.0\nelement face 1\nproperty list uchar int vertex_indices\n\
              end_header\n3 0 -1 2\n",
        ];
        for bytes in cases {
            assert!(
                matches!(parse(bytes), Err(ModelError::Parse(_))),
                "{:?}",
                String::from_utf8_lossy(bytes)
            );
        }
        let mut cut = binary(true);
        cut.truncate(cut.len() - 3);
        assert!(matches!(parse(&cut), Err(ModelError::Parse(_))));
    }
}
