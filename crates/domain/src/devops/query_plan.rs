//! A SQL query plan on the tree table (`show_query_plan`).
//!
//! The adapter reads PostgreSQL's `EXPLAIN (FORMAT JSON)` output, with
//! `ANALYZE` and `BUFFERS` when they were given, into one [`PlanNode`] per
//! plan node in preorder (each node followed by its children, one level
//! deeper). [`QueryPlanSpec::analyse`] then derives what a reader of the
//! plan looks for: the time each node spent itself (its time minus its
//! children's), the hot nodes, the row misestimates and the usual warnings
//! (sorts and hashes spilling to disk, sequential scans that filter most
//! rows away). [`QueryPlanSpec::to_tree`] lays it all out as a
//! [`TreeSpec`].
//!
//! Times are milliseconds. PostgreSQL reports `Actual Total Time` and
//! `Actual Rows` per loop; [`PlanNode::inclusive_time`] is over all loops,
//! divided by the processes of a parallel plan (the leader plus the
//! workers of the `Gather` above), so it stays comparable with the
//! execution time.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::invalid;
use crate::block::BlockError;
use crate::widget::tree::{BadgeTone, ColumnKind, TreeColumn, TreeNode, TreeSpec, format_number};

/// Plan nodes a block keeps.
pub const MAX_NODES: usize = 10_000;
/// Deepest nesting of a plan.
pub const MAX_DEPTH: u32 = 120;
/// A node is hot when its self time is at least this share of the total.
pub const HOT_SHARE: f64 = 0.1;
/// A row estimate off by at least this factor is flagged.
pub const MISESTIMATE_FACTOR: f64 = 10.0;
/// Rows a sequential scan must filter away to be flagged.
pub const SEQ_SCAN_REMOVED: f64 = 1000.0;

const BLOCK: &str = "query_plan";

/// The database that wrote the plan.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PlanEngine {
    /// PostgreSQL `EXPLAIN (FORMAT JSON)`.
    #[default]
    Postgres,
}

impl PlanEngine {
    /// Display name.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Postgres => "PostgreSQL",
        }
    }
}

/// Buffer counts of a node (`BUFFERS`), in blocks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PlanBuffers {
    /// Shared blocks found in the cache.
    #[serde(default)]
    pub shared_hit: u64,
    /// Shared blocks read from disk (or the OS cache).
    #[serde(default)]
    pub shared_read: u64,
    /// Shared blocks dirtied.
    #[serde(default)]
    pub shared_dirtied: u64,
    /// Shared blocks written.
    #[serde(default)]
    pub shared_written: u64,
    /// Local (temporary table) blocks hit or read.
    #[serde(default)]
    pub local: u64,
    /// Temporary file blocks read.
    #[serde(default)]
    pub temp_read: u64,
    /// Temporary file blocks written (a spill).
    #[serde(default)]
    pub temp_written: u64,
}

/// One node of the plan.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PlanNode {
    /// `Node Type`: `Seq Scan`, `Hash Join`, `Sort`...
    pub node_type: String,
    /// Nesting level, 0 for the root.
    #[serde(default)]
    pub depth: u32,
    /// Table, view, function or CTE scanned (`schema.name` when the schema
    /// was given).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relation: Option<String>,
    /// Alias of the relation, when it differs from its name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    /// Index used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<String>,
    /// `Inner`, `Left`, `Semi`... of a join.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub join_type: Option<String>,
    /// How the node feeds its parent: `Outer`, `Inner`, `InitPlan`,
    /// `SubPlan`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_relationship: Option<String>,
    /// Name of an `InitPlan` / `SubPlan`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subplan_name: Option<String>,
    /// The main condition or key (index, hash, merge or join condition,
    /// filter, sort or group key).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
    /// Planner cost to the first row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub startup_cost: Option<f64>,
    /// Planner cost to the last row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_cost: Option<f64>,
    /// Rows the planner expected, per loop.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_rows: Option<f64>,
    /// Time to the last row, per loop, in ms (`ANALYZE`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual_time: Option<f64>,
    /// Rows produced, per loop (`ANALYZE`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual_rows: Option<f64>,
    /// Times the node ran (`ANALYZE`); 0 means it never ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loops: Option<f64>,
    /// Rows removed by the filter, join filter and index recheck, per
    /// loop.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rows_removed: Option<f64>,
    /// Workers launched (`Gather`, `Gather Merge`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workers: Option<u32>,
    /// Buffer counts (`BUFFERS`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub buffers: Option<PlanBuffers>,
    /// `Sort Space Type` (`Memory`, `Disk`) of a sort.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort_space_type: Option<String>,
    /// `Sort Space Used`, in kB.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort_space_kb: Option<f64>,
    /// `Hash Batches` of a hash; more than one means it spilled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash_batches: Option<u64>,
    /// Time over all loops (per process of a parallel plan), in ms; set
    /// by [`QueryPlanSpec::analyse`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inclusive_time: Option<f64>,
    /// [`Self::inclusive_time`] minus the children's, in ms; set by
    /// [`QueryPlanSpec::analyse`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub self_time: Option<f64>,
    /// What looks wrong with the node; set by [`QueryPlanSpec::analyse`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    /// The node's other scalar properties, as written.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    #[schemars(with = "std::collections::BTreeMap<String, Value>")]
    pub attrs: Map<String, Value>,
}

impl PlanNode {
    /// `Seq Scan on orders o`, `Index Scan using orders_pkey on orders`,
    /// `Hash Join (Left)`, `InitPlan 1: Result`.
    #[must_use]
    pub fn label(&self) -> String {
        let mut label = self.node_type.clone();
        if let Some(join) = &self.join_type
            && !self.node_type.contains(join.as_str())
            && join != "Inner"
        {
            label = format!("{label} ({join})");
        }
        if let Some(index) = &self.index {
            label.push_str(" using ");
            label.push_str(index);
        }
        if let Some(relation) = &self.relation {
            label.push_str(" on ");
            label.push_str(relation);
            if let Some(alias) = &self.alias {
                label.push(' ');
                label.push_str(alias);
            }
        }
        if let Some(name) = &self.subplan_name {
            label = format!("{name}: {label}");
        }
        label
    }

    /// Whether `ANALYZE` saw the node never run.
    #[must_use]
    pub fn never_executed(&self) -> bool {
        self.loops == Some(0.0)
    }

    /// Rows produced over all loops.
    #[must_use]
    pub fn total_rows(&self) -> Option<f64> {
        Some(self.actual_rows? * self.loops.unwrap_or(1.0).max(1.0))
    }

    /// Rows the planner expected over the loops that ran.
    #[must_use]
    pub fn estimated_rows(&self) -> Option<f64> {
        let loops = self.loops.unwrap_or(1.0).max(1.0);
        Some(self.plan_rows? * loops)
    }

    /// How far the row estimate was off: the factor (at least 1) and
    /// whether the planner expected fewer rows than came. `None` without
    /// `ANALYZE` or for a node that never ran.
    #[must_use]
    pub fn misestimate(&self) -> Option<(f64, bool)> {
        if self.never_executed() {
            return None;
        }
        let actual = self.actual_rows?.max(1.0);
        let planned = self.plan_rows?.max(1.0);
        if actual >= planned {
            Some((actual / planned, true))
        } else {
            Some((planned / actual, false))
        }
    }
}

/// A plan, its timings and its nodes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct QueryPlanSpec {
    /// Caption; the file name when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Workspace path of the file, when the plan came from one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Who wrote the plan.
    #[serde(default)]
    pub engine: PlanEngine,
    /// The plan has actual times and rows (`ANALYZE`).
    #[serde(default)]
    pub analyzed: bool,
    /// The plan has buffer counts (`BUFFERS`).
    #[serde(default)]
    pub buffers: bool,
    /// `Planning Time`, in ms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub planning_time: Option<f64>,
    /// `Execution Time`, in ms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_time: Option<f64>,
    /// Time in triggers, in ms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger_time: Option<f64>,
    /// JIT compilation time, in ms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jit_time: Option<f64>,
    /// Nodes in preorder, the root first.
    pub nodes: Vec<PlanNode>,
}

/// A row estimate that was off.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Misestimate {
    /// Node index.
    pub node: usize,
    /// Factor, at least 1.
    pub factor: f64,
    /// More rows came than the planner expected.
    pub under: bool,
}

impl QueryPlanSpec {
    /// Checks the size, that depths form a tree from one root and that
    /// numbers are finite and not negative.
    ///
    /// # Errors
    ///
    /// [`BlockError::Invalid`] naming the first problem.
    pub fn validate(&self) -> Result<(), BlockError> {
        if self.nodes.is_empty() {
            return Err(invalid(BLOCK, "the plan has no nodes"));
        }
        if self.nodes.len() > MAX_NODES {
            return Err(invalid(
                BLOCK,
                format!("{} nodes; at most {MAX_NODES}", self.nodes.len()),
            ));
        }
        let mut previous: Option<u32> = None;
        for (ix, node) in self.nodes.iter().enumerate() {
            let ok = match previous {
                None => node.depth == 0,
                Some(p) => node.depth >= 1 && node.depth <= p + 1,
            };
            if !ok || node.depth > MAX_DEPTH {
                return Err(invalid(
                    BLOCK,
                    format!("node {ix} ({}) has depth {}", node.node_type, node.depth),
                ));
            }
            previous = Some(node.depth);
            let numbers = [
                node.startup_cost,
                node.total_cost,
                node.plan_rows,
                node.actual_time,
                node.actual_rows,
                node.loops,
                node.rows_removed,
                node.sort_space_kb,
                node.inclusive_time,
                node.self_time,
            ];
            if numbers.iter().flatten().any(|n| !n.is_finite() || *n < 0.0) {
                return Err(invalid(
                    BLOCK,
                    format!(
                        "node {ix} ({}) has a negative or non-finite number",
                        node.node_type
                    ),
                ));
            }
        }
        let times = [
            self.planning_time,
            self.execution_time,
            self.trigger_time,
            self.jit_time,
        ];
        if times.iter().flatten().any(|n| !n.is_finite() || *n < 0.0) {
            return Err(invalid(BLOCK, "a plan time is negative or not finite"));
        }
        Ok(())
    }

    /// Indices of the direct children of node `ix`.
    #[must_use]
    pub fn children(&self, ix: usize) -> Vec<usize> {
        let Some(node) = self.nodes.get(ix) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for (child, n) in self.nodes.iter().enumerate().skip(ix + 1) {
            if n.depth <= node.depth {
                break;
            }
            if n.depth == node.depth + 1 {
                out.push(child);
            }
        }
        out
    }

    /// The parent of every node (`None` for the root).
    #[must_use]
    pub fn parents(&self) -> Vec<Option<usize>> {
        let mut stack: Vec<usize> = Vec::new();
        let mut out = Vec::with_capacity(self.nodes.len());
        for (ix, node) in self.nodes.iter().enumerate() {
            while stack
                .last()
                .is_some_and(|&top| self.nodes[top].depth >= node.depth)
            {
                stack.pop();
            }
            out.push(stack.last().copied());
            stack.push(ix);
        }
        out
    }

    /// The time the shares are of: the execution time, else the root's
    /// inclusive time.
    #[must_use]
    pub fn total_time(&self) -> Option<f64> {
        self.execution_time
            .or_else(|| self.nodes.first().and_then(|n| n.inclusive_time))
            .filter(|t| *t > 0.0)
    }

    /// Node `ix`'s self time as a share of [`Self::total_time`].
    #[must_use]
    pub fn self_share(&self, ix: usize) -> Option<f64> {
        let total = self.total_time()?;
        Some(self.nodes.get(ix)?.self_time? / total)
    }

    /// Whether node `ix` spent at least [`HOT_SHARE`] of the time itself.
    #[must_use]
    pub fn is_hot(&self, ix: usize) -> bool {
        self.self_share(ix).is_some_and(|s| s >= HOT_SHARE)
    }

    /// Fills the inclusive and self times and the warnings of every node.
    pub fn analyse(&mut self) {
        let parents = self.parents();
        // Processes sharing a node: the leader plus the workers of the
        // nearest Gather above it.
        let mut processes = vec![1.0_f64; self.nodes.len()];
        for ix in 0..self.nodes.len() {
            if let Some(parent) = parents[ix] {
                let p = &self.nodes[parent];
                processes[ix] = match (p.node_type.starts_with("Gather"), p.workers) {
                    (true, Some(w)) => f64::from(w) + 1.0,
                    _ => processes[parent],
                };
            }
        }
        for (ix, node) in self.nodes.iter_mut().enumerate() {
            node.inclusive_time = node.actual_time.map(|t| {
                let loops = node.loops.unwrap_or(1.0);
                t * loops / processes[ix]
            });
        }
        for ix in 0..self.nodes.len() {
            let Some(inclusive) = self.nodes[ix].inclusive_time else {
                continue;
            };
            let children: f64 = self
                .children(ix)
                .iter()
                .filter_map(|&c| self.nodes[c].inclusive_time)
                .sum();
            self.nodes[ix].self_time = Some((inclusive - children).max(0.0));
        }
        for node in &mut self.nodes {
            node.warnings = warnings(node);
        }
    }

    /// The `n` nodes with the most self time, most first.
    #[must_use]
    pub fn slowest(&self, n: usize) -> Vec<usize> {
        let mut ixs: Vec<usize> = (0..self.nodes.len())
            .filter(|&ix| self.nodes[ix].self_time.is_some_and(|t| t > 0.0))
            .collect();
        ixs.sort_by(|&a, &b| {
            let t = |ix: usize| self.nodes[ix].self_time.unwrap_or(0.0);
            t(b).total_cmp(&t(a)).then(a.cmp(&b))
        });
        ixs.truncate(n);
        ixs
    }

    /// The `n` worst row estimates off by at least `min_factor`, worst
    /// first.
    #[must_use]
    pub fn misestimates(&self, min_factor: f64, n: usize) -> Vec<Misestimate> {
        let mut out: Vec<Misestimate> = self
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(node, n)| {
                let (factor, under) = n.misestimate()?;
                (factor >= min_factor).then_some(Misestimate {
                    node,
                    factor,
                    under,
                })
            })
            .collect();
        out.sort_by(|a, b| b.factor.total_cmp(&a.factor).then(a.node.cmp(&b.node)));
        out.truncate(n);
        out
    }

    /// The plan as a tree table: time, self time (bar), share, rows
    /// (bar), estimate, misestimate, loops, buffers, cost and a note;
    /// time columns only with `ANALYZE`, buffer columns only with
    /// `BUFFERS`. Hot nodes carry a `hot` badge, then misestimates and
    /// warnings.
    #[must_use]
    pub fn to_tree(&self) -> TreeSpec {
        let column = |name: &str, kind, unit: Option<&str>| TreeColumn {
            name: name.to_owned(),
            kind,
            unit: unit.map(str::to_owned),
        };
        let mut columns = Vec::new();
        if self.analyzed {
            columns.push(column("time", ColumnKind::Duration, Some("ms")));
            columns.push(column("self", ColumnKind::Bar, Some("ms")));
            columns.push(column("self %", ColumnKind::Number, Some("%")));
            columns.push(column("rows", ColumnKind::Bar, None));
            columns.push(column("est. rows", ColumnKind::Number, None));
            columns.push(column("misestimate", ColumnKind::Text, None));
            columns.push(column("loops", ColumnKind::Number, None));
        } else {
            columns.push(column("est. rows", ColumnKind::Bar, None));
        }
        if self.buffers {
            columns.push(column("hit", ColumnKind::Number, Some("blocks")));
            columns.push(column("read", ColumnKind::Number, Some("blocks")));
            if self
                .nodes
                .iter()
                .any(|n| n.buffers.is_some_and(|b| b.temp_read + b.temp_written > 0))
            {
                columns.push(column("temp", ColumnKind::Number, Some("blocks")));
            }
        }
        columns.push(column("cost", ColumnKind::Number, None));
        columns.push(column("note", ColumnKind::Text, None));

        let rows: Vec<TreeNode> = (0..self.nodes.len()).map(|ix| self.tree_node(ix)).collect();
        // Nest the preorder rows by depth.
        let mut stack: Vec<(u32, TreeNode)> = Vec::new();
        let mut roots: Vec<TreeNode> = Vec::new();
        for (ix, row) in rows.into_iter().enumerate() {
            let depth = self.nodes[ix].depth;
            while stack.last().is_some_and(|(d, _)| *d >= depth) {
                pop_into(&mut stack, &mut roots);
            }
            stack.push((depth, row));
        }
        while !stack.is_empty() {
            pop_into(&mut stack, &mut roots);
        }
        TreeSpec {
            title: None,
            columns,
            nodes: roots,
            expand_depth: MAX_DEPTH,
        }
    }

    fn tree_node(&self, ix: usize) -> TreeNode {
        let node = &self.nodes[ix];
        let mut values = Map::new();
        let mut put = |key: &str, value: Option<f64>| {
            if let Some(v) = value.and_then(serde_json::Number::from_f64) {
                values.insert(key.to_owned(), Value::Number(v));
            }
        };
        if self.analyzed {
            put("time", node.inclusive_time);
            put("self", node.self_time);
            put(
                "self %",
                self.self_share(ix).map(|s| (s * 1000.0).round() / 10.0),
            );
            put("rows", node.total_rows());
            put("est. rows", node.estimated_rows());
            put("loops", node.loops);
        } else {
            put("est. rows", node.plan_rows);
        }
        if let Some(b) = node.buffers {
            #[allow(clippy::cast_precision_loss)]
            {
                put("hit", Some(b.shared_hit as f64));
                put("read", Some(b.shared_read as f64));
                let temp = b.temp_read + b.temp_written;
                put("temp", (temp > 0).then_some(temp as f64));
            }
        }
        put("cost", node.total_cost);
        if let Some((factor, under)) = node.misestimate()
            && factor >= 2.0
        {
            let direction = if under { "under" } else { "over" };
            values.insert(
                "misestimate".into(),
                format!("×{} {direction}", format_number(round_factor(factor))).into(),
            );
        }
        let mut note: Vec<String> = Vec::new();
        if let Some(condition) = &node.condition {
            note.push(condition.clone());
        }
        note.extend(node.warnings.iter().cloned());
        if !note.is_empty() {
            values.insert("note".into(), note.join("; ").into());
        }
        let (badge, tone) = if self.is_hot(ix) {
            let share = self.self_share(ix).unwrap_or(0.0) * 100.0;
            (Some(format!("hot {share:.0}%")), Some(BadgeTone::Error))
        } else if node.never_executed() {
            (Some("never executed".to_owned()), Some(BadgeTone::Neutral))
        } else if let Some((factor, _)) =
            node.misestimate().filter(|(f, _)| *f >= MISESTIMATE_FACTOR)
        {
            (
                Some(format!("rows ×{}", format_number(round_factor(factor)))),
                Some(BadgeTone::Warning),
            )
        } else if let Some(warning) = node.warnings.first() {
            (Some(short_warning(warning)), Some(BadgeTone::Warning))
        } else {
            (None, None)
        };
        TreeNode {
            label: node.label(),
            badge,
            tone,
            values,
            children: Vec::new(),
        }
    }

    /// One CSV line per node: label, depth, times, rows, loops, buffers.
    #[must_use]
    pub fn to_csv(&self) -> String {
        let mut out = String::from(
            "node,depth,time_ms,self_ms,rows,estimated_rows,loops,shared_hit,shared_read,cost\n",
        );
        let num = |v: Option<f64>| v.map(format_number).unwrap_or_default();
        for node in &self.nodes {
            let label = node.label();
            let label = if label.contains([',', '"', '\n']) {
                format!("\"{}\"", label.replace('"', "\"\""))
            } else {
                label
            };
            let (hit, read) = node.buffers.map_or((String::new(), String::new()), |b| {
                (b.shared_hit.to_string(), b.shared_read.to_string())
            });
            out.push_str(&format!(
                "{label},{},{},{},{},{},{},{hit},{read},{}\n",
                node.depth,
                num(node.inclusive_time),
                num(node.self_time),
                num(node.total_rows()),
                num(node.estimated_rows()),
                num(node.loops),
                num(node.total_cost),
            ));
        }
        out
    }
}

fn pop_into(stack: &mut Vec<(u32, TreeNode)>, roots: &mut Vec<TreeNode>) {
    if let Some((_, node)) = stack.pop() {
        match stack.last_mut() {
            Some((_, parent)) => parent.children.push(node),
            None => roots.push(node),
        }
    }
}

/// A factor for display: whole above 10, one decimal below.
#[must_use]
pub fn round_factor(factor: f64) -> f64 {
    if factor >= 10.0 {
        factor.round()
    } else {
        (factor * 10.0).round() / 10.0
    }
}

/// A badge from a warning: its first words.
fn short_warning(warning: &str) -> String {
    warning
        .split_whitespace()
        .take(3)
        .collect::<Vec<_>>()
        .join(" ")
}

/// What looks wrong with `node`.
fn warnings(node: &PlanNode) -> Vec<String> {
    let mut out = Vec::new();
    if node.sort_space_type.as_deref() == Some("Disk") {
        match node.sort_space_kb {
            Some(kb) => out.push(format!("sort spilled to disk ({} kB)", format_number(kb))),
            None => out.push("sort spilled to disk".to_owned()),
        }
    }
    if let Some(batches) = node.hash_batches.filter(|b| *b > 1) {
        out.push(format!("hash spilled to disk ({batches} batches)"));
    }
    if node.node_type.contains("Seq Scan")
        && let Some(removed) = node.rows_removed
    {
        let loops = node.loops.unwrap_or(1.0).max(1.0);
        let removed = removed * loops;
        let kept = node.total_rows().unwrap_or(0.0);
        if removed >= SEQ_SCAN_REMOVED && removed > kept {
            out.push(format!(
                "seq scan filtered out {} rows (kept {}): an index may help",
                format_number(removed),
                format_number(kept)
            ));
        }
    }
    if let Some(b) = node.buffers
        && b.temp_written > 0
        && node.sort_space_type.as_deref() != Some("Disk")
        && node.hash_batches.is_none_or(|n| n <= 1)
    {
        out.push(format!("wrote {} temp blocks", b.temp_written));
    }
    if let Some((factor, under)) = node.misestimate()
        && factor >= MISESTIMATE_FACTOR
    {
        out.push(format!(
            "rows misestimated ×{} ({})",
            format_number(round_factor(factor)),
            if under {
                "more than planned"
            } else {
                "fewer than planned"
            }
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(node_type: &str, depth: u32) -> PlanNode {
        PlanNode {
            node_type: node_type.into(),
            depth,
            ..PlanNode::default()
        }
    }

    fn spec() -> QueryPlanSpec {
        let mut root = node("Hash Join", 0);
        root.join_type = Some("Inner".into());
        root.actual_time = Some(100.0);
        root.actual_rows = Some(5000.0);
        root.plan_rows = Some(50.0);
        root.loops = Some(1.0);
        root.total_cost = Some(1234.5);
        let mut scan = node("Seq Scan", 1);
        scan.relation = Some("orders".into());
        scan.alias = Some("o".into());
        scan.actual_time = Some(70.0);
        scan.actual_rows = Some(10_000.0);
        scan.plan_rows = Some(9000.0);
        scan.loops = Some(1.0);
        scan.rows_removed = Some(90_000.0);
        let mut hash = node("Hash", 1);
        hash.actual_time = Some(20.0);
        hash.actual_rows = Some(10.0);
        hash.plan_rows = Some(10.0);
        hash.loops = Some(1.0);
        hash.hash_batches = Some(4);
        let mut inner = node("Index Scan", 2);
        inner.index = Some("users_pkey".into());
        inner.relation = Some("users".into());
        inner.actual_time = Some(5.0);
        inner.actual_rows = Some(1.0);
        inner.plan_rows = Some(1.0);
        inner.loops = Some(3.0);
        let mut spec = QueryPlanSpec {
            title: None,
            path: None,
            engine: PlanEngine::Postgres,
            analyzed: true,
            buffers: false,
            planning_time: Some(0.5),
            execution_time: Some(101.0),
            trigger_time: None,
            jit_time: None,
            nodes: vec![root, scan, hash, inner],
        };
        spec.analyse();
        spec
    }

    #[test]
    fn self_times_subtract_children_over_loops() {
        let spec = spec();
        let times: Vec<_> = spec.nodes.iter().map(|n| n.self_time).collect();
        // inner: 5 ms × 3 loops = 15; hash: 20 - 15 = 5; root: 100 - 70 - 20.
        assert_eq!(times, [Some(10.0), Some(70.0), Some(5.0), Some(15.0)]);
        assert_eq!(spec.slowest(2), [1, 3]);
        assert!(spec.is_hot(1));
        assert!(!spec.is_hot(2));
        assert_eq!(spec.children(0), [1, 2]);
        assert_eq!(spec.parents(), [None, Some(0), Some(0), Some(2)]);
        spec.validate().unwrap();
    }

    #[test]
    fn parallel_nodes_are_divided_by_processes() {
        let mut gather = node("Gather", 0);
        gather.actual_time = Some(50.0);
        gather.loops = Some(1.0);
        gather.workers = Some(2);
        let mut scan = node("Parallel Seq Scan", 1);
        scan.actual_time = Some(40.0);
        scan.loops = Some(3.0);
        let mut spec = QueryPlanSpec {
            nodes: vec![gather, scan],
            ..spec()
        };
        spec.analyse();
        assert_eq!(spec.nodes[1].inclusive_time, Some(40.0));
        assert_eq!(spec.nodes[0].self_time, Some(10.0));
    }

    #[test]
    fn misestimates_and_warnings() {
        let spec = spec();
        let worst = spec.misestimates(2.0, 5);
        assert_eq!(worst.len(), 1);
        assert_eq!(worst[0].node, 0);
        assert!((worst[0].factor - 100.0).abs() < 1e-9);
        assert!(worst[0].under);
        assert_eq!(
            spec.nodes[0].warnings,
            ["rows misestimated ×100 (more than planned)"]
        );
        assert_eq!(
            spec.nodes[1].warnings,
            ["seq scan filtered out 90000 rows (kept 10000): an index may help"]
        );
        assert_eq!(spec.nodes[2].warnings, ["hash spilled to disk (4 batches)"]);
        assert_eq!(spec.nodes[1].label(), "Seq Scan on orders o");
        assert_eq!(
            spec.nodes[3].label(),
            "Index Scan using users_pkey on users"
        );
    }

    #[test]
    fn the_tree_nests_and_badges_nodes() {
        let spec = spec();
        let tree = spec.to_tree();
        tree.validate().unwrap();
        assert_eq!(tree.nodes.len(), 1);
        let root = &tree.nodes[0];
        assert_eq!(root.label, "Hash Join");
        assert_eq!(root.badge.as_deref(), Some("rows ×100"));
        assert_eq!(root.children.len(), 2);
        assert_eq!(root.children[0].badge.as_deref(), Some("hot 69%"));
        assert_eq!(root.children[0].tone, Some(BadgeTone::Error));
        assert_eq!(
            root.children[1].children[0].label,
            "Index Scan using users_pkey on users"
        );
        assert_eq!(
            root.values.get("misestimate").and_then(Value::as_str),
            Some("×100 under")
        );
        let names: Vec<_> = tree.columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "time",
                "self",
                "self %",
                "rows",
                "est. rows",
                "misestimate",
                "loops",
                "cost",
                "note"
            ]
        );
        let csv = spec.to_csv();
        assert!(csv.starts_with("node,depth,"), "{csv}");
        assert!(
            csv.contains("\nSeq Scan on orders o,1,70,70,10000,9000,1,,,\n"),
            "{csv}"
        );
    }

    #[test]
    fn bad_depths_are_rejected() {
        let mut spec = spec();
        spec.nodes[1].depth = 3;
        assert!(spec.validate().is_err());
        spec.nodes.clear();
        assert!(spec.validate().is_err());
    }
}
