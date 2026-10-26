//! IFC room schedules (`show_ifc_schedule`) on IFClite's STEP scanner and
//! decoder (`ifc-lite-core`, MPL-2.0, used unmodified).
//!
//! Only the entities a schedule needs are decoded: the project and its
//! unit assignment, the spatial structure (site, buildings, storeys and
//! spaces through `IfcRelAggregates`, and `IfcRelContainedInSpatialStructure`
//! for spaces some exporters contain instead), the walls, doors and
//! windows contained in each storey (or in one of its spaces), and the
//! element quantities and property sets assigned to spaces through
//! `IfcRelDefinesByProperties`. A space's area is, in this order: the
//! `NetFloorArea`, then the `GrossFloorArea` of `Qto_SpaceBaseQuantities`
//! (or `BaseQuantities`), the `NetPlannedArea`, then `GrossPlannedArea`, of
//! `Pset_SpaceCommon`, or any other `IfcQuantityArea` named like a floor
//! area; the source is kept with it. Nothing is computed from geometry.
//! Attribute positions are the same in IFC2X3, IFC4 and IFC4X3 for every
//! attribute read.

use std::collections::HashMap;

use domain::engineering::ifc::{ElementCounts, IfcScheduleSpec, IfcSpace, IfcStorey};
use ifc_lite_core::{
    AttributeValue, DecodedEntity, EntityDecoder, EntityScanner, decode_ifc_string,
};

/// Most entities scanned.
pub const MAX_ENTITIES: usize = 20_000_000;

/// An entity found by the scanner: its type (upper case) and where its
/// record is.
struct Found {
    kind: String,
    start: usize,
    end: usize,
}

/// Reads `bytes` as an IFC file; `path` names it.
///
/// # Errors
///
/// A message when the file is not IFC (no `IFC` schema in the header) or
/// has neither storeys nor spaces.
pub fn parse(bytes: &[u8], path: &str) -> Result<IfcScheduleSpec, String> {
    let schema = ifc_lite_core::declared_schema_bounded(bytes)
        .filter(|s| s.to_ascii_uppercase().starts_with("IFC"))
        .ok_or_else(|| format!("`{path}` is not an IFC file (no IFC schema in FILE_SCHEMA)"))?;
    let mut model = Model::scan(bytes)?;
    model.build(path, schema.to_ascii_uppercase())
}

struct Model<'a> {
    found: HashMap<u32, Found>,
    by_kind: HashMap<String, Vec<u32>>,
    decoder: EntityDecoder<'a>,
}

impl<'a> Model<'a> {
    fn scan(bytes: &'a [u8]) -> Result<Self, String> {
        let mut scanner = EntityScanner::new(bytes);
        let mut found = HashMap::new();
        let mut by_kind: HashMap<String, Vec<u32>> = HashMap::new();
        while let Some((id, kind, start, end)) = scanner.next_entity() {
            if found.len() >= MAX_ENTITIES {
                return Err(format!("the file has more than {MAX_ENTITIES} entities"));
            }
            let kind = kind.to_ascii_uppercase();
            if interesting(&kind) {
                by_kind.entry(kind.clone()).or_default().push(id);
            }
            found.insert(id, Found { kind, start, end });
        }
        for ids in by_kind.values_mut() {
            ids.sort_unstable();
        }
        Ok(Self {
            found,
            by_kind,
            decoder: EntityDecoder::new(bytes),
        })
    }

    fn ids(&self, kind: &str) -> Vec<u32> {
        self.by_kind.get(kind).cloned().unwrap_or_default()
    }

    fn kind(&self, id: u32) -> Option<&str> {
        self.found.get(&id).map(|f| f.kind.as_str())
    }

    fn decode(&mut self, id: u32) -> Option<DecodedEntity> {
        let f = self.found.get(&id)?;
        self.decoder.decode_at(f.start, f.end).ok()
    }

    fn text(&mut self, id: u32, index: usize) -> Option<String> {
        let e = self.decode(id)?;
        text(e.get(index)?)
    }

    fn build(&mut self, path: &str, schema: String) -> Result<IfcScheduleSpec, String> {
        let mut warnings = Vec::new();
        // Spatial parents (child → parent) from aggregation and from the
        // containment of spaces.
        let mut parent: HashMap<u32, u32> = HashMap::new();
        for rel in self.ids("IFCRELAGGREGATES") {
            let Some(e) = self.decode(rel) else { continue };
            let (Some(whole), Some(parts)) = (e.get_ref(4), e.get(5).map(refs)) else {
                continue;
            };
            for part in parts {
                parent.entry(part).or_insert(whole);
            }
        }
        // Element → the structure containing it.
        let mut container: HashMap<u32, u32> = HashMap::new();
        for rel in self.ids("IFCRELCONTAINEDINSPATIALSTRUCTURE") {
            let Some(e) = self.decode(rel) else { continue };
            let (Some(elements), Some(structure)) = (e.get(4).map(refs), e.get_ref(5)) else {
                continue;
            };
            for element in elements {
                container.entry(element).or_insert(structure);
                if self.kind(element) == Some("IFCSPACE") {
                    parent.entry(element).or_insert(structure);
                }
            }
        }
        // The storey above `id` in the spatial tree.
        let storey_of = |model: &Self, mut id: u32| -> Option<u32> {
            for _ in 0..64 {
                if model.kind(id) == Some("IFCBUILDINGSTOREY") {
                    return Some(id);
                }
                id = *parent.get(&id)?;
            }
            None
        };
        let building_of = |model: &Self, mut id: u32| -> Option<u32> {
            for _ in 0..64 {
                if model.kind(id) == Some("IFCBUILDING") {
                    return Some(id);
                }
                id = *parent.get(&id)?;
            }
            None
        };

        let areas = self.space_areas();
        let mut storeys: Vec<(u32, IfcStorey)> = Vec::new();
        for id in self.ids("IFCBUILDINGSTOREY") {
            let Some(e) = self.decode(id) else { continue };
            let name = e
                .get(2)
                .and_then(text)
                .or_else(|| e.get(7).and_then(text))
                .unwrap_or_else(|| format!("storey #{id}"));
            let elevation = e.get(9).and_then(AttributeValue::as_float);
            let building = building_of(self, id).and_then(|b| self.text(b, 2));
            storeys.push((
                id,
                IfcStorey {
                    name,
                    elevation,
                    building,
                    spaces: Vec::new(),
                    elements: ElementCounts::default(),
                },
            ));
        }
        let index: HashMap<u32, usize> = storeys
            .iter()
            .enumerate()
            .map(|(i, (id, _))| (*id, i))
            .collect();
        let mut unplaced_spaces = Vec::new();
        for id in self.ids("IFCSPACE") {
            let Some(e) = self.decode(id) else { continue };
            let (area, area_source) = match areas.get(&id) {
                Some((value, source)) => (Some(*value), Some(source.clone())),
                None => (None, None),
            };
            let space = IfcSpace {
                name: e.get(2).and_then(text),
                long_name: e.get(7).and_then(text),
                area,
                area_source,
                global_id: e.get(0).and_then(text),
            };
            match storey_of(self, id).and_then(|s| index.get(&s)) {
                Some(&i) => storeys[i].1.spaces.push(space),
                None => unplaced_spaces.push(space),
            }
        }
        let mut unplaced = ElementCounts::default();
        for (kinds, slot) in [
            (
                &["IFCWALL", "IFCWALLSTANDARDCASE", "IFCWALLELEMENTEDCASE"][..],
                0,
            ),
            (&["IFCDOOR", "IFCDOORSTANDARDCASE"][..], 1),
            (&["IFCWINDOW", "IFCWINDOWSTANDARDCASE"][..], 2),
        ] {
            for kind in kinds {
                for id in self.ids(kind) {
                    let counts = match container
                        .get(&id)
                        .and_then(|&c| storey_of(self, c))
                        .and_then(|s| index.get(&s))
                    {
                        Some(&i) => &mut storeys[i].1.elements,
                        None => &mut unplaced,
                    };
                    match slot {
                        0 => counts.walls += 1,
                        1 => counts.doors += 1,
                        _ => counts.windows += 1,
                    }
                }
            }
        }
        let mut storeys: Vec<IfcStorey> = storeys.into_iter().map(|(_, s)| s).collect();
        // By elevation; storeys without one keep their place at the end.
        storeys.sort_by(|a, b| match (a.elevation, b.elevation) {
            (Some(x), Some(y)) => x.total_cmp(&y),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        });
        if storeys.is_empty() && unplaced_spaces.is_empty() {
            return Err(format!(
                "`{path}` has no building storey and no space (IfcBuildingStorey, IfcSpace)"
            ));
        }
        if !unplaced_spaces.is_empty() {
            warnings.push(format!(
                "{} spaces are in no storey of the spatial structure",
                unplaced_spaces.len()
            ));
        }
        let project = self.ids("IFCPROJECT").first().copied();
        let (length_unit, area_unit) = match project {
            Some(p) => self.units(p),
            None => (None, None),
        };
        let site = self
            .ids("IFCSITE")
            .first()
            .copied()
            .and_then(|s| self.text(s, 2));
        let buildings = self
            .ids("IFCBUILDING")
            .into_iter()
            .filter_map(|b| self.text(b, 2))
            .collect();
        Ok(IfcScheduleSpec {
            title: None,
            path: path.to_owned(),
            schema,
            project: project.and_then(|p| self.text(p, 2).or_else(|| self.text(p, 7))),
            site,
            buildings,
            length_unit,
            area_unit,
            storeys,
            unplaced_spaces,
            unplaced,
            warnings,
        })
    }

    /// Each space's area as the file gives it, with its source, by the
    /// order in the module docs.
    fn space_areas(&mut self) -> HashMap<u32, (f64, String)> {
        let spaces: std::collections::HashSet<u32> = self.ids("IFCSPACE").into_iter().collect();
        let mut best: HashMap<u32, (u8, f64, String)> = HashMap::new();
        for rel in self.ids("IFCRELDEFINESBYPROPERTIES") {
            let Some(e) = self.decode(rel) else { continue };
            let (Some(objects), Some(definition)) = (e.get(4).map(refs), e.get_ref(5)) else {
                continue;
            };
            let objects: Vec<u32> = objects.into_iter().filter(|o| spaces.contains(o)).collect();
            if objects.is_empty() {
                continue;
            }
            for (rank, value, source) in self.areas_of(definition) {
                for &space in &objects {
                    let better = best.get(&space).is_none_or(|(r, _, _)| rank < *r);
                    if better {
                        best.insert(space, (rank, value, source.clone()));
                    }
                }
            }
        }
        best.into_iter()
            .map(|(space, (_, value, source))| (space, (value, source)))
            .collect()
    }

    /// The candidate areas of a property definition: rank (lower is
    /// better), value and source.
    fn areas_of(&mut self, definition: u32) -> Vec<(u8, f64, String)> {
        let Some(kind) = self.kind(definition).map(str::to_owned) else {
            return Vec::new();
        };
        let Some(e) = self.decode(definition) else {
            return Vec::new();
        };
        let set = e.get(2).and_then(text).unwrap_or_default();
        let mut out = Vec::new();
        match kind.as_str() {
            "IFCELEMENTQUANTITY" => {
                let base = matches!(set.as_str(), "Qto_SpaceBaseQuantities" | "BaseQuantities");
                for q in e.get(5).map(refs).unwrap_or_default() {
                    if self.kind(q) != Some("IFCQUANTITYAREA") {
                        continue;
                    }
                    let Some(qe) = self.decode(q) else { continue };
                    let (Some(name), Some(value)) = (
                        qe.get(0).and_then(text),
                        qe.get(3).and_then(AttributeValue::as_float),
                    ) else {
                        continue;
                    };
                    let rank = match (base, name.as_str()) {
                        (true, "NetFloorArea") => 0,
                        (true, "GrossFloorArea") => 1,
                        (_, n) if n.to_ascii_lowercase().contains("floorarea") => 4,
                        _ => continue,
                    };
                    out.push((rank, value, format!("{set}.{name}")));
                }
            }
            "IFCPROPERTYSET" if set == "Pset_SpaceCommon" => {
                for p in e.get(4).map(refs).unwrap_or_default() {
                    if self.kind(p) != Some("IFCPROPERTYSINGLEVALUE") {
                        continue;
                    }
                    let Some(pe) = self.decode(p) else { continue };
                    let (Some(name), Some(value)) = (
                        pe.get(0).and_then(text),
                        pe.get(2).and_then(AttributeValue::as_float),
                    ) else {
                        continue;
                    };
                    let rank = match name.as_str() {
                        "NetPlannedArea" => 2,
                        "GrossPlannedArea" => 3,
                        _ => continue,
                    };
                    out.push((rank, value, format!("{set}.{name}")));
                }
            }
            _ => {}
        }
        out
    }

    /// The project's length and area units.
    fn units(&mut self, project: u32) -> (Option<String>, Option<String>) {
        let Some(assignment) = self.decode(project).and_then(|p| p.get_ref(8)) else {
            return (None, None);
        };
        let units = self
            .decode(assignment)
            .and_then(|a| a.get(0).map(refs))
            .unwrap_or_default();
        let (mut length, mut area) = (None, None);
        for unit in units {
            let kind = self.kind(unit).map(str::to_owned);
            let Some(e) = self.decode(unit) else { continue };
            let unit_type = e
                .get(1)
                .and_then(AttributeValue::as_enum)
                .map(str::to_owned);
            let symbol = match kind.as_deref() {
                Some("IFCSIUNIT") => si_symbol(
                    e.get(2).and_then(AttributeValue::as_enum),
                    e.get(3).and_then(AttributeValue::as_enum),
                ),
                Some("IFCCONVERSIONBASEDUNIT") => {
                    e.get(2).and_then(text).map(|n| converted_symbol(&n))
                }
                _ => None,
            };
            match unit_type.as_deref() {
                Some("LENGTHUNIT") => length = length.or(symbol),
                Some("AREAUNIT") => area = area.or(symbol),
                _ => {}
            }
        }
        (length, area)
    }
}

/// Whether the reader looks entities of `kind` up by kind.
fn interesting(kind: &str) -> bool {
    matches!(
        kind,
        "IFCPROJECT"
            | "IFCSITE"
            | "IFCBUILDING"
            | "IFCBUILDINGSTOREY"
            | "IFCSPACE"
            | "IFCRELAGGREGATES"
            | "IFCRELCONTAINEDINSPATIALSTRUCTURE"
            | "IFCRELDEFINESBYPROPERTIES"
            | "IFCWALL"
            | "IFCWALLSTANDARDCASE"
            | "IFCWALLELEMENTEDCASE"
            | "IFCDOOR"
            | "IFCDOORSTANDARDCASE"
            | "IFCWINDOW"
            | "IFCWINDOWSTANDARDCASE"
    )
}

/// A non-empty string attribute, decoded from ISO 10303-21 escapes.
fn text(value: &AttributeValue) -> Option<String> {
    let raw = value.as_string()?;
    let decoded = decode_ifc_string(raw);
    let trimmed = decoded.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

/// The entity references of a list attribute.
fn refs(value: &AttributeValue) -> Vec<u32> {
    match value.as_list() {
        Some(items) => items
            .iter()
            .filter_map(AttributeValue::as_entity_ref)
            .collect(),
        None => value.as_entity_ref().into_iter().collect(),
    }
}

/// The symbol of an `IfcSIUnit` (`MILLI` + `METRE` → `mm`).
fn si_symbol(prefix: Option<&str>, name: Option<&str>) -> Option<String> {
    let base = match name? {
        "METRE" => "m",
        "SQUARE_METRE" => "m²",
        "CUBIC_METRE" => "m³",
        other => return Some(other.to_ascii_lowercase()),
    };
    let prefix = match prefix {
        None => "",
        Some("MILLI") => "m",
        Some("CENTI") => "c",
        Some("DECI") => "d",
        Some("KILO") => "k",
        Some("MICRO") => "µ",
        Some(other) => return Some(format!("{} {base}", other.to_ascii_lowercase())),
    };
    Some(format!("{prefix}{base}"))
}

/// The symbol of an `IfcConversionBasedUnit` by its name.
fn converted_symbol(name: &str) -> String {
    match name.to_ascii_uppercase().as_str() {
        "FOOT" | "FEET" => "ft".to_owned(),
        "INCH" => "in".to_owned(),
        "SQUARE FOOT" | "SQUARE_FOOT" | "SQUARE FEET" => "ft²".to_owned(),
        "SQUARE INCH" | "SQUARE_INCH" => "in²".to_owned(),
        _ => name.to_ascii_lowercase(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn units_have_short_symbols() {
        assert_eq!(
            si_symbol(Some("MILLI"), Some("METRE")).as_deref(),
            Some("mm")
        );
        assert_eq!(si_symbol(None, Some("SQUARE_METRE")).as_deref(), Some("m²"));
        assert_eq!(converted_symbol("square foot"), "ft²");
        assert_eq!(converted_symbol("FOOT"), "ft");
    }

    #[test]
    fn other_files_are_refused() {
        let err = parse(b"ISO-10303-21;\nHEADER;\nFILE_SCHEMA(('AUTOMOTIVE_DESIGN'));\nENDSEC;\nDATA;\nENDSEC;\nEND-ISO-10303-21;\n", "a.ifc").unwrap_err();
        assert!(err.contains("not an IFC file"), "{err}");
        let empty = parse(b"ISO-10303-21;\nHEADER;\nFILE_SCHEMA(('IFC4'));\nENDSEC;\nDATA;\n#1=IFCPROJECT('x',$,'P',$,$,$,$,$,$);\nENDSEC;\nEND-ISO-10303-21;\n", "a.ifc").unwrap_err();
        assert!(empty.contains("no building storey"), "{empty}");
    }
}
