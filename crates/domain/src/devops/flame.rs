//! A profile as a flame graph (`show_flamegraph`).
//!
//! The adapter merges the stacks of a folded-stack file (`a;b;c 12`) or a
//! pprof profile into a call tree and flattens it here: [`FlameSpec::nodes`]
//! holds the root (`all`) first, then every frame followed by its
//! callees, siblings sorted by name. Each node knows its depth and where
//! it starts on the value axis, so the block draws it without rebuilding
//! the tree; the helpers sum self time per function and follow the
//! hottest path for the summary.

use std::collections::HashMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::invalid;
use crate::block::BlockError;

/// Nodes a flame graph keeps; smaller frames beyond it are hidden.
pub const MAX_NODES: usize = 100_000;
/// Deepest stack drawn.
pub const MAX_DEPTH: u32 = 1_024;
/// Name of the root node.
pub const ROOT: &str = "all";

const BLOCK: &str = "flame_graph";

/// The file format a profile came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProfileFormat {
    /// Brendan Gregg's collapsed stacks: `frame;frame;frame count`.
    Folded,
    /// A pprof `profile.proto` (gzipped or not).
    Pprof,
}

impl ProfileFormat {
    /// Display name.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Folded => "folded stacks",
            Self::Pprof => "pprof",
        }
    }
}

/// One frame of the call tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FlameNode {
    /// Function (or frame) name.
    pub name: String,
    /// 0 for the root, 1 for its callees...
    pub depth: u32,
    /// Offset on the value axis: the parent's start plus the totals of the
    /// siblings before it.
    pub start: u64,
    /// Value of the frame and everything it called.
    pub total: u64,
    /// Value spent in the frame itself.
    pub self_value: u64,
}

/// Self and total value of one function over the whole tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameStat {
    /// Function name.
    pub name: String,
    /// Sum of the self values of its frames.
    pub self_value: u64,
    /// Sum of the totals of its outermost frames (recursion counted once).
    pub total: u64,
}

/// A call tree to draw as an icicle.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct FlameSpec {
    /// Caption; the file name when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Workspace path of the file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// What the file was.
    pub format: ProfileFormat,
    /// What a value counts: `samples`, or a pprof sample type such as
    /// `cpu nanoseconds` or `alloc_space bytes`.
    pub unit: String,
    /// The root first, then each frame followed by its callees.
    pub nodes: Vec<FlameNode>,
    /// Frames left out because they were too small to draw.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub hidden: usize,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_zero(n: &usize) -> bool {
    *n == 0
}

impl FlameSpec {
    /// Checks the size, that the root comes first and that every node
    /// sits inside its parent.
    ///
    /// # Errors
    ///
    /// [`BlockError::Invalid`] naming the first problem.
    pub fn validate(&self) -> Result<(), BlockError> {
        if self.nodes.len() > MAX_NODES {
            return Err(invalid(
                BLOCK,
                format!("{} frames; at most {MAX_NODES} are kept", self.nodes.len()),
            ));
        }
        let Some(root) = self.nodes.first() else {
            return Err(invalid(BLOCK, "the profile has no root frame"));
        };
        if root.depth != 0 || root.start != 0 {
            return Err(invalid(BLOCK, "the first frame must be the root"));
        }
        let parents = self.parents();
        for (ix, node) in self.nodes.iter().enumerate().skip(1) {
            if node.depth > MAX_DEPTH {
                return Err(invalid(
                    BLOCK,
                    format!("frame {ix} is {} deep (max {MAX_DEPTH})", node.depth),
                ));
            }
            let Some(p) = parents[ix] else {
                return Err(invalid(BLOCK, format!("frame {ix} has no parent")));
            };
            let parent = &self.nodes[p];
            if node.depth != parent.depth + 1
                || node.start < parent.start
                || node.start + node.total > parent.start + parent.total
            {
                return Err(invalid(
                    BLOCK,
                    format!("frame {ix} ({:?}) lies outside its caller", node.name),
                ));
            }
            if node.self_value > node.total {
                return Err(invalid(
                    BLOCK,
                    format!("frame {ix} has more self value than total"),
                ));
            }
        }
        Ok(())
    }

    /// The root's total.
    #[must_use]
    pub fn total(&self) -> u64 {
        self.nodes.first().map_or(0, |n| n.total)
    }

    /// Deepest frame.
    #[must_use]
    pub fn max_depth(&self) -> u32 {
        self.nodes.iter().map(|n| n.depth).max().unwrap_or(0)
    }

    /// The parent of every node (`None` for the root).
    #[must_use]
    pub fn parents(&self) -> Vec<Option<usize>> {
        let mut stack: Vec<usize> = Vec::new();
        self.nodes
            .iter()
            .enumerate()
            .map(|(ix, node)| {
                while stack
                    .last()
                    .is_some_and(|&top| self.nodes[top].depth >= node.depth)
                {
                    stack.pop();
                }
                let parent = stack.last().copied();
                stack.push(ix);
                parent
            })
            .collect()
    }

    /// The names from the root's first callee down to node `ix`.
    #[must_use]
    pub fn stack(&self, ix: usize) -> Vec<&str> {
        let parents = self.parents();
        let mut out = Vec::new();
        let mut at = Some(ix);
        while let Some(i) = at {
            if i == 0 {
                break;
            }
            out.push(self.nodes[i].name.as_str());
            at = parents[i];
        }
        out.reverse();
        out
    }

    /// Self and total value per function name, most self first; ties by
    /// name.
    #[must_use]
    pub fn frame_stats(&self) -> Vec<FrameStat> {
        let mut stats: HashMap<&str, (u64, u64)> = HashMap::new();
        // Names on the path from the root to the current node, to count
        // a recursive function's total once.
        let mut path: Vec<&str> = Vec::new();
        for node in self.nodes.iter().skip(1) {
            path.truncate(node.depth.saturating_sub(1) as usize);
            let entry = stats.entry(node.name.as_str()).or_default();
            entry.0 += node.self_value;
            if !path.contains(&node.name.as_str()) {
                entry.1 += node.total;
            }
            path.push(node.name.as_str());
        }
        let mut out: Vec<FrameStat> = stats
            .into_iter()
            .map(|(name, (self_value, total))| FrameStat {
                name: name.to_owned(),
                self_value,
                total,
            })
            .collect();
        out.sort_by(|a, b| {
            b.self_value
                .cmp(&a.self_value)
                .then(b.total.cmp(&a.total))
                .then(a.name.cmp(&b.name))
        });
        out
    }

    /// From the root, the heaviest callee at each level, while it holds
    /// at least half of its caller.
    #[must_use]
    pub fn hottest_path(&self) -> Vec<usize> {
        let parents = self.parents();
        let mut path = Vec::new();
        let mut at = 0;
        loop {
            let best = (at + 1..self.nodes.len())
                .take_while(|&ix| self.nodes[ix].depth > self.nodes[at].depth)
                .filter(|&ix| parents[ix] == Some(at))
                .max_by(|&a, &b| {
                    self.nodes[a]
                        .total
                        .cmp(&self.nodes[b].total)
                        .then(b.cmp(&a))
                });
            match best {
                Some(ix) if self.nodes[ix].total * 2 >= self.nodes[at].total.max(1) => {
                    path.push(ix);
                    at = ix;
                }
                _ => break,
            }
        }
        path
    }

    /// Nodes whose name contains `query` (ignoring ASCII case), and the
    /// share of the total they cover (nested matches counted once).
    #[must_use]
    pub fn search(&self, query: &str) -> (Vec<usize>, f64) {
        let needle = query.trim().to_ascii_lowercase();
        if needle.is_empty() {
            return (Vec::new(), 0.0);
        }
        let mut hits = Vec::new();
        let mut covered = 0u64;
        // Depth of the outermost matching frame being inside of, if any.
        let mut inside: Option<u32> = None;
        for (ix, node) in self.nodes.iter().enumerate().skip(1) {
            if inside.is_some_and(|d| node.depth <= d) {
                inside = None;
            }
            if node.name.to_ascii_lowercase().contains(&needle) {
                hits.push(ix);
                if inside.is_none() {
                    covered += node.total;
                    inside = Some(node.depth);
                }
            }
        }
        let total = self.total();
        let share = if total == 0 {
            0.0
        } else {
            covered as f64 / total as f64
        };
        (hits, share)
    }

    /// The tree back as folded stacks (one line per frame with self
    /// value), for copying into other tools.
    #[must_use]
    pub fn to_folded(&self) -> String {
        let mut out = String::new();
        let mut path: Vec<&str> = Vec::new();
        for node in self.nodes.iter().skip(1) {
            path.truncate(node.depth.saturating_sub(1) as usize);
            path.push(node.name.as_str());
            if node.self_value > 0 {
                out.push_str(&path.join(";"));
                out.push(' ');
                out.push_str(&node.self_value.to_string());
                out.push('\n');
            }
        }
        out
    }
}

/// A call tree being built from stacks, flattened by [`FlameTree::finish`].
#[derive(Debug, Clone, Default)]
pub struct FlameTree {
    nodes: Vec<TreeNode>,
}

#[derive(Debug, Clone, Default)]
struct TreeNode {
    name: String,
    total: u64,
    self_value: u64,
    children: HashMap<String, usize>,
}

impl FlameTree {
    /// An empty tree (only the root).
    #[must_use]
    pub fn new() -> Self {
        Self {
            nodes: vec![TreeNode {
                name: ROOT.to_owned(),
                ..TreeNode::default()
            }],
        }
    }

    /// Adds `value` to the stack `frames`, outermost first. Empty frame
    /// names are skipped; frames deeper than [`MAX_DEPTH`] are folded into
    /// the last kept one.
    pub fn add<'a>(&mut self, frames: impl IntoIterator<Item = &'a str>, value: u64) {
        if value == 0 {
            return;
        }
        let mut at = 0;
        self.nodes[0].total += value;
        for (depth, frame) in frames.into_iter().filter(|f| !f.is_empty()).enumerate() {
            if depth >= MAX_DEPTH as usize {
                break;
            }
            let next = match self.nodes[at].children.get(frame) {
                Some(&ix) => ix,
                None => {
                    let ix = self.nodes.len();
                    self.nodes.push(TreeNode {
                        name: frame.to_owned(),
                        ..TreeNode::default()
                    });
                    self.nodes[at].children.insert(frame.to_owned(), ix);
                    ix
                }
            };
            at = next;
            self.nodes[at].total += value;
        }
        self.nodes[at].self_value += value;
    }

    /// Frames so far, the root included.
    #[must_use]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Whether nothing was added.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes[0].total == 0
    }

    /// The flattened tree: preorder, siblings by name. When there are more
    /// than [`MAX_NODES`] frames, the smallest are dropped (the smallest
    /// total that keeps the count in bound) and counted in `hidden`.
    #[must_use]
    pub fn finish(self, format: ProfileFormat, unit: &str) -> FlameSpec {
        let mut min_total = 0u64;
        if self.nodes.len() > MAX_NODES {
            let mut totals: Vec<u64> = self.nodes.iter().map(|n| n.total).collect();
            totals.sort_unstable_by(|a, b| b.cmp(a));
            // Keep strictly above the total of the first node that does
            // not fit; ties are dropped together.
            min_total = totals[MAX_NODES - 1].saturating_add(1);
            if totals.first().is_some_and(|&t| t < min_total) {
                min_total = totals[0];
            }
        }
        let mut nodes = Vec::with_capacity(self.nodes.len().min(MAX_NODES));
        let mut hidden = 0;
        // (tree index, depth, start)
        let mut work = vec![(0usize, 0u32, 0u64)];
        while let Some((ix, depth, start)) = work.pop() {
            let node = &self.nodes[ix];
            nodes.push(FlameNode {
                name: node.name.clone(),
                depth,
                start,
                total: node.total,
                self_value: node.self_value,
            });
            let mut kids: Vec<(&String, usize)> =
                node.children.iter().map(|(k, &v)| (k, v)).collect();
            kids.sort_by(|a, b| a.0.cmp(b.0));
            let mut offset = start;
            let mut next = Vec::with_capacity(kids.len());
            for (_, kid) in kids {
                let total = self.nodes[kid].total;
                if total < min_total || nodes.len() + work.len() + next.len() >= MAX_NODES {
                    hidden += 1 + self.descendants(kid);
                } else {
                    next.push((kid, depth + 1, offset));
                }
                offset += total;
            }
            work.extend(next.into_iter().rev());
        }
        FlameSpec {
            title: None,
            path: None,
            format,
            unit: unit.to_owned(),
            nodes,
            hidden,
        }
    }

    fn descendants(&self, ix: usize) -> usize {
        let mut count = 0;
        let mut work: Vec<usize> = self.nodes[ix].children.values().copied().collect();
        while let Some(at) = work.pop() {
            count += 1;
            work.extend(self.nodes[at].children.values().copied());
        }
        count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> FlameSpec {
        let mut tree = FlameTree::new();
        tree.add(["main", "parse", "lex"], 30);
        tree.add(["main", "parse"], 10);
        tree.add(["main", "eval", "eval", "add"], 50);
        tree.add(["main", "eval"], 5);
        tree.add(["idle"], 5);
        tree.finish(ProfileFormat::Folded, "samples")
    }

    #[test]
    fn tree_is_preorder_with_offsets() {
        let spec = sample();
        spec.validate().unwrap();
        let names: Vec<_> = spec
            .nodes
            .iter()
            .map(|n| (n.name.as_str(), n.depth, n.start, n.total, n.self_value))
            .collect();
        assert_eq!(
            names,
            [
                ("all", 0, 0, 100, 0),
                ("idle", 1, 0, 5, 5),
                ("main", 1, 5, 95, 0),
                ("eval", 2, 5, 55, 5),
                ("eval", 3, 5, 50, 0),
                ("add", 4, 5, 50, 50),
                ("parse", 2, 60, 40, 10),
                ("lex", 3, 60, 30, 30),
            ]
        );
        assert_eq!(spec.total(), 100);
        assert_eq!(spec.max_depth(), 4);
        assert_eq!(spec.stack(5), ["main", "eval", "eval", "add"]);
    }

    #[test]
    fn stats_count_recursion_once() {
        let stats = sample().frame_stats();
        let eval = stats.iter().find(|s| s.name == "eval").unwrap();
        assert_eq!((eval.self_value, eval.total), (5, 55));
        assert_eq!(stats[0].name, "add");
        assert_eq!(stats[1].name, "lex");
    }

    #[test]
    fn hottest_path_and_search() {
        let spec = sample();
        let path: Vec<_> = spec
            .hottest_path()
            .into_iter()
            .map(|ix| spec.nodes[ix].name.as_str())
            .collect();
        assert_eq!(path, ["main", "eval", "eval", "add"]);
        let (hits, share) = spec.search("EVAL");
        assert_eq!(hits, [3, 4]);
        assert!((share - 0.55).abs() < 1e-12);
        assert_eq!(spec.search(" ").0, Vec::<usize>::new());
    }

    #[test]
    fn folded_round_trip() {
        let spec = sample();
        assert_eq!(
            spec.to_folded(),
            "idle 5\nmain;eval 5\nmain;eval;eval;add 50\nmain;parse 10\nmain;parse;lex 30\n"
        );
    }

    #[test]
    fn too_many_frames_hide_the_smallest() {
        let mut tree = FlameTree::new();
        for ix in 0..MAX_NODES + 10 {
            let name = format!("f{ix}");
            tree.add(["main", name.as_str()], if ix < 5 { 1_000 } else { 1 });
        }
        let spec = tree.finish(ProfileFormat::Folded, "samples");
        spec.validate().unwrap();
        assert!(spec.nodes.len() <= MAX_NODES);
        assert_eq!(spec.nodes.len() + spec.hidden, MAX_NODES + 12);
        assert_eq!(spec.nodes.len(), 7, "root, main and the five big ones");
    }

    #[test]
    fn rejects_bad_trees() {
        let mut spec = sample();
        spec.nodes[3].start = 99;
        assert!(spec.validate().unwrap_err().to_string().contains("outside"));
        let empty = FlameSpec {
            nodes: Vec::new(),
            ..sample()
        };
        assert!(empty.validate().is_err());
    }
}
