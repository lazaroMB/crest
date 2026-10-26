//! Netlists as connectivity (`show_netlist`): which pin of which
//! component sits on which net, from a SPICE deck or a KiCad `.net`
//! export.
//!
//! The adapter copies the components (reference, value, model or
//! footprint) and their pin-to-net assignments; the nets, the nets with a
//! single pin (often a wire that goes nowhere), the component table and
//! the component–net graph are derived here, so what the user sees and
//! what the model is told come from the file, not from reading it by eye.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::invalid;
use crate::block::BlockError;
use crate::widget::graph::{
    FlowDirection, GraphEdge, GraphGroup, GraphNode, GraphSpec, LayoutKind,
};
use crate::widget::tree::{TreeColumn, TreeNode, TreeSpec};

/// Components a block keeps.
pub const MAX_COMPONENTS: usize = 5_000;
/// Pins a block keeps over all components.
pub const MAX_PINS: usize = 50_000;

const BLOCK: &str = "netlist";

/// Where the netlist came from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NetlistFormat {
    /// A SPICE deck (`.cir`, `.sp`, `.net` from a simulator).
    #[default]
    Spice,
    /// A KiCad netlist export (`.net`, s-expressions).
    Kicad,
}

impl NetlistFormat {
    /// Name for captions.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Spice => "SPICE",
            Self::Kicad => "KiCad",
        }
    }
}

/// One pin of a component and its net.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PinNet {
    /// Pin number or name (`1`, `C`, `d` for a MOSFET drain).
    pub pin: String,
    /// Net name.
    pub net: String,
    /// What the pin does, when the file says (`VCC`, `~`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub function: Option<String>,
}

/// One component (a SPICE element or a KiCad symbol instance).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Component {
    /// Reference designator (`R1`, `Q3`, `XU1`).
    pub reference: String,
    /// What it is (`resistor`, `capacitor`, `mosfet`, `subcircuit`, or a
    /// KiCad library part such as `Device:R`).
    pub kind: String,
    /// Its value (`10k`, `100n`, `DC 5`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// SPICE model or subcircuit name; KiCad footprint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The pins in order.
    #[serde(default)]
    pub pins: Vec<PinNet>,
}

/// A subcircuit defined in a SPICE deck.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Subcircuit {
    /// Its name.
    pub name: String,
    /// Its port nodes in order.
    pub ports: Vec<String>,
    /// Elements inside it.
    pub elements: usize,
}

/// A net with the pins on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Net {
    /// Its name.
    pub name: String,
    /// `(component index, pin)` of each pin on it.
    pub pins: Vec<(usize, String)>,
}

/// Whether `name` is a ground net (`0`, `GND`, `VSS`, `AGND`...).
#[must_use]
pub fn is_ground(name: &str) -> bool {
    let upper = name.trim_start_matches('/').to_ascii_uppercase();
    upper == "0" || upper.ends_with("GND") || upper == "VSS" || upper == "GROUND"
}

/// Whether `name` is a KiCad placeholder for a pin left open.
#[must_use]
pub fn is_unconnected(name: &str) -> bool {
    name.starts_with("unconnected-") || name.starts_with("Net-(unconnected")
}

/// A netlist block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct NetlistSpec {
    /// Title shown above; defaults to the file name or the design.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Workspace path, when read from a file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// The source format.
    pub format: NetlistFormat,
    /// The SPICE title line or the KiCad design source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub design: Option<String>,
    /// The top-level components.
    pub components: Vec<Component>,
    /// Subcircuits defined in the deck (not flattened).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subcircuits: Vec<Subcircuit>,
    /// Lines or forms the reader skipped, and why.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

impl NetlistSpec {
    /// Checks sizes and that references are set and distinct.
    ///
    /// # Errors
    ///
    /// [`BlockError::Invalid`] naming the first problem.
    pub fn validate(&self) -> Result<(), BlockError> {
        if self.components.len() > MAX_COMPONENTS {
            return Err(invalid(
                BLOCK,
                format!(
                    "{} components; at most {MAX_COMPONENTS}",
                    self.components.len()
                ),
            ));
        }
        if self.pin_count() > MAX_PINS {
            return Err(invalid(BLOCK, format!("more than {MAX_PINS} pins")));
        }
        let mut seen = std::collections::HashSet::new();
        for c in &self.components {
            if c.reference.trim().is_empty() {
                return Err(invalid(BLOCK, "a component has no reference"));
            }
            if !seen.insert(c.reference.as_str()) {
                return Err(invalid(
                    BLOCK,
                    format!("reference {} appears twice", c.reference),
                ));
            }
        }
        Ok(())
    }

    /// Pins over all components.
    #[must_use]
    pub fn pin_count(&self) -> usize {
        self.components.iter().map(|c| c.pins.len()).sum()
    }

    /// The nets, ground first, then by name.
    #[must_use]
    pub fn nets(&self) -> Vec<Net> {
        let mut by_name: BTreeMap<&str, Vec<(usize, String)>> = BTreeMap::new();
        for (ix, c) in self.components.iter().enumerate() {
            for p in &c.pins {
                by_name
                    .entry(p.net.as_str())
                    .or_default()
                    .push((ix, p.pin.clone()));
            }
        }
        let mut nets: Vec<Net> = by_name
            .into_iter()
            .map(|(name, pins)| Net {
                name: name.to_owned(),
                pins,
            })
            .collect();
        nets.sort_by_key(|n| !is_ground(&n.name));
        nets
    }

    /// Nets with one pin only: a wire that goes nowhere, or a pin a
    /// KiCad export marks unconnected.
    #[must_use]
    pub fn single_pin_nets(&self) -> Vec<Net> {
        self.nets()
            .into_iter()
            .filter(|n| n.pins.len() == 1 || is_unconnected(&n.name))
            .collect()
    }

    /// Component count per kind, most common first.
    #[must_use]
    pub fn kind_counts(&self) -> Vec<(String, usize)> {
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        for c in &self.components {
            *counts.entry(c.kind.as_str()).or_default() += 1;
        }
        let mut out: Vec<(String, usize)> =
            counts.into_iter().map(|(k, n)| (k.to_owned(), n)).collect();
        out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        out
    }

    /// The component–net graph: a box per component and per net (nets
    /// tagged `net`, `ground` or `unconnected`), an edge per pin labelled
    /// with the pin.
    #[must_use]
    pub fn to_graph(&self) -> GraphSpec {
        let mut nodes: Vec<GraphNode> = self
            .components
            .iter()
            .map(|c| {
                let mut attrs = Map::new();
                attrs.insert("kind".into(), json!(c.kind));
                if let Some(v) = &c.value {
                    attrs.insert("value".into(), json!(v));
                }
                if let Some(m) = &c.model {
                    attrs.insert("model".into(), json!(m));
                }
                let label = match &c.value {
                    Some(v) => format!("{} {v}", c.reference),
                    None => c.reference.clone(),
                };
                GraphNode {
                    id: format!("c:{}", c.reference),
                    label: Some(label),
                    group: None,
                    kind: Some(c.kind.clone()),
                    attrs,
                }
            })
            .collect();
        let mut edges = Vec::new();
        for net in self.nets() {
            let kind = if is_ground(&net.name) {
                "ground"
            } else if net.pins.len() == 1 || is_unconnected(&net.name) {
                "unconnected"
            } else {
                "net"
            };
            let mut attrs = Map::new();
            attrs.insert("pins".into(), json!(net.pins.len()));
            nodes.push(GraphNode {
                id: format!("n:{}", net.name),
                label: Some(net.name.clone()),
                group: None,
                kind: Some(kind.into()),
                attrs,
            });
            for (ix, pin) in &net.pins {
                edges.push(GraphEdge {
                    from: format!("c:{}", self.components[*ix].reference),
                    to: format!("n:{}", net.name),
                    label: Some(pin.clone()),
                    weight: None,
                });
            }
        }
        GraphSpec {
            title: None,
            directed: false,
            layout: LayoutKind::Force,
            direction: FlowDirection::Down,
            seed: Some(1),
            nodes,
            edges,
            groups: Vec::<GraphGroup>::new(),
        }
    }

    /// The component table: one row per component (kind, value, model)
    /// with a row per pin and its net under it.
    #[must_use]
    pub fn to_tree(&self) -> TreeSpec {
        let col = |name: &str| TreeColumn {
            name: name.into(),
            kind: crate::widget::tree::ColumnKind::Text,
            unit: None,
        };
        let single: std::collections::HashSet<String> =
            self.single_pin_nets().into_iter().map(|n| n.name).collect();
        let nodes = self
            .components
            .iter()
            .map(|c| {
                let mut values = Map::new();
                values.insert("kind".into(), json!(c.kind));
                values.insert(
                    "value".into(),
                    c.value.clone().map_or(Value::Null, Value::from),
                );
                values.insert(
                    "model / footprint".into(),
                    c.model.clone().map_or(Value::Null, Value::from),
                );
                let open = c.pins.iter().any(|p| single.contains(&p.net));
                TreeNode {
                    label: c.reference.clone(),
                    badge: open.then(|| "open pin".to_owned()),
                    tone: open.then_some(crate::widget::tree::BadgeTone::Warning),
                    values,
                    children: c
                        .pins
                        .iter()
                        .map(|p| {
                            let mut values = Map::new();
                            values.insert("kind".into(), json!("pin"));
                            values.insert("value".into(), json!(p.net));
                            if let Some(f) = &p.function {
                                values.insert("model / footprint".into(), json!(f));
                            }
                            let lone = single.contains(&p.net);
                            TreeNode {
                                label: format!("pin {}", p.pin),
                                badge: lone.then(|| "single-pin net".to_owned()),
                                tone: lone.then_some(crate::widget::tree::BadgeTone::Warning),
                                values,
                                children: Vec::new(),
                            }
                        })
                        .collect(),
                }
            })
            .collect();
        TreeSpec {
            title: Some("Components".into()),
            columns: vec![col("kind"), col("value"), col("model / footprint")],
            nodes,
            expand_depth: 1,
        }
    }

    /// The nets as CSV: `net,component,pin`.
    #[must_use]
    pub fn nets_csv(&self) -> String {
        let mut out = String::from("net,component,pin\n");
        for net in self.nets() {
            for (ix, pin) in &net.pins {
                out.push_str(&format!(
                    "{},{},{}\n",
                    csv(&net.name),
                    csv(&self.components[*ix].reference),
                    csv(pin)
                ));
            }
        }
        out
    }

    /// The components as CSV: `reference,kind,value,model,nets`.
    #[must_use]
    pub fn components_csv(&self) -> String {
        let mut out = String::from("reference,kind,value,model,nets\n");
        for c in &self.components {
            let nets: Vec<String> = c
                .pins
                .iter()
                .map(|p| format!("{}={}", p.pin, p.net))
                .collect();
            out.push_str(&format!(
                "{},{},{},{},{}\n",
                csv(&c.reference),
                csv(&c.kind),
                csv(c.value.as_deref().unwrap_or("")),
                csv(c.model.as_deref().unwrap_or("")),
                csv(&nets.join(" "))
            ));
        }
        out
    }
}

fn csv(field: &str) -> String {
    if field.contains([',', '"', '\n']) {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn part(reference: &str, kind: &str, nets: &[&str]) -> Component {
        Component {
            reference: reference.into(),
            kind: kind.into(),
            value: Some("1k".into()),
            model: None,
            pins: nets
                .iter()
                .enumerate()
                .map(|(i, n)| PinNet {
                    pin: (i + 1).to_string(),
                    net: (*n).into(),
                    function: None,
                })
                .collect(),
        }
    }

    fn divider() -> NetlistSpec {
        NetlistSpec {
            title: None,
            path: Some("div.cir".into()),
            format: NetlistFormat::Spice,
            design: None,
            components: vec![
                part("V1", "voltage source", &["in", "0"]),
                part("R1", "resistor", &["in", "out"]),
                part("R2", "resistor", &["out", "0"]),
                part("R3", "resistor", &["out", "dangling"]),
            ],
            subcircuits: Vec::new(),
            warnings: Vec::new(),
        }
    }

    #[test]
    fn nets_put_ground_first_and_flag_single_pins() {
        let spec = divider();
        spec.validate().unwrap_or_else(|e| panic!("{e}"));
        let nets = spec.nets();
        let names: Vec<&str> = nets.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(names, ["0", "dangling", "in", "out"]);
        assert_eq!(nets[3].pins.len(), 3);
        let single: Vec<String> = spec.single_pin_nets().into_iter().map(|n| n.name).collect();
        assert_eq!(single, ["dangling"]);
        assert_eq!(
            spec.kind_counts(),
            [("resistor".to_owned(), 3), ("voltage source".to_owned(), 1)]
        );
        assert!(is_ground("GND") && is_ground("/AGND") && !is_ground("VCC"));
    }

    #[test]
    fn graph_and_tree_cover_every_pin() {
        let spec = divider();
        let graph = spec.to_graph();
        assert_eq!(graph.nodes.len(), 4 + 4);
        assert_eq!(graph.edges.len(), spec.pin_count());
        assert!(graph.validate().is_ok());
        let dangling = graph.nodes.iter().find(|n| n.id == "n:dangling");
        assert_eq!(
            dangling.and_then(|n| n.kind.as_deref()),
            Some("unconnected")
        );
        let tree = spec.to_tree();
        assert!(tree.validate().is_ok());
        assert_eq!(tree.nodes[3].badge.as_deref(), Some("open pin"));
        assert!(
            spec.nets_csv()
                .starts_with("net,component,pin\n0,V1,2\n0,R2,2\n")
        );
        let mut twice = divider();
        twice.components[1].reference = "V1".into();
        assert!(twice.validate().is_err());
    }
}
