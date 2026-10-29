//! The causal loop diagram, computed from checked claims.
//!
//! Only claims whose check is `Keep` become edges. Every simple cycle is a
//! loop, **reinforcing** with an even number of negative edges and
//! **balancing** with an odd number. Open questions are listed, not drawn.
//! The model never draws this diagram; it is rendered to Mermaid here.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

use super::claim::{Claim, ClaimCheck, ClaimId, Sign};

/// An edge of the diagram.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edge {
    /// The claim it comes from.
    pub claim: ClaimId,
    /// From.
    pub origin: String,
    /// To.
    pub destination: String,
    /// Its sign.
    pub sign: Sign,
}

/// Reinforcing or balancing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoopKind {
    /// An even number of negative edges: change feeds itself.
    Reinforcing,
    /// An odd number: change is resisted.
    Balancing,
}

impl LoopKind {
    /// `R` or `B`.
    #[must_use]
    pub const fn letter(self) -> char {
        match self {
            Self::Reinforcing => 'R',
            Self::Balancing => 'B',
        }
    }
}

/// A simple cycle of the diagram.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Loop {
    /// `R1`, `B2`…
    pub name: String,
    /// The variables, in order; the last links back to the first.
    pub nodes: Vec<String>,
    /// Its kind.
    pub kind: LoopKind,
}

/// A claim that is not drawn, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Undrawn {
    /// The claim.
    pub claim: ClaimId,
    /// `open` or `rejected`.
    pub status: String,
    /// Why.
    pub why: String,
}

/// The computed diagram.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Cld {
    /// Variables, in first-seen order.
    pub nodes: Vec<String>,
    /// Edges, in claim order.
    pub edges: Vec<Edge>,
    /// Loops, by their smallest node then length.
    pub loops: Vec<Loop>,
    /// Claims that are open questions.
    pub open: Vec<Undrawn>,
    /// Claims that were rejected.
    pub rejected: Vec<Undrawn>,
}

impl Cld {
    /// Builds the diagram from `claims` and their checks (paired by index).
    #[must_use]
    pub fn build(claims: &[(Claim, ClaimCheck)]) -> Self {
        let mut cld = Self::default();
        let mut index: BTreeMap<String, usize> = BTreeMap::new();
        let mut adjacency: Vec<Vec<(usize, Sign)>> = Vec::new();
        for (claim, check) in claims {
            match check {
                ClaimCheck::Keep { sign } => {
                    let a = node_of(&claim.origin, &mut cld.nodes, &mut index);
                    let b = node_of(&claim.destination, &mut cld.nodes, &mut index);
                    adjacency.resize(cld.nodes.len(), Vec::new());
                    if let Some(out) = adjacency.get_mut(a) {
                        out.push((b, *sign));
                    }
                    cld.edges.push(Edge {
                        claim: claim.id,
                        origin: claim.origin.clone(),
                        destination: claim.destination.clone(),
                        sign: *sign,
                    });
                }
                ClaimCheck::Open { why } => cld.open.push(Undrawn {
                    claim: claim.id,
                    status: "open".to_owned(),
                    why: why.clone(),
                }),
                ClaimCheck::Reject { why } => cld.rejected.push(Undrawn {
                    claim: claim.id,
                    status: "rejected".to_owned(),
                    why: why.clone(),
                }),
            }
        }
        adjacency.resize(cld.nodes.len(), Vec::new());
        cld.loops = find_loops(&cld.nodes, &adjacency);
        cld
    }

    /// Whether anything is drawn.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.edges.is_empty()
    }

    /// The diagram as a Mermaid flowchart, loops and undrawn claims as
    /// comments.
    #[must_use]
    pub fn to_mermaid(&self) -> String {
        let mut out = String::from("flowchart LR\n");
        for (i, node) in self.nodes.iter().enumerate() {
            let _ = writeln!(out, "    n{i}[\"{}\"]", escape(node));
        }
        for edge in &self.edges {
            let a = self.node_index(&edge.origin);
            let b = self.node_index(&edge.destination);
            let arrow = match edge.sign {
                Sign::Plus => "-->",
                Sign::Minus => "-.->",
            };
            let _ = writeln!(
                out,
                "    n{a} {arrow}|\"{} {}\"| n{b}",
                edge.sign.symbol(),
                edge.claim
            );
        }
        for l in &self.loops {
            let kind = match l.kind {
                LoopKind::Reinforcing => "reinforcing",
                LoopKind::Balancing => "balancing",
            };
            let _ = writeln!(
                out,
                "    %% loop {}: {} → {} ({kind})",
                l.name,
                l.nodes.join(" → "),
                l.nodes.first().map_or("", String::as_str)
            );
        }
        for u in self.open.iter().chain(&self.rejected) {
            let _ = writeln!(out, "    %% {} {}: {}", u.status, u.claim, u.why);
        }
        out
    }

    fn node_index(&self, name: &str) -> usize {
        self.nodes.iter().position(|n| n == name).unwrap_or(0)
    }
}

/// The index of `name` in `nodes`, adding it if new.
fn node_of(name: &str, nodes: &mut Vec<String>, index: &mut BTreeMap<String, usize>) -> usize {
    if let Some(i) = index.get(name) {
        return *i;
    }
    let i = nodes.len();
    nodes.push(name.to_owned());
    index.insert(name.to_owned(), i);
    i
}

/// Every simple cycle, each once: a depth-first search from each node
/// through nodes with a greater index only, so a cycle is found from its
/// smallest node and in one rotation.
fn find_loops(nodes: &[String], adjacency: &[Vec<(usize, Sign)>]) -> Vec<Loop> {
    let mut loops = Vec::new();
    let mut counts = [0usize; 2];
    for start in 0..nodes.len() {
        let mut path = vec![start];
        let mut negatives = vec![0usize];
        let mut on_path = vec![false; nodes.len()];
        on_path[start] = true;
        dfs(
            start,
            start,
            adjacency,
            &mut path,
            &mut negatives,
            &mut on_path,
            &mut |cycle: &[usize], negative: usize| {
                let kind = if negative.is_multiple_of(2) {
                    LoopKind::Reinforcing
                } else {
                    LoopKind::Balancing
                };
                let slot = usize::from(kind == LoopKind::Balancing);
                counts[slot] += 1;
                loops.push(Loop {
                    name: format!("{}{}", kind.letter(), counts[slot]),
                    nodes: cycle.iter().map(|i| nodes[*i].clone()).collect(),
                    kind,
                });
            },
        );
    }
    loops
}

fn dfs(
    start: usize,
    at: usize,
    adjacency: &[Vec<(usize, Sign)>],
    path: &mut Vec<usize>,
    negatives: &mut Vec<usize>,
    on_path: &mut [bool],
    found: &mut impl FnMut(&[usize], usize),
) {
    let Some(edges) = adjacency.get(at) else {
        return;
    };
    for (next, sign) in edges {
        let negative = negatives.last().copied().unwrap_or(0) + usize::from(*sign == Sign::Minus);
        if *next == start {
            found(path, negative);
            continue;
        }
        if *next < start || on_path[*next] {
            continue;
        }
        on_path[*next] = true;
        path.push(*next);
        negatives.push(negative);
        dfs(start, *next, adjacency, path, negatives, on_path, found);
        negatives.pop();
        path.pop();
        on_path[*next] = false;
    }
}

fn escape(text: &str) -> String {
    text.replace('"', "'")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::claim::Direction;
    use crate::discovery::event::SourceId;

    fn kept(id: u32, from: &str, to: &str, sign: Sign) -> (Claim, ClaimCheck) {
        let direction = match sign {
            Sign::Plus => Direction::IncreasingIncreases,
            Sign::Minus => Direction::IncreasingDecreases,
        };
        (
            Claim {
                id: ClaimId(id),
                origin: from.into(),
                destination: to.into(),
                direction,
                stated_sign: sign,
                span: "x".into(),
                source: SourceId(1),
            },
            ClaimCheck::Keep { sign },
        )
    }

    #[test]
    fn a_two_node_loop_with_one_negative_edge_is_balancing() {
        let cld = Cld::build(&[
            kept(1, "cache size", "latency", Sign::Plus),
            kept(2, "latency", "cache size", Sign::Minus),
        ]);
        assert_eq!(cld.nodes, vec!["cache size", "latency"]);
        assert_eq!(cld.loops.len(), 1);
        assert_eq!(cld.loops[0].kind, LoopKind::Balancing);
        assert_eq!(cld.loops[0].name, "B1");
        assert_eq!(cld.loops[0].nodes, vec!["cache size", "latency"]);
    }

    #[test]
    fn two_negatives_make_a_reinforcing_loop_and_each_cycle_is_found_once() {
        let cld = Cld::build(&[
            kept(1, "a", "b", Sign::Minus),
            kept(2, "b", "c", Sign::Plus),
            kept(3, "c", "a", Sign::Minus),
            kept(4, "b", "a", Sign::Plus),
        ]);
        let names: Vec<_> = cld
            .loops
            .iter()
            .map(|l| (l.name.as_str(), l.kind))
            .collect();
        assert_eq!(
            names,
            vec![("R1", LoopKind::Reinforcing), ("B1", LoopKind::Balancing)]
        );
        assert_eq!(cld.loops[0].nodes, vec!["a", "b", "c"]);
        assert_eq!(cld.loops[1].nodes, vec!["a", "b"]);
    }

    #[test]
    fn open_and_rejected_claims_are_listed_not_drawn() {
        let (open, _) = kept(5, "x", "y", Sign::Plus);
        let (rejected, _) = kept(6, "y", "x", Sign::Plus);
        let cld = Cld::build(&[
            kept(1, "x", "y", Sign::Plus),
            (
                open,
                ClaimCheck::Open {
                    why: "no span".into(),
                },
            ),
            (
                rejected,
                ClaimCheck::Reject {
                    why: "not found".into(),
                },
            ),
        ]);
        assert_eq!(cld.edges.len(), 1);
        assert!(cld.loops.is_empty());
        assert_eq!(cld.open.len(), 1);
        assert_eq!(cld.rejected.len(), 1);
        let mermaid = cld.to_mermaid();
        assert!(mermaid.contains("%% open CL5: no span"), "{mermaid}");
        assert!(mermaid.contains("%% rejected CL6: not found"), "{mermaid}");
    }

    #[test]
    fn mermaid_draws_signs_and_loops() {
        let cld = Cld::build(&[
            kept(1, "load", "queue \"depth\"", Sign::Plus),
            kept(2, "queue \"depth\"", "load", Sign::Minus),
        ]);
        let m = cld.to_mermaid();
        assert!(m.starts_with("flowchart LR\n"));
        assert!(m.contains("n1[\"queue 'depth'\"]"), "{m}");
        assert!(m.contains("n0 -->|\"+ CL1\"| n1"), "{m}");
        assert!(m.contains("n1 -.->|\"− CL2\"| n0"), "{m}");
        assert!(
            m.contains("%% loop B1: load → queue 'depth' → load (balancing)")
                || m.contains("%% loop B1: load → queue \"depth\" → load (balancing)"),
            "{m}"
        );
    }

    #[test]
    fn an_empty_build_is_empty() {
        let cld = Cld::build(&[]);
        assert!(cld.is_empty());
        assert_eq!(cld.to_mermaid(), "flowchart LR\n");
    }
}
