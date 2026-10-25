//! Diagrams from source files on the graph widget: Graphviz DOT
//! (`show_dot`) and entity-relationship diagrams from SQL DDL
//! (`show_er`).
//!
//! The adapter builds the [`GraphSpec`] the block draws: for DOT, the
//! nodes, edges and clusters (as groups) of the file, `rankdir` as the
//! flow and `color` as the node kind (nodes of one color share a palette
//! color); for DDL, one node per table (its columns in the node's
//! attributes) and one edge per foreign key, plus the [`ErTable`]s the
//! block lists beside the graph. Mermaid diagrams the model writes itself
//! stay with `show_mermaid`: these two draw what a file says.

use std::collections::BTreeSet;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Map;

use super::invalid;
use crate::block::BlockError;
use crate::widget::graph::GraphSpec;
use crate::widget::tree::{BadgeTone, ColumnKind, TreeColumn, TreeNode, TreeSpec};

/// Tables an ER diagram keeps.
pub const MAX_TABLES: usize = 2_000;
/// Warnings kept (skipped statements, unsupported DOT features).
pub const MAX_WARNINGS: usize = 50;

const BLOCK: &str = "diagram";

/// What the diagram was drawn from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DiagramSource {
    /// A Graphviz DOT file.
    Dot,
    /// SQL `CREATE TABLE` / `ALTER TABLE` statements.
    SqlDdl,
}

impl DiagramSource {
    /// Display name.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Dot => "DOT",
            Self::SqlDdl => "SQL DDL",
        }
    }
}

/// A column of a table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ErColumn {
    /// Column name.
    pub name: String,
    /// Its type as written (`varchar(80)`, `bigint`).
    pub data_type: String,
    /// Part of the primary key.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub primary_key: bool,
    /// May hold NULL.
    #[serde(default = "yes")]
    pub nullable: bool,
    /// Has a unique constraint of its own.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unique: bool,
    /// `table.column` it references.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub references: Option<String>,
}

fn yes() -> bool {
    true
}

/// A table and its columns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ErTable {
    /// Table name (schema-qualified when the DDL was).
    pub name: String,
    /// Columns in order.
    #[serde(default)]
    pub columns: Vec<ErColumn>,
}

impl ErTable {
    /// Names of the primary key columns.
    #[must_use]
    pub fn primary_key(&self) -> Vec<&str> {
        self.columns
            .iter()
            .filter(|c| c.primary_key)
            .map(|c| c.name.as_str())
            .collect()
    }
}

/// A diagram, its graph and (for DDL) its tables.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DiagramSpec {
    /// Caption; the graph's own name or the file name when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Workspace path of the file, when it came from one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// What it was drawn from.
    pub source: DiagramSource,
    /// The SQL dialect the DDL was read in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dialect: Option<String>,
    /// What the block draws.
    pub graph: GraphSpec,
    /// Tables of an ER diagram, in DDL order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tables: Vec<ErTable>,
    /// What was skipped (statements that did not parse, DOT features not
    /// drawn).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

/// In and out degree of a node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Degree {
    /// Node index.
    pub node: usize,
    /// Edges into it.
    pub incoming: usize,
    /// Edges out of it.
    pub outgoing: usize,
}

impl DiagramSpec {
    /// Checks the graph and the table count.
    ///
    /// # Errors
    ///
    /// [`BlockError::Invalid`] naming the first problem.
    pub fn validate(&self) -> Result<(), BlockError> {
        self.graph.validate()?;
        if self.tables.len() > MAX_TABLES {
            return Err(invalid(
                BLOCK,
                format!("{} tables; at most {MAX_TABLES}", self.tables.len()),
            ));
        }
        Ok(())
    }

    /// Degrees of every node, the most connected first.
    #[must_use]
    pub fn degrees(&self) -> Vec<Degree> {
        let mut out: Vec<Degree> = (0..self.graph.nodes.len())
            .map(|node| Degree {
                node,
                incoming: 0,
                outgoing: 0,
            })
            .collect();
        for (a, b) in self.graph.ends() {
            if let Some(d) = out.get_mut(a) {
                d.outgoing += 1;
            }
            if let Some(d) = out.get_mut(b) {
                d.incoming += 1;
            }
        }
        out.sort_by(|a, b| {
            (b.incoming + b.outgoing)
                .cmp(&(a.incoming + a.outgoing))
                .then(a.node.cmp(&b.node))
        });
        out
    }

    /// Whether the edges (taken as directed) form a cycle; self-loops
    /// count.
    #[must_use]
    pub fn has_cycle(&self) -> bool {
        let n = self.graph.nodes.len();
        let mut next = vec![Vec::new(); n];
        for (a, b) in self.graph.ends() {
            if a < n && b < n {
                next[a].push(b);
            }
        }
        // 0 = new, 1 = on the stack, 2 = done.
        let mut state = vec![0_u8; n];
        for start in 0..n {
            if state[start] != 0 {
                continue;
            }
            let mut stack = vec![(start, 0_usize)];
            state[start] = 1;
            while let Some((node, at)) = stack.pop() {
                if let Some(&child) = next[node].get(at) {
                    stack.push((node, at + 1));
                    match state[child] {
                        1 => return true,
                        0 => {
                            state[child] = 1;
                            stack.push((child, 0));
                        }
                        _ => {}
                    }
                } else {
                    state[node] = 2;
                }
            }
        }
        false
    }

    /// Tables no foreign key touches.
    #[must_use]
    pub fn isolated_tables(&self) -> Vec<&str> {
        let touched: BTreeSet<usize> = self
            .graph
            .ends()
            .into_iter()
            .flat_map(|(a, b)| [a, b])
            .collect();
        self.graph
            .nodes
            .iter()
            .enumerate()
            .filter(|(ix, _)| !touched.contains(ix))
            .map(|(_, n)| n.id.as_str())
            .collect()
    }

    /// The tables as a tree table: a row per table, its columns nested
    /// with type, `PK` / `FK` / `unique` badges and the referenced
    /// column.
    #[must_use]
    pub fn tables_tree(&self) -> TreeSpec {
        let column = |name: &str| TreeColumn {
            name: name.to_owned(),
            kind: ColumnKind::Text,
            unit: None,
        };
        let nodes = self
            .tables
            .iter()
            .map(|t| {
                let children = t
                    .columns
                    .iter()
                    .map(|c| {
                        let mut values = Map::new();
                        let mut ty = c.data_type.clone();
                        if !c.nullable {
                            ty.push_str(" not null");
                        }
                        values.insert("type".into(), ty.into());
                        if let Some(r) = &c.references {
                            values.insert("references".into(), r.clone().into());
                        }
                        let (badge, tone) = if c.primary_key {
                            (Some("PK"), Some(BadgeTone::Warning))
                        } else if c.references.is_some() {
                            (Some("FK"), Some(BadgeTone::Info))
                        } else if c.unique {
                            (Some("unique"), Some(BadgeTone::Neutral))
                        } else {
                            (None, None)
                        };
                        TreeNode {
                            label: c.name.clone(),
                            badge: badge.map(str::to_owned),
                            tone,
                            values,
                            children: Vec::new(),
                        }
                    })
                    .collect();
                TreeNode {
                    label: t.name.clone(),
                    badge: Some(format!("{} columns", t.columns.len())),
                    tone: Some(BadgeTone::Neutral),
                    values: Map::new(),
                    children,
                }
            })
            .collect();
        TreeSpec {
            title: Some("Tables".to_owned()),
            columns: vec![column("type"), column("references")],
            nodes,
            expand_depth: 1,
        }
    }

    /// The ER diagram as Mermaid `erDiagram` source.
    #[must_use]
    pub fn to_mermaid(&self) -> String {
        let ident = |s: &str| {
            s.chars()
                .map(|c| {
                    if c.is_alphanumeric() || c == '_' || c == '-' {
                        c
                    } else {
                        '_'
                    }
                })
                .collect::<String>()
        };
        let mut out = String::from("erDiagram\n");
        for t in &self.tables {
            out.push_str(&format!("    {} {{\n", ident(&t.name)));
            for c in &t.columns {
                let ty: String = ident(c.data_type.split('(').next().unwrap_or("x"));
                let mut keys = Vec::new();
                if c.primary_key {
                    keys.push("PK");
                }
                if c.references.is_some() {
                    keys.push("FK");
                }
                out.push_str(&format!(
                    "        {} {}{}\n",
                    if ty.is_empty() { "x".to_owned() } else { ty },
                    ident(&c.name),
                    if keys.is_empty() {
                        String::new()
                    } else {
                        format!(" {}", keys.join(","))
                    }
                ));
            }
            out.push_str("    }\n");
        }
        for edge in &self.graph.edges {
            out.push_str(&format!(
                "    {} }}o--|| {} : \"{}\"\n",
                ident(&edge.from),
                ident(&edge.to),
                edge.label.as_deref().unwrap_or("").replace('"', "'")
            ));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn er() -> DiagramSpec {
        let graph: GraphSpec = serde_json::from_value(json!({
            "nodes": [{"id": "users", "kind": "table"}, {"id": "orders", "kind": "table"},
                      {"id": "audit", "kind": "table"}],
            "edges": [{"from": "orders", "to": "users", "label": "user_id → id"}]
        }))
        .unwrap_or_else(|e| panic!("{e}"));
        let col = |name: &str, ty: &str, pk, references: Option<&str>| ErColumn {
            name: name.into(),
            data_type: ty.into(),
            primary_key: pk,
            nullable: !pk,
            unique: false,
            references: references.map(str::to_owned),
        };
        DiagramSpec {
            title: None,
            path: Some("db/schema.sql".into()),
            source: DiagramSource::SqlDdl,
            dialect: Some("postgres".into()),
            graph,
            tables: vec![
                ErTable {
                    name: "users".into(),
                    columns: vec![col("id", "bigint", true, None)],
                },
                ErTable {
                    name: "orders".into(),
                    columns: vec![
                        col("id", "bigint", true, None),
                        col("user_id", "bigint", false, Some("users.id")),
                    ],
                },
                ErTable {
                    name: "audit".into(),
                    columns: Vec::new(),
                },
            ],
            warnings: Vec::new(),
        }
    }

    #[test]
    fn degrees_cycles_and_isolated_tables() {
        let spec = er();
        spec.validate().unwrap();
        let top = spec.degrees()[0];
        assert_eq!((top.node, top.incoming, top.outgoing), (0, 1, 0));
        assert!(!spec.has_cycle());
        assert_eq!(spec.isolated_tables(), ["audit"]);
        let mut cyclic = er();
        cyclic.graph.edges.push(crate::widget::graph::GraphEdge {
            from: "users".into(),
            to: "orders".into(),
            label: None,
            weight: None,
        });
        assert!(cyclic.has_cycle());
    }

    #[test]
    fn tables_tree_and_mermaid() {
        let spec = er();
        let tree = spec.tables_tree();
        tree.validate().unwrap();
        assert_eq!(tree.nodes[1].children[1].badge.as_deref(), Some("FK"));
        assert_eq!(
            tree.nodes[1].children[0]
                .values
                .get("type")
                .and_then(|v| v.as_str()),
            Some("bigint not null")
        );
        let mermaid = spec.to_mermaid();
        assert!(
            mermaid.starts_with("erDiagram\n    users {\n        bigint id PK\n"),
            "{mermaid}"
        );
        assert!(mermaid.contains("        bigint user_id FK\n"), "{mermaid}");
        assert!(
            mermaid.contains("    orders }o--|| users : \"user_id → id\"\n"),
            "{mermaid}"
        );
    }
}
