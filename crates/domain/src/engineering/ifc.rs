//! A room schedule read from an IFC model (`show_ifc_schedule`): the
//! building storeys, the spaces on each with their names and floor areas,
//! and how many walls, doors and windows each storey contains.
//!
//! Areas are only ever the file's own: an `IfcQuantityArea` of the
//! space's element quantities (`Qto_SpaceBaseQuantities` first) or an area
//! property of its property sets, each with where it was found. A space
//! whose file gives no area shows [`NOT_GIVEN`]; nothing is computed from
//! geometry. The full 3D model is not read.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::cad::csv_field;
use super::invalid;
use crate::block::BlockError;
use crate::widget::tree::{BadgeTone, ColumnKind, TreeColumn, TreeNode, TreeSpec};

/// What a space without an area in the file shows.
pub const NOT_GIVEN: &str = "not given";
/// Most storeys a schedule lists.
pub const MAX_STOREYS: usize = 1000;
/// Most spaces a schedule lists.
pub const MAX_SPACES: usize = 50_000;

/// Building elements counted per storey.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ElementCounts {
    /// `IfcWall` and its standard/elemented cases.
    #[serde(default)]
    pub walls: usize,
    /// `IfcDoor` (and `IfcDoorStandardCase`).
    #[serde(default)]
    pub doors: usize,
    /// `IfcWindow` (and `IfcWindowStandardCase`).
    #[serde(default)]
    pub windows: usize,
}

impl ElementCounts {
    /// Whether nothing was counted.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.walls == 0 && self.doors == 0 && self.windows == 0
    }

    /// The sum with `other`.
    #[must_use]
    pub fn plus(self, other: Self) -> Self {
        Self {
            walls: self.walls + other.walls,
            doors: self.doors + other.doors,
            windows: self.windows + other.windows,
        }
    }
}

/// A space (room) of a storey.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct IfcSpace {
    /// Its `Name` (often the room number).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Its `LongName` (often the room's use).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub long_name: Option<String>,
    /// Floor area in [`IfcScheduleSpec::area_unit`], when the file gives
    /// one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub area: Option<f64>,
    /// Where the area was read (`Qto_SpaceBaseQuantities.NetFloorArea`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub area_source: Option<String>,
    /// Its `GlobalId`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub global_id: Option<String>,
}

impl IfcSpace {
    /// The name, the long name, or `space`.
    #[must_use]
    pub fn label(&self) -> String {
        match (&self.name, &self.long_name) {
            (Some(n), Some(l)) if n != l => format!("{n} {l}"),
            (Some(n), _) => n.clone(),
            (None, Some(l)) => l.clone(),
            (None, None) => "space".to_owned(),
        }
    }
}

/// A building storey.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct IfcStorey {
    /// Its name.
    pub name: String,
    /// Elevation in [`IfcScheduleSpec::length_unit`], when given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elevation: Option<f64>,
    /// The building it belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub building: Option<String>,
    /// Its spaces, in file order.
    #[serde(default)]
    pub spaces: Vec<IfcSpace>,
    /// Walls, doors and windows contained in it (or in its spaces).
    #[serde(default)]
    pub elements: ElementCounts,
}

impl IfcStorey {
    /// The sum of the areas the file gives, and how many spaces have
    /// none.
    #[must_use]
    pub fn area(&self) -> (f64, usize) {
        let given: f64 = self.spaces.iter().filter_map(|s| s.area).sum();
        let missing = self.spaces.iter().filter(|s| s.area.is_none()).count();
        (given, missing)
    }
}

/// The schedule of an IFC file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct IfcScheduleSpec {
    /// Title above the schedule; defaults to the file name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The file, relative to the workspace.
    pub path: String,
    /// The schema of the header (`IFC2X3`, `IFC4`, `IFC4X3`).
    pub schema: String,
    /// The project's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// The site's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub site: Option<String>,
    /// Building names.
    #[serde(default)]
    pub buildings: Vec<String>,
    /// The project's length unit (`m`, `mm`, `ft`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length_unit: Option<String>,
    /// The project's area unit (`m²`, `ft²`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub area_unit: Option<String>,
    /// The storeys, by elevation (file order when not given).
    pub storeys: Vec<IfcStorey>,
    /// Spaces in no storey.
    #[serde(default)]
    pub unplaced_spaces: Vec<IfcSpace>,
    /// Walls, doors and windows in no storey.
    #[serde(default)]
    pub unplaced: ElementCounts,
    /// Reader notes.
    #[serde(default)]
    pub warnings: Vec<String>,
}

impl IfcScheduleSpec {
    /// The title, or the file name.
    #[must_use]
    pub fn display_title(&self) -> String {
        self.title
            .clone()
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| {
                self.path
                    .rsplit(['/', '\\'])
                    .next()
                    .unwrap_or(&self.path)
                    .to_owned()
            })
    }

    /// Every space, storey by storey, then the unplaced ones.
    pub fn spaces(&self) -> impl Iterator<Item = (Option<&IfcStorey>, &IfcSpace)> {
        self.storeys
            .iter()
            .flat_map(|st| st.spaces.iter().map(move |s| (Some(st), s)))
            .chain(self.unplaced_spaces.iter().map(|s| (None, s)))
    }

    /// Spaces counted.
    #[must_use]
    pub fn space_count(&self) -> usize {
        self.spaces().count()
    }

    /// Spaces whose file gives no area.
    #[must_use]
    pub fn missing_areas(&self) -> usize {
        self.spaces().filter(|(_, s)| s.area.is_none()).count()
    }

    /// Elements over every storey and the unplaced ones.
    #[must_use]
    pub fn element_totals(&self) -> ElementCounts {
        self.storeys
            .iter()
            .fold(self.unplaced, |acc, s| acc.plus(s.elements))
    }

    /// The area unit for readouts.
    #[must_use]
    pub fn area_label(&self) -> &str {
        self.area_unit.as_deref().unwrap_or("area units")
    }

    /// The schedule as a tree: storeys with their area total and element
    /// counts, their spaces under them with each area and its source.
    #[must_use]
    pub fn to_tree(&self) -> TreeSpec {
        let unit = self.area_unit.clone();
        let columns = vec![
            TreeColumn {
                name: "Name".into(),
                kind: ColumnKind::Text,
                unit: None,
            },
            TreeColumn {
                name: "Area".into(),
                kind: ColumnKind::Number,
                unit,
            },
            TreeColumn {
                name: "Source".into(),
                kind: ColumnKind::Text,
                unit: None,
            },
            TreeColumn {
                name: "Walls".into(),
                kind: ColumnKind::Number,
                unit: None,
            },
            TreeColumn {
                name: "Doors".into(),
                kind: ColumnKind::Number,
                unit: None,
            },
            TreeColumn {
                name: "Windows".into(),
                kind: ColumnKind::Number,
                unit: None,
            },
        ];
        let space_node = |s: &IfcSpace| {
            let mut values = Map::new();
            if let Some(l) = &s.long_name {
                values.insert("Name".into(), json!(l));
            }
            match s.area {
                Some(a) => {
                    values.insert("Area".into(), json!(a));
                    if let Some(src) = &s.area_source {
                        values.insert("Source".into(), json!(src));
                    }
                }
                None => {
                    values.insert("Source".into(), json!(NOT_GIVEN));
                }
            }
            TreeNode {
                label: s
                    .name
                    .clone()
                    .or_else(|| s.long_name.clone())
                    .unwrap_or_else(|| "space".into()),
                badge: s.area.is_none().then(|| "no area".to_owned()),
                tone: s.area.is_none().then_some(BadgeTone::Warning),
                values,
                children: Vec::new(),
            }
        };
        let counts = |values: &mut Map<String, Value>, c: &ElementCounts| {
            values.insert("Walls".into(), json!(c.walls));
            values.insert("Doors".into(), json!(c.doors));
            values.insert("Windows".into(), json!(c.windows));
        };
        let mut nodes: Vec<TreeNode> = self
            .storeys
            .iter()
            .map(|st| {
                let mut values = Map::new();
                let (given, missing) = st.area();
                if st.spaces.len() > missing {
                    values.insert("Area".into(), json!(given));
                    values.insert(
                        "Source".into(),
                        json!(if missing > 0 {
                            format!("sum of {} given", st.spaces.len() - missing)
                        } else {
                            "sum".to_owned()
                        }),
                    );
                }
                if let Some(e) = st.elevation {
                    values.insert("Name".into(), json!(format!("elevation {}", trim(e))));
                }
                counts(&mut values, &st.elements);
                TreeNode {
                    label: st.name.clone(),
                    badge: Some(format!("{} spaces", st.spaces.len())),
                    tone: Some(BadgeTone::Info),
                    values,
                    children: st.spaces.iter().map(space_node).collect(),
                }
            })
            .collect();
        if !self.unplaced_spaces.is_empty() || !self.unplaced.is_empty() {
            let mut values = Map::new();
            counts(&mut values, &self.unplaced);
            nodes.push(TreeNode {
                label: "(no storey)".into(),
                badge: None,
                tone: None,
                values,
                children: self.unplaced_spaces.iter().map(space_node).collect(),
            });
        }
        TreeSpec {
            title: Some(self.display_title()),
            columns,
            nodes,
            expand_depth: 2,
        }
    }

    /// The spaces as CSV: storey, name, long name, area, unit, source.
    #[must_use]
    pub fn to_csv(&self) -> String {
        let mut out = String::from("storey,space,long_name,area,unit,source\n");
        for (storey, s) in self.spaces() {
            let (area, unit, source) = match s.area {
                Some(a) => (
                    trim(a),
                    self.area_unit.clone().unwrap_or_default(),
                    s.area_source.clone().unwrap_or_default(),
                ),
                None => (String::new(), String::new(), NOT_GIVEN.to_owned()),
            };
            out.push_str(&format!(
                "{},{},{},{area},{},{}\n",
                csv_field(storey.map_or("", |st| st.name.as_str())),
                csv_field(s.name.as_deref().unwrap_or("")),
                csv_field(s.long_name.as_deref().unwrap_or("")),
                csv_field(&unit),
                csv_field(&source)
            ));
        }
        out
    }

    /// Checks the counts and that areas are finite and not negative.
    ///
    /// # Errors
    ///
    /// [`BlockError::Invalid`] naming the first problem.
    pub fn validate(&self) -> Result<(), BlockError> {
        const KIND: &str = "ifc_schedule";
        if self.path.trim().is_empty() {
            return Err(invalid(KIND, "the path is empty"));
        }
        if self.storeys.len() > MAX_STOREYS {
            return Err(invalid(KIND, format!("more than {MAX_STOREYS} storeys")));
        }
        if self.space_count() > MAX_SPACES {
            return Err(invalid(KIND, format!("more than {MAX_SPACES} spaces")));
        }
        if self.storeys.is_empty() && self.unplaced_spaces.is_empty() {
            return Err(invalid(KIND, "the model has no storey and no space"));
        }
        if let Some((_, s)) = self
            .spaces()
            .find(|(_, s)| s.area.is_some_and(|a| !a.is_finite() || a < 0.0))
        {
            return Err(invalid(
                KIND,
                format!("space `{}` has a bad area", s.label()),
            ));
        }
        if let Some(s) = self
            .spaces()
            .map(|(_, s)| s)
            .find(|s| s.area.is_some() && s.area_source.is_none())
        {
            return Err(invalid(
                KIND,
                format!("space `{}` has an area without a source", s.label()),
            ));
        }
        if let Some(st) = self
            .storeys
            .iter()
            .find(|st| st.elevation.is_some_and(|e| !e.is_finite()))
        {
            return Err(invalid(
                KIND,
                format!("storey `{}` has a bad elevation", st.name),
            ));
        }
        Ok(())
    }
}

/// `v` with at most 3 decimals, without trailing zeros.
fn trim(v: f64) -> String {
    let text = format!("{v:.3}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text.is_empty() || text == "-0" {
        "0".to_owned()
    } else {
        text.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn space(name: &str, long: &str, area: Option<f64>) -> IfcSpace {
        IfcSpace {
            name: Some(name.into()),
            long_name: Some(long.into()),
            area,
            area_source: area.map(|_| "Qto_SpaceBaseQuantities.NetFloorArea".into()),
            global_id: None,
        }
    }

    fn spec() -> IfcScheduleSpec {
        IfcScheduleSpec {
            title: None,
            path: "bim/house.ifc".into(),
            schema: "IFC4".into(),
            project: Some("House".into()),
            site: None,
            buildings: vec!["Main".into()],
            length_unit: Some("m".into()),
            area_unit: Some("m²".into()),
            storeys: vec![IfcStorey {
                name: "Ground floor".into(),
                elevation: Some(0.0),
                building: Some("Main".into()),
                spaces: vec![
                    space("101", "Kitchen, open", Some(12.5)),
                    space("102", "Store", None),
                ],
                elements: ElementCounts {
                    walls: 4,
                    doors: 2,
                    windows: 3,
                },
            }],
            unplaced_spaces: Vec::new(),
            unplaced: ElementCounts {
                walls: 1,
                doors: 0,
                windows: 0,
            },
            warnings: Vec::new(),
        }
    }

    #[test]
    fn a_schedule_totals_only_given_areas() {
        let spec = spec();
        assert_eq!(spec.validate(), Ok(()));
        assert_eq!(spec.storeys[0].area(), (12.5, 1));
        assert_eq!((spec.space_count(), spec.missing_areas()), (2, 1));
        assert_eq!(spec.element_totals().walls, 5);
        assert_eq!(
            spec.to_csv(),
            "storey,space,long_name,area,unit,source\n\
             Ground floor,101,\"Kitchen, open\",12.5,m²,Qto_SpaceBaseQuantities.NetFloorArea\n\
             Ground floor,102,Store,,,not given\n"
        );
        let tree = spec.to_tree();
        assert_eq!(tree.nodes.len(), 2, "a storey and the unplaced walls");
        let storey = &tree.nodes[0];
        assert_eq!(storey.values["Area"], json!(12.5));
        assert_eq!(storey.values["Source"], json!("sum of 1 given"));
        assert_eq!(storey.children[1].values["Source"], json!(NOT_GIVEN));
        assert_eq!(storey.children[1].tone, Some(BadgeTone::Warning));
    }

    #[test]
    fn areas_need_a_source_and_a_sane_value() {
        let mut unsourced = spec();
        unsourced.storeys[0].spaces[0].area_source = None;
        assert!(unsourced.validate().is_err());
        let mut negative = spec();
        negative.storeys[0].spaces[0].area = Some(-1.0);
        assert!(negative.validate().is_err());
        let mut empty = spec();
        empty.storeys.clear();
        assert!(empty.validate().is_err());
    }
}
