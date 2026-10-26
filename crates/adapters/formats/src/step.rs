//! STEP parts (ISO 10303-21, AP203/AP214/AP242 B-rep) tessellated with
//! truck: `truck-stepio` builds the shells, `truck-meshalgo` meshes them.
//!
//! The file is parsed once ([`ruststep`]); the same data section gives the
//! truck table and what the block lists besides the mesh: the schema and
//! originating system of the header, the length unit of the global unit
//! context, the product names and which solid (`MANIFOLD_SOLID_BREP`,
//! `BREP_WITH_VOIDS`, `SHELL_BASED_SURFACE_MODEL`) owns which shell, and
//! the product a solid's shape representation belongs to.
//!
//! Both truck crates are stale (2024) and panic on some real files (an
//! index out of bounds in the triangulation of some trimmed faces), so
//! every shell is meshed under `catch_unwind`: a shell that crashes is
//! left out with a note and the rest is shown. The whole read runs on a
//! thread of its own and is given up after [`TIME_LIMIT`] (the thread is
//! left to finish on its own; truck has no cancellation). Assembly
//! placements are not applied (truck 0.3 has no assembly reader): every
//! shell is drawn in the coordinates it was written in.

use std::collections::{HashMap, HashSet};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc;
use std::time::Duration;

use application::ports::formats::StepRequest;
use domain::engineering::cad::{CadModelSpec, CadSolid, MAX_SOLIDS};
use domain::model3d::{ModelMesh, UpAxis};
use ruststep::ast::{EntityInstance, Name, Parameter, Record};
use truck_meshalgo::prelude::*;
use truck_stepio::r#in::Table;

/// Most triangles a part may have (as the other 3D models).
pub const MAX_STEP_TRIANGLES: usize = 1_000_000;
/// How long a read may take.
pub const TIME_LIMIT: Duration = Duration::from_secs(60);
/// The default chord tolerance, as a share of the diagonal of the box
/// around the file's points.
pub const RELATIVE_TOLERANCE: f64 = 5e-4;

/// Parses and tessellates `bytes` on a thread of its own, within
/// [`TIME_LIMIT`].
///
/// # Errors
///
/// A message: not STEP, nothing meshed, over the limits, too slow, or the
/// reader crashed outside a shell.
pub fn parse(bytes: &[u8], request: &StepRequest) -> Result<CadModelSpec, String> {
    parse_within(bytes, request, TIME_LIMIT)
}

fn parse_within(
    bytes: &[u8],
    request: &StepRequest,
    limit: Duration,
) -> Result<CadModelSpec, String> {
    let text = String::from_utf8_lossy(bytes).into_owned();
    if !looks_like_step(&text) {
        return Err(format!(
            "`{}` is not a STEP file (no ISO-10303-21 header)",
            request.path
        ));
    }
    let request = request.clone();
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("step-reader".to_owned())
        .spawn(move || {
            let result = catch_unwind(AssertUnwindSafe(|| read(&text, &request)))
                .unwrap_or_else(|_| Err("the STEP reader crashed on this file".to_owned()));
            // The caller may have given up waiting.
            let _ = tx.send(result);
        })
        .map_err(|e| format!("cannot start the STEP reader: {e}"))?;
    match rx.recv_timeout(limit) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => Err(format!(
            "the STEP reader took longer than {} s; try a coarser `tolerance`",
            limit.as_secs()
        )),
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            Err("the STEP reader stopped without a result".to_owned())
        }
    }
}

/// Whether `text` starts (after blanks and comments) with the exchange
/// structure's keyword.
fn looks_like_step(text: &str) -> bool {
    let mut rest = text.trim_start_matches('\u{feff}').trim_start();
    while let Some(after) = rest.strip_prefix("/*") {
        match after.find("*/") {
            Some(end) => rest = after[end + 2..].trim_start(),
            None => return false,
        }
    }
    rest.starts_with("ISO-10303-21")
}

/// A shell's owner and the shells it is made of.
struct SolidDef {
    id: u64,
    name: String,
    shells: Vec<u64>,
}

fn read(text: &str, request: &StepRequest) -> Result<CadModelSpec, String> {
    let exchange = ruststep::parser::parse(text).map_err(|e| {
        format!(
            "`{}` is not valid STEP: {}",
            request.path,
            short(&e.to_string())
        )
    })?;
    let data = exchange
        .data
        .first()
        .ok_or_else(|| format!("`{}` has no DATA section", request.path))?;
    let header = Header::of(&exchange.header);
    let entities = Entities::of(&data.entities);
    let table = Table::from_data_section(data);

    let mut warnings = Vec::new();
    if entities.has_any(&[
        "NEXT_ASSEMBLY_USAGE_OCCURRENCE",
        "ITEM_DEFINED_TRANSFORMATION",
    ]) {
        warnings.push(
            "assembly placements are not applied: every part is drawn in its own coordinates"
                .to_owned(),
        );
    }
    let products = entities.products();
    let product_of_solid = entities.solid_products();
    let mut solids = entities.solids();
    let owned: HashSet<u64> = solids.iter().flat_map(|s| s.shells.clone()).collect();
    let mut loose: Vec<u64> = table
        .shell
        .keys()
        .copied()
        .filter(|id| !owned.contains(id))
        .collect();
    loose.sort_unstable();
    solids.extend(loose.into_iter().map(|id| SolidDef {
        id,
        name: format!("shell #{id}"),
        shells: vec![id],
    }));
    solids.retain(|s| s.shells.iter().any(|id| table.shell.contains_key(id)));
    if solids.is_empty() {
        return Err(format!(
            "`{}` has no B-rep solid or shell to show (an assembly that only \
             references other files, or a format truck does not read)",
            request.path
        ));
    }
    if solids.len() > MAX_SOLIDS {
        return Err(format!(
            "{} solids is over the limit of {MAX_SOLIDS}",
            solids.len()
        ));
    }
    let tolerance = match request.tolerance {
        Some(t) if t.is_finite() && t > 0.0 => t,
        Some(_) => return Err("`tolerance` must be a positive number".to_owned()),
        None => default_tolerance(&table),
    };

    let mut mesh = ModelMesh {
        positions: Vec::new(),
        triangles: Vec::new(),
        up: UpAxis::Z,
    };
    let mut out = Vec::with_capacity(solids.len());
    let single_product = (products.len() == 1).then(|| products[0].clone());
    for (ordinal, solid) in solids.iter().enumerate() {
        let first_triangle = mesh.triangles.len();
        let first_vertex = mesh.positions.len();
        let (mut faces, mut failed) = (0, 0);
        for shell_id in &solid.shells {
            let Some(holder) = table.shell.get(shell_id) else {
                continue;
            };
            let shell = match table.to_compressed_shell(holder) {
                Ok(shell) => shell,
                Err(err) => {
                    warnings.push(format!(
                        "{}: shell #{shell_id} could not be converted ({})",
                        solid.name,
                        short(&format!("{err:?}"))
                    ));
                    continue;
                }
            };
            let count = shell.faces.len();
            faces += count;
            let Ok(meshed) =
                catch_unwind(AssertUnwindSafe(|| shell.robust_triangulation(tolerance)))
            else {
                failed += count;
                warnings.push(format!(
                    "{}: the tessellator crashed on shell #{shell_id}; its {count} faces are not shown",
                    solid.name
                ));
                continue;
            };
            for face in &meshed.faces {
                match &face.surface {
                    Some(polygon) => append(&mut mesh, polygon)?,
                    None => failed += 1,
                }
            }
        }
        let triangles = mesh.triangles.len() - first_triangle;
        let (min, max) = bounds(&mesh.positions[first_vertex..]);
        let name = if solid.name.trim().is_empty() {
            format!("solid {}", ordinal + 1)
        } else {
            solid.name.clone()
        };
        out.push(CadSolid {
            name,
            product: product_of_solid
                .get(&solid.id)
                .cloned()
                .or_else(|| single_product.clone()),
            faces,
            failed_faces: failed,
            first_triangle,
            triangles,
            min,
            max,
        });
    }
    if mesh.triangles.is_empty() {
        let mut message = format!("no face of `{}` could be meshed", request.path);
        if let Some(first) = warnings.iter().find(|w| !w.starts_with("assembly")) {
            message.push_str(&format!(" ({first})"));
        }
        return Err(message);
    }
    Ok(CadModelSpec {
        title: None,
        path: request.path.clone(),
        schema: header.schema,
        application: header.application,
        unit: entities.length_unit(),
        products,
        solids: out,
        mesh,
        tolerance,
        warnings,
    })
}

/// Adds `polygon`'s triangles to `mesh`.
fn append(mesh: &mut ModelMesh, polygon: &PolygonMesh) -> Result<(), String> {
    let base = mesh.positions.len();
    // Coordinates are drawn in single precision.
    #[allow(clippy::cast_possible_truncation)]
    mesh.positions.extend(
        polygon
            .positions()
            .iter()
            .map(|p| [p.x as f32, p.y as f32, p.z as f32]),
    );
    for triangle in polygon.faces().triangle_iter() {
        if mesh.triangles.len() >= MAX_STEP_TRIANGLES {
            return Err(format!(
                "the part has more than {MAX_STEP_TRIANGLES} triangles; try a coarser `tolerance`"
            ));
        }
        let index = |v: &StandardVertex| {
            u32::try_from(base + v.pos).map_err(|_| "too many vertices".to_owned())
        };
        mesh.triangles.push([
            index(&triangle[0])?,
            index(&triangle[1])?,
            index(&triangle[2])?,
        ]);
    }
    Ok(())
}

/// The box around `points` (zeros when empty).
fn bounds(points: &[[f32; 3]]) -> ([f32; 3], [f32; 3]) {
    let Some(first) = points.first() else {
        return ([0.0; 3], [0.0; 3]);
    };
    points
        .iter()
        .fold((*first, *first), |(mut min, mut max), p| {
            for k in 0..3 {
                min[k] = min[k].min(p[k]);
                max[k] = max[k].max(p[k]);
            }
            (min, max)
        })
}

/// [`RELATIVE_TOLERANCE`] of the diagonal of the box around every
/// cartesian point of the file.
fn default_tolerance(table: &Table) -> f64 {
    let (mut min, mut max) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
    for point in table.cartesian_point.values() {
        for (k, &c) in point.coordinates.iter().take(3).enumerate() {
            if c.is_finite() {
                min[k] = min[k].min(c);
                max[k] = max[k].max(c);
            }
        }
    }
    let diagonal = (0..3)
        .map(|k| (max[k] - min[k]).powi(2))
        .sum::<f64>()
        .sqrt();
    if diagonal.is_finite() && diagonal > 0.0 {
        (diagonal * RELATIVE_TOLERANCE).max(1e-6)
    } else {
        0.01
    }
}

/// A message cut to one short line.
fn short(text: &str) -> String {
    let line = text.lines().next().unwrap_or("").trim();
    if line.chars().count() > 160 {
        format!("{}…", line.chars().take(160).collect::<String>())
    } else {
        line.to_owned()
    }
}

/// What the header says.
#[derive(Default)]
struct Header {
    schema: Option<String>,
    application: Option<String>,
}

impl Header {
    fn of(records: &[Record]) -> Self {
        let mut header = Self::default();
        for record in records {
            let args = args(record);
            match record.name.as_str() {
                "FILE_SCHEMA" => {
                    header.schema = args.first().and_then(|p| match p {
                        Parameter::List(items) => items.iter().find_map(string),
                        other => string(other),
                    });
                }
                "FILE_NAME" => {
                    // originating_system, else preprocessor_version.
                    header.application = args
                        .get(5)
                        .and_then(string)
                        .or_else(|| args.get(4).and_then(string));
                }
                _ => {}
            }
        }
        header
    }
}

/// The parameters of a record.
fn args(record: &Record) -> &[Parameter] {
    match &record.parameter {
        Parameter::List(items) => items,
        other => std::slice::from_ref(other),
    }
}

/// A non-empty string parameter, decoded.
fn string(p: &Parameter) -> Option<String> {
    match p {
        Parameter::String(s) => {
            let s = decode(s);
            (!s.trim().is_empty()).then(|| s.trim().to_owned())
        }
        Parameter::Typed { parameter, .. } => string(parameter),
        _ => None,
    }
}

fn reference(p: &Parameter) -> Option<u64> {
    match p {
        Parameter::Ref(Name::Entity(id)) => Some(*id),
        Parameter::Typed { parameter, .. } => reference(parameter),
        _ => None,
    }
}

fn references(p: &Parameter) -> Vec<u64> {
    match p {
        Parameter::List(items) => items.iter().filter_map(reference).collect(),
        other => reference(other).into_iter().collect(),
    }
}

fn enumeration(p: &Parameter) -> Option<&str> {
    match p {
        Parameter::Enumeration(e) => Some(e),
        _ => None,
    }
}

/// `\X2\00E9\X0\` (UTF-16 hex) and `\X\E9` (Latin-1) escapes of ISO
/// 10303-21 strings; the rest is kept.
fn decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find('\\') {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        if let Some(body) = tail.strip_prefix("\\X2\\")
            && let Some(end) = body.find("\\X0\\")
        {
            let units: Vec<u16> = body.as_bytes()[..end]
                .chunks(4)
                .filter_map(|c| u16::from_str_radix(std::str::from_utf8(c).ok()?, 16).ok())
                .collect();
            out.push_str(&String::from_utf16_lossy(&units));
            rest = &body[end + 4..];
        } else if let Some(body) = tail.strip_prefix("\\X\\")
            && body.len() >= 2
            && let Some(hex) = body.get(..2)
            && let Ok(code) = u8::from_str_radix(hex, 16)
        {
            out.push(char::from(code));
            rest = &body[2..];
        } else {
            out.push('\\');
            rest = &tail[1..];
        }
    }
    out.push_str(rest);
    out
}

/// The data section's records by entity id (a complex instance has
/// several).
struct Entities<'a> {
    by_id: HashMap<u64, Vec<&'a Record>>,
    ids: Vec<u64>,
}

impl<'a> Entities<'a> {
    fn of(instances: &'a [EntityInstance]) -> Self {
        let mut by_id: HashMap<u64, Vec<&Record>> = HashMap::new();
        let mut ids = Vec::with_capacity(instances.len());
        for instance in instances {
            let (id, records): (u64, Vec<&Record>) = match instance {
                EntityInstance::Simple { id, record } => (*id, vec![record]),
                EntityInstance::Complex { id, subsuper } => (*id, subsuper.0.iter().collect()),
            };
            ids.push(id);
            by_id.insert(id, records);
        }
        ids.sort_unstable();
        Self { by_id, ids }
    }

    /// The record named `name` of entity `id`.
    fn record(&self, id: u64, name: &str) -> Option<&'a Record> {
        self.by_id
            .get(&id)?
            .iter()
            .copied()
            .find(|r| r.name == name)
    }

    /// The first record of entity `id`, if it is a simple one.
    fn simple(&self, id: u64) -> Option<&'a Record> {
        match self.by_id.get(&id)?.as_slice() {
            [record] => Some(record),
            _ => None,
        }
    }

    /// Ids of the entities with a record named `name`, in id order.
    fn named<'s>(&'s self, name: &'s str) -> impl Iterator<Item = (u64, &'a Record)> + 's {
        self.ids
            .iter()
            .filter_map(move |id| self.record(*id, name).map(|r| (*id, r)))
    }

    fn has_any(&self, names: &[&str]) -> bool {
        self.by_id
            .values()
            .flatten()
            .any(|r| names.contains(&r.name.as_str()))
    }

    /// Product names (`PRODUCT(id, name, …)`), the id when the name is
    /// empty, in file order and without repeats.
    fn products(&self) -> Vec<String> {
        let mut seen = HashSet::new();
        self.named("PRODUCT")
            .filter_map(|(_, r)| {
                let a = args(r);
                a.get(1)
                    .and_then(string)
                    .or_else(|| a.first().and_then(string))
            })
            .filter(|name| seen.insert(name.clone()))
            .collect()
    }

    /// The solids and the shells each is made of, in id order.
    fn solids(&self) -> Vec<SolidDef> {
        let mut out = Vec::new();
        for id in &self.ids {
            let Some(records) = self.by_id.get(id) else {
                continue;
            };
            for record in records {
                let a = args(record);
                let shells = match record.name.as_str() {
                    "MANIFOLD_SOLID_BREP" | "FACETED_BREP" => {
                        a.get(1).and_then(reference).into_iter().collect()
                    }
                    "BREP_WITH_VOIDS" => {
                        let mut shells: Vec<u64> =
                            a.get(1).and_then(reference).into_iter().collect();
                        shells.extend(a.get(2).map(references).unwrap_or_default());
                        shells
                    }
                    "SHELL_BASED_SURFACE_MODEL" => a.get(1).map(references).unwrap_or_default(),
                    _ => continue,
                };
                // Oriented shells point at the shell they turn.
                let shells = shells
                    .into_iter()
                    .map(|s| match self.simple(s) {
                        Some(r)
                            if r.name == "ORIENTED_CLOSED_SHELL"
                                || r.name == "ORIENTED_OPEN_SHELL" =>
                        {
                            args(r).get(1).and_then(reference).unwrap_or(s)
                        }
                        _ => s,
                    })
                    .collect();
                out.push(SolidDef {
                    id: *id,
                    name: a.first().and_then(string).unwrap_or_default(),
                    shells,
                });
                break;
            }
        }
        out
    }

    /// The product name of each solid whose shape representation a
    /// product's shape definition uses, directly or through a
    /// representation relationship.
    fn solid_products(&self) -> HashMap<u64, String> {
        // Representation → product.
        let mut rep_product: HashMap<u64, String> = HashMap::new();
        for (_, sdr) in self.named("SHAPE_DEFINITION_REPRESENTATION") {
            let a = args(sdr);
            let (Some(definition), Some(rep)) =
                (a.first().and_then(reference), a.get(1).and_then(reference))
            else {
                continue;
            };
            if let Some(product) = self.product_of_definition(definition) {
                rep_product.insert(rep, product);
            }
        }
        // Relationships pass the product on, until nothing changes.
        let links: Vec<(u64, u64)> = self
            .ids
            .iter()
            .filter_map(|id| {
                let records = self.by_id.get(id)?;
                let r = records
                    .iter()
                    .find(|r| r.name == "SHAPE_REPRESENTATION_RELATIONSHIP")?;
                let a = args(r);
                Some((a.get(2).and_then(reference)?, a.get(3).and_then(reference)?))
            })
            .collect();
        for _ in 0..8 {
            let mut changed = false;
            for (a, b) in &links {
                match (rep_product.get(a).cloned(), rep_product.get(b).cloned()) {
                    (Some(p), None) => {
                        rep_product.insert(*b, p);
                        changed = true;
                    }
                    (None, Some(p)) => {
                        rep_product.insert(*a, p);
                        changed = true;
                    }
                    _ => {}
                }
            }
            if !changed {
                break;
            }
        }
        let mut out = HashMap::new();
        for (rep, product) in rep_product {
            let Some(record) = self
                .by_id
                .get(&rep)
                .and_then(|rs| rs.iter().find(|r| r.name.ends_with("REPRESENTATION")))
            else {
                continue;
            };
            for item in args(record).get(1).map(references).unwrap_or_default() {
                out.entry(item).or_insert_with(|| product.clone());
            }
        }
        out
    }

    /// `PRODUCT_DEFINITION_SHAPE` → `PRODUCT_DEFINITION` →
    /// `PRODUCT_DEFINITION_FORMATION` → `PRODUCT`.
    fn product_of_definition(&self, shape: u64) -> Option<String> {
        let pds = self.record(shape, "PRODUCT_DEFINITION_SHAPE")?;
        let pd_id = args(pds).get(2).and_then(reference)?;
        let pd = self.record(pd_id, "PRODUCT_DEFINITION")?;
        let formation = args(pd).get(2).and_then(reference)?;
        let pdf = self
            .by_id
            .get(&formation)?
            .iter()
            .find(|r| r.name.starts_with("PRODUCT_DEFINITION_FORMATION"))?;
        let product_id = args(pdf).get(2).and_then(reference)?;
        let product = self.record(product_id, "PRODUCT")?;
        let a = args(product);
        a.get(1)
            .and_then(string)
            .or_else(|| a.first().and_then(string))
    }

    /// The length unit of the global unit context (else the first length
    /// unit of the file), as a short symbol.
    fn length_unit(&self) -> Option<String> {
        let mut candidates: Vec<u64> = Vec::new();
        for id in &self.ids {
            if let Some(r) = self.record(*id, "GLOBAL_UNIT_ASSIGNED_CONTEXT") {
                candidates.extend(args(r).first().map(references).unwrap_or_default());
            }
        }
        let is_length = |id: &u64| self.record(*id, "LENGTH_UNIT").is_some();
        let unit = candidates
            .into_iter()
            .find(is_length)
            .or_else(|| self.ids.iter().copied().find(is_length))?;
        if let Some(si) = self.record(unit, "SI_UNIT") {
            let a = args(si);
            let prefix = a.first().and_then(enumeration);
            let name = a.get(1).and_then(enumeration)?;
            if name != "METRE" {
                return Some(name.to_ascii_lowercase());
            }
            let symbol = match prefix {
                None => "m",
                Some("MILLI") => "mm",
                Some("CENTI") => "cm",
                Some("DECI") => "dm",
                Some("MICRO") => "µm",
                Some("NANO") => "nm",
                Some("KILO") => "km",
                Some(other) => return Some(format!("{}metre", other.to_ascii_lowercase())),
            };
            return Some(symbol.to_owned());
        }
        let converted = self.record(unit, "CONVERSION_BASED_UNIT")?;
        let name = args(converted).first().and_then(string)?;
        Some(match name.to_ascii_uppercase().as_str() {
            "INCH" | "INCHES" => "in".to_owned(),
            "FOOT" | "FEET" => "ft".to_owned(),
            "METRE" | "METER" => "m".to_owned(),
            "MILLIMETRE" | "MILLIMETER" => "mm".to_owned(),
            "CENTIMETRE" | "CENTIMETER" => "cm".to_owned(),
            other => other.to_ascii_lowercase(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 10 × 20 × 5 mm block with a named solid in product `Block`.
    const CUBE: &str = include_str!("../tests/fixtures/engineering/block.step");

    fn request() -> StepRequest {
        StepRequest {
            path: "parts/block.step".into(),
            tolerance: None,
        }
    }

    #[test]
    fn a_block_is_meshed_with_its_names_and_unit() {
        let spec = parse(CUBE.as_bytes(), &request()).unwrap();
        assert_eq!(spec.validate(), Ok(()));
        assert_eq!(spec.unit.as_deref(), Some("mm"));
        assert_eq!(spec.products, ["Block"]);
        assert_eq!(spec.protocol(), Some("AP214"));
        assert_eq!(spec.solids.len(), 1);
        let solid = &spec.solids[0];
        assert_eq!(solid.name, "Body");
        assert_eq!(solid.product.as_deref(), Some("Block"));
        assert_eq!((solid.faces, solid.failed_faces), (6, 0));
        assert_eq!(solid.triangles, 12);
        assert_eq!(solid.size(), [10.0, 20.0, 5.0]);
        assert!(
            (spec.tolerance - 0.5 * RELATIVE_TOLERANCE * 45.0).abs() < 0.01,
            "{}",
            spec.tolerance
        );
    }

    #[test]
    fn other_files_and_bad_tolerances_are_refused() {
        let err = parse(b"solid x\nendsolid\n", &request()).unwrap_err();
        assert!(err.contains("not a STEP file"), "{err}");
        let err = parse(b"ISO-10303-21;\nHEADER;\nENDSEC;\n", &request()).unwrap_err();
        assert!(
            err.contains("not valid STEP") || err.contains("no DATA"),
            "{err}"
        );
        let bad = StepRequest {
            tolerance: Some(-1.0),
            ..request()
        };
        assert!(
            parse(CUBE.as_bytes(), &bad)
                .unwrap_err()
                .contains("tolerance")
        );
    }

    #[test]
    fn a_header_comment_is_skipped_and_escapes_decoded() {
        assert!(looks_like_step("/* made by hand */\nISO-10303-21;"));
        assert!(!looks_like_step("/* open"));
        assert_eq!(decode("Caf\\X\\E9 \\X2\\00FC00DF\\X0\\!"), "Café üß!");
        assert_eq!(decode("a\\b"), "a\\b");
    }
}
