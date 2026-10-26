//! Networks for `show_network`: an edge list (CSV/TSV) or node-link JSON
//! into a `petgraph` graph, with per-node metrics.
//!
//! petgraph holds the graph and gives the strongly connected components
//! (Kosaraju) and the union-find for the weak ones; the metrics follow
//! NetworkX's definitions so results can be checked against it:
//!
//! - degree (in + out when directed), and strength (sum of weights);
//! - betweenness by Brandes' algorithm on hop counts, normalised by
//!   `1 / ((n − 1)(n − 2))` over ordered pairs (so an undirected path's
//!   middle node of 5 gets 4 / 6); sampled from a seeded subset of sources
//!   (and scaled by `n / k`) on large graphs, said in the envelope;
//! - closeness on hop counts, towards the node for directed graphs,
//!   Wasserman–Faust scaled (`(r − 1)/(n − 1) · (r − 1)/Σd`) when not
//!   everything is reachable;
//! - PageRank by power iteration (damping 0.85, uniform teleport and
//!   dangling redistribution, up to 100 iterations to an L1 change below
//!   `n · 1e-6`), weighted when the edges are; petgraph's own `page_rank`
//!   is quadratic per iteration and drops the teleport term on linked
//!   pairs, so it is not used;
//! - communities by asynchronous label propagation in a seeded random
//!   order (weighted, ties broken by the seed), with their modularity.
//!
//! Self-loops are dropped and repeated edges merged (weights summed), so
//! the metrics are those of the simple graph; both are noted.

use std::collections::{BTreeMap, HashMap, VecDeque};

use application::csv::{delimiter_for, records};
use application::ports::formats::NetworkRequest;
use domain::computed::{Computed, ComputedValue};
use domain::science::molecule_grid::value_key;
use domain::science::network::{MAX_EDGES, MAX_NODES, NetworkEdge, NetworkNode, NetworkSpec};
use petgraph::Direction;
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::unionfind::UnionFind;
use serde_json::Value;

use crate::{chem, sci};

/// Envelope kind of the metrics.
pub const KIND: &str = "network.metrics";
/// Engine name of the network metrics.
pub const GRAPH_ENGINE_NAME: &str = "crest-graph";
/// The library underneath.
pub const BACKEND: &str = "petgraph 0.8.3 (graph, components); crest metrics";
/// The default seed.
pub const DEFAULT_SEED: u64 = 20_261_026;
/// Above this many node × edge steps, betweenness is sampled.
pub const EXACT_BETWEENNESS: u64 = 50_000_000;
/// PageRank damping.
pub const DAMPING: f64 = 0.85;
/// Nodes whose own metrics go into the envelope.
pub const ENVELOPE_NODES: usize = 200;

const SOURCE: &[&str] = &["source", "from", "src", "node1", "u", "head"];
const TARGET: &[&str] = &["target", "to", "dst", "node2", "v", "tail"];
const WEIGHT: &[&str] = &["weight", "value", "w", "strength"];

/// Neighbours of a node with the edge weights.
type Adjacency = Vec<(usize, f64)>;

/// SplitMix64: a small seeded generator for the orders and samples.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        #[allow(clippy::cast_possible_truncation)]
        let pick = (self.next() % n.max(1) as u64) as usize;
        pick
    }

    fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            let j = self.below(i + 1);
            items.swap(i, j);
        }
    }
}

/// The parsed input: node ids (with labels) and edges by index.
#[derive(Default)]
struct Input {
    ids: Vec<String>,
    labels: Vec<Option<String>>,
    index: HashMap<String, usize>,
    edges: Vec<(usize, usize, f64)>,
    weighted: bool,
    directed: Option<bool>,
}

impl Input {
    fn node(&mut self, id: &str) -> usize {
        if let Some(ix) = self.index.get(id) {
            return *ix;
        }
        let ix = self.ids.len();
        self.ids.push(id.to_owned());
        self.labels.push(None);
        self.index.insert(id.to_owned(), ix);
        ix
    }
}

fn scalar_text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.trim().to_owned()).filter(|s| !s.is_empty()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn weight_of(v: Option<&Value>) -> Result<Option<f64>, String> {
    match v {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => Ok(n.as_f64()),
        Some(Value::String(s)) if s.trim().is_empty() => Ok(None),
        Some(Value::String(s)) => s
            .trim()
            .parse::<f64>()
            .map(Some)
            .map_err(|_| format!("weight {s:?} is not a number")),
        Some(other) => Err(format!("weight {other} is not a number")),
    }
}

fn read_json(value: Value, input: &mut Input) -> Result<(), String> {
    let (nodes, edges) = match value {
        Value::Object(mut obj) => {
            input.directed = obj.get("directed").and_then(Value::as_bool);
            let edges = obj
                .remove("links")
                .or_else(|| obj.remove("edges"))
                .ok_or("node-link JSON needs `links` or `edges`")?;
            (obj.remove("nodes"), edges)
        }
        list @ Value::Array(_) => (None, list),
        _ => {
            return Err("a network JSON is node-link (`nodes`, `links`) or a list of edges".into());
        }
    };
    if let Some(Value::Array(nodes)) = nodes {
        for n in nodes {
            let id = match &n {
                Value::Object(o) => o.get("id").and_then(scalar_text),
                other => scalar_text(other),
            }
            .ok_or("a node has no `id`")?;
            let ix = input.node(&id);
            if let Value::Object(o) = &n {
                input.labels[ix] = o
                    .get("label")
                    .or_else(|| o.get("name"))
                    .and_then(scalar_text)
                    .filter(|l| *l != id);
            }
        }
    }
    let Value::Array(edges) = edges else {
        return Err("`links` must be a list".into());
    };
    for (i, e) in edges.into_iter().enumerate() {
        let (a, b, w) = match &e {
            Value::Object(o) => (
                o.get("source")
                    .or_else(|| o.get("from"))
                    .and_then(scalar_text),
                o.get("target")
                    .or_else(|| o.get("to"))
                    .and_then(scalar_text),
                weight_of(o.get("weight")).map_err(|m| format!("edge {}: {m}", i + 1))?,
            ),
            Value::Array(pair) => (
                pair.first().and_then(scalar_text),
                pair.get(1).and_then(scalar_text),
                weight_of(pair.get(2)).map_err(|m| format!("edge {}: {m}", i + 1))?,
            ),
            _ => (None, None, None),
        };
        let (Some(a), Some(b)) = (a, b) else {
            return Err(format!("edge {} has no source or target", i + 1));
        };
        input.weighted |= w.is_some();
        let (a, b) = (input.node(&a), input.node(&b));
        input.edges.push((a, b, w.unwrap_or(1.0)));
    }
    Ok(())
}

fn column(
    header: &[String],
    given: Option<&str>,
    names: &[&str],
    fallback: Option<usize>,
) -> Result<Option<usize>, String> {
    if let Some(name) = given {
        return header
            .iter()
            .position(|h| h.trim().eq_ignore_ascii_case(name))
            .map(Some)
            .ok_or_else(|| format!("no column {name:?}; the header is {}", header.join(", ")));
    }
    Ok(header
        .iter()
        .position(|h| names.contains(&h.trim().to_ascii_lowercase().as_str()))
        .or(fallback))
}

fn read_csv(text: &str, request: &NetworkRequest, input: &mut Input) -> Result<(), String> {
    let delimiter = request.path.as_deref().map_or(',', delimiter_for);
    let mut rows = records(text, delimiter)?.into_iter();
    let header = rows.next().ok_or("the edge list is empty")?;
    if header.len() < 2 {
        return Err("an edge list needs a source and a target column".into());
    }
    let s = column(&header, request.source.as_deref(), SOURCE, Some(0))?.unwrap_or(0);
    let t = column(&header, request.target.as_deref(), TARGET, Some(1))?.unwrap_or(1);
    let w = column(&header, request.weight.as_deref(), WEIGHT, None)?;
    if s == t {
        return Err("the source and target columns are the same".into());
    }
    for (i, row) in rows.enumerate() {
        let cell = |c: usize| row.get(c).map(|v| v.trim()).filter(|v| !v.is_empty());
        let (Some(a), Some(b)) = (cell(s), cell(t)) else {
            // A row with only a source is an isolated node.
            if let Some(a) = cell(s) {
                input.node(a);
            }
            continue;
        };
        let weight = match w.and_then(cell) {
            Some(v) => Some(
                v.parse::<f64>()
                    .map_err(|_| format!("row {}: weight {v:?} is not a number", i + 2))?,
            ),
            None => None,
        };
        input.weighted |= weight.is_some();
        let (a, b) = (input.node(a), input.node(b));
        input.edges.push((a, b, weight.unwrap_or(1.0)));
    }
    Ok(())
}

/// Shortest-path data of one Brandes source.
fn brandes(adj: &[Vec<usize>], s: usize, bc: &mut [f64]) {
    let n = adj.len();
    let mut stack = Vec::with_capacity(n);
    let mut preds: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut sigma = vec![0.0f64; n];
    let mut dist = vec![-1i64; n];
    sigma[s] = 1.0;
    dist[s] = 0;
    let mut queue = VecDeque::from([s]);
    while let Some(v) = queue.pop_front() {
        stack.push(v);
        for &w in &adj[v] {
            if dist[w] < 0 {
                dist[w] = dist[v] + 1;
                queue.push_back(w);
            }
            if dist[w] == dist[v] + 1 {
                sigma[w] += sigma[v];
                preds[w].push(v);
            }
        }
    }
    let mut delta = vec![0.0f64; n];
    while let Some(w) = stack.pop() {
        for &v in &preds[w] {
            delta[v] += sigma[v] / sigma[w] * (1.0 + delta[w]);
        }
        if w != s {
            bc[w] += delta[w];
        }
    }
}

/// Hop distances from `s` over `adj`.
fn hops(adj: &[Vec<usize>], s: usize) -> Vec<i64> {
    let mut dist = vec![-1i64; adj.len()];
    dist[s] = 0;
    let mut queue = VecDeque::from([s]);
    while let Some(v) = queue.pop_front() {
        for &w in &adj[v] {
            if dist[w] < 0 {
                dist[w] = dist[v] + 1;
                queue.push_back(w);
            }
        }
    }
    dist
}

/// The network of `request`.
///
/// # Errors
///
/// A message for unreadable input, unknown columns, no edges or a network
/// over the limits.
#[allow(clippy::too_many_lines)]
pub fn analyse(request: &NetworkRequest) -> Result<NetworkSpec, String> {
    let text = std::str::from_utf8(&request.bytes).map_err(|_| "the network is not UTF-8")?;
    let text = text.trim_start_matches('\u{feff}');
    let mut input = Input::default();
    if text.trim_start().starts_with(['{', '[']) {
        let value: Value = serde_json::from_str(text).map_err(|e| format!("not JSON: {e}"))?;
        read_json(value, &mut input)?;
    } else {
        read_csv(text, request, &mut input)?;
    }
    if input.edges.is_empty() {
        return Err("the network has no edges".into());
    }
    if input.ids.len() > MAX_NODES || input.edges.len() > MAX_EDGES {
        return Err(format!(
            "{} nodes and {} edges; at most {MAX_NODES} and {MAX_EDGES}",
            input.ids.len(),
            input.edges.len()
        ));
    }
    let directed = request.directed.or(input.directed).unwrap_or(false);
    let n = input.ids.len();
    let mut warnings = Vec::new();
    // The simple graph: no self-loops, repeated edges merged.
    let mut merged: BTreeMap<(usize, usize), f64> = BTreeMap::new();
    let mut loops = 0;
    let mut repeats = 0;
    for &(a, b, w) in &input.edges {
        if a == b {
            loops += 1;
            continue;
        }
        let key = if directed || a < b { (a, b) } else { (b, a) };
        match merged.get_mut(&key) {
            Some(total) => {
                *total += w;
                repeats += 1;
            }
            None => {
                merged.insert(key, w);
            }
        }
    }
    if loops > 0 {
        warnings.push(format!("{loops} self-loops dropped"));
    }
    if repeats > 0 {
        warnings.push(format!(
            "{repeats} repeated edges merged{}",
            if input.weighted {
                " (weights summed)"
            } else {
                ""
            }
        ));
    }
    let mut graph: DiGraph<(), f64> = DiGraph::with_capacity(n, merged.len());
    let ix: Vec<NodeIndex> = (0..n).map(|_| graph.add_node(())).collect();
    for (&(a, b), &w) in &merged {
        graph.add_edge(ix[a], ix[b], w);
    }
    // Adjacency from the petgraph graph: out (successors, or neighbours)
    // and in (predecessors, or the same neighbours).
    let neighbours = |v: usize, dir: Option<Direction>| -> Vec<(usize, f64)> {
        let mut out: Vec<(usize, f64)> = match dir {
            Some(d) => graph
                .edges_directed(ix[v], d)
                .map(|e| {
                    let other = if d == Direction::Outgoing {
                        petgraph::visit::EdgeRef::target(&e)
                    } else {
                        petgraph::visit::EdgeRef::source(&e)
                    };
                    (other.index(), *e.weight())
                })
                .collect(),
            None => graph
                .edges_directed(ix[v], Direction::Outgoing)
                .map(|e| (petgraph::visit::EdgeRef::target(&e).index(), *e.weight()))
                .chain(
                    graph
                        .edges_directed(ix[v], Direction::Incoming)
                        .map(|e| (petgraph::visit::EdgeRef::source(&e).index(), *e.weight())),
                )
                .collect(),
        };
        out.sort_by_key(|(w, _)| *w);
        out
    };
    let (out_w, in_w): (Vec<Adjacency>, Vec<Adjacency>) = if directed {
        (
            (0..n)
                .map(|v| neighbours(v, Some(Direction::Outgoing)))
                .collect(),
            (0..n)
                .map(|v| neighbours(v, Some(Direction::Incoming)))
                .collect(),
        )
    } else {
        let both: Vec<_> = (0..n).map(|v| neighbours(v, None)).collect();
        (both.clone(), both)
    };
    let out_adj: Vec<Vec<usize>> = out_w
        .iter()
        .map(|l| l.iter().map(|(w, _)| *w).collect())
        .collect();
    let in_adj: Vec<Vec<usize>> = in_w
        .iter()
        .map(|l| l.iter().map(|(w, _)| *w).collect())
        .collect();
    let seed = request.seed.unwrap_or(DEFAULT_SEED);
    let mut rng = Rng(seed);
    #[allow(clippy::cast_precision_loss)]
    let nf = n as f64;

    // Components: weak by union-find, strong by Kosaraju.
    let mut uf = UnionFind::new(n);
    for e in graph.edge_indices() {
        if let Some((a, b)) = graph.edge_endpoints(e) {
            uf.union(a.index(), b.index());
        }
    }
    let component = renumber(&uf.into_labeling());
    let components = component.iter().copied().max().map_or(0, |m| m + 1);
    let strong_components = directed.then(|| petgraph::algo::kosaraju_scc(&graph).len());

    // Betweenness (Brandes, hop counts).
    let steps = (n as u64).saturating_mul(merged.len() as u64 + n as u64);
    let mut sources: Vec<usize> = (0..n).collect();
    let sampled = if steps > EXACT_BETWEENNESS {
        let k = ((EXACT_BETWEENNESS / (merged.len() as u64 + n as u64)).max(100) as usize).min(n);
        rng.shuffle(&mut sources);
        sources.truncate(k);
        Some(k)
    } else {
        None
    };
    let mut bc = vec![0.0f64; n];
    for &s in &sources {
        brandes(&out_adj, s, &mut bc);
    }
    if n > 2 {
        let mut scale = 1.0 / ((nf - 1.0) * (nf - 2.0));
        if let Some(k) = sampled {
            #[allow(clippy::cast_precision_loss)]
            {
                scale *= nf / k as f64;
            }
        }
        for v in &mut bc {
            *v *= scale;
        }
    } else {
        bc.iter_mut().for_each(|v| *v = 0.0);
    }
    if let Some(k) = sampled {
        warnings.push(format!(
            "betweenness sampled from {k} of {n} sources (seed {seed}), scaled by n/k"
        ));
    }

    // Closeness (towards the node), Wasserman–Faust scaled.
    let closeness: Vec<f64> = (0..n)
        .map(|u| {
            let dist = hops(&in_adj, u);
            let reach: Vec<i64> = dist.into_iter().filter(|d| *d >= 0).collect();
            #[allow(clippy::cast_precision_loss)]
            let (total, r) = (reach.iter().sum::<i64>() as f64, reach.len() as f64);
            if total > 0.0 && n > 1 {
                (r - 1.0) / total * ((r - 1.0) / (nf - 1.0))
            } else {
                0.0
            }
        })
        .collect();

    // PageRank by power iteration.
    let out_strength: Vec<f64> = out_w
        .iter()
        .map(|l| {
            l.iter()
                .map(|(_, w)| if input.weighted { *w } else { 1.0 })
                .sum()
        })
        .collect();
    let mut pr = vec![1.0 / nf; n];
    let mut iterations = 0;
    let mut converged = false;
    for _ in 0..100 {
        iterations += 1;
        let dangling: f64 = (0..n)
            .filter(|v| out_strength[*v] == 0.0)
            .map(|v| pr[v])
            .sum();
        let mut next = vec![(1.0 - DAMPING) / nf + DAMPING * dangling / nf; n];
        for v in 0..n {
            if out_strength[v] == 0.0 {
                continue;
            }
            for &(w, weight) in &out_w[v] {
                let weight = if input.weighted { weight } else { 1.0 };
                next[w] += DAMPING * pr[v] * weight / out_strength[v];
            }
        }
        let change: f64 = next.iter().zip(&pr).map(|(a, b)| (a - b).abs()).sum();
        pr = next;
        if change < nf * 1e-6 {
            converged = true;
            break;
        }
    }
    if !converged {
        warnings.push("PageRank did not converge in 100 iterations".into());
    }

    // Communities: asynchronous label propagation on the undirected view.
    let undirected: Vec<Vec<(usize, f64)>> = if directed {
        (0..n).map(|v| neighbours(v, None)).collect()
    } else {
        out_w.clone()
    };
    let mut labels: Vec<usize> = (0..n).collect();
    let mut order: Vec<usize> = (0..n).collect();
    for _ in 0..100 {
        rng.shuffle(&mut order);
        let mut changed = false;
        for &v in &order {
            if undirected[v].is_empty() {
                continue;
            }
            let mut weight: BTreeMap<usize, f64> = BTreeMap::new();
            for &(w, x) in &undirected[v] {
                *weight.entry(labels[w]).or_default() += if input.weighted { x } else { 1.0 };
            }
            let best = weight.values().copied().fold(f64::MIN, f64::max);
            let top: Vec<usize> = weight
                .iter()
                .filter(|(_, w)| (**w - best).abs() <= 1e-12)
                .map(|(l, _)| *l)
                .collect();
            if top.contains(&labels[v]) {
                continue;
            }
            labels[v] = top[rng.below(top.len())];
            changed = true;
        }
        if !changed {
            break;
        }
    }
    let community = renumber(&labels);
    let communities = community.iter().copied().max().map_or(0, |m| m + 1);
    let modularity = modularity(&merged, &community, directed, input.weighted);

    let degree: Vec<(f64, f64, f64)> = (0..n)
        .map(|v| {
            #[allow(clippy::cast_precision_loss)]
            let (o, i) = (out_adj[v].len() as f64, in_adj[v].len() as f64);
            let strength: f64 = if directed {
                out_w[v].iter().chain(&in_w[v]).map(|(_, w)| *w).sum()
            } else {
                out_w[v].iter().map(|(_, w)| *w).sum()
            };
            (if directed { o + i } else { o }, i, strength)
        })
        .collect();
    let nodes: Vec<NetworkNode> = (0..n)
        .map(|v| NetworkNode {
            id: input.ids[v].clone(),
            label: input.labels[v].clone(),
            degree: degree[v].0,
            in_degree: directed.then_some(degree[v].1),
            #[allow(clippy::cast_precision_loss)]
            out_degree: directed.then_some(out_adj[v].len() as f64),
            strength: degree[v].2,
            betweenness: bc[v],
            closeness: closeness[v],
            pagerank: pr[v],
            component: component[v],
            community: community[v],
        })
        .collect();
    let edges = merged
        .iter()
        .map(|(&(a, b), &w)| NetworkEdge {
            from: input.ids[a].clone(),
            to: input.ids[b].clone(),
            weight: input.weighted.then_some(w),
        })
        .collect();
    let mut spec = NetworkSpec {
        title: None,
        path: request.path.clone(),
        directed,
        weighted: input.weighted,
        nodes,
        edges,
        components,
        strong_components,
        communities,
        modularity,
        betweenness_sources: sampled,
        seed,
        size_by: request.size_by,
        metrics: Computed::new(
            "nw_pending",
            KIND,
            "Network metrics",
            sci::engine(GRAPH_ENGINE_NAME, BACKEND),
        ),
        warnings,
    };
    spec.metrics = envelope(request, &spec, iterations);
    Ok(spec)
}

/// Labels renumbered 0, 1, … by group size (largest first, ties by the
/// first member).
fn renumber(labels: &[usize]) -> Vec<usize> {
    let mut groups: BTreeMap<usize, (usize, usize)> = BTreeMap::new();
    for (v, &l) in labels.iter().enumerate() {
        let entry = groups.entry(l).or_insert((0, v));
        entry.0 += 1;
    }
    let mut order: Vec<(usize, usize, usize)> = groups
        .into_iter()
        .map(|(l, (size, first))| (l, size, first))
        .collect();
    order.sort_by(|a, b| b.1.cmp(&a.1).then(a.2.cmp(&b.2)));
    let new: HashMap<usize, usize> = order.iter().enumerate().map(|(i, g)| (g.0, i)).collect();
    labels.iter().map(|l| new[l]).collect()
}

/// Newman's modularity of `community` (weighted when `weighted`).
fn modularity(
    edges: &BTreeMap<(usize, usize), f64>,
    community: &[usize],
    directed: bool,
    weighted: bool,
) -> f64 {
    let w = |x: f64| if weighted { x } else { 1.0 };
    let m: f64 = edges.values().map(|x| w(*x)).sum();
    if m == 0.0 {
        return 0.0;
    }
    let k = community.iter().copied().max().map_or(0, |c| c + 1);
    let (mut inside, mut out_deg, mut in_deg) = (vec![0.0; k], vec![0.0; k], vec![0.0; k]);
    for (&(a, b), &x) in edges {
        let x = w(x);
        if community[a] == community[b] {
            inside[community[a]] += x;
        }
        out_deg[community[a]] += x;
        in_deg[community[b]] += x;
    }
    (0..k)
        .map(|c| {
            if directed {
                inside[c] / m - out_deg[c] * in_deg[c] / (m * m)
            } else {
                let d = out_deg[c] + in_deg[c];
                inside[c] / m - (d / (2.0 * m)).powi(2)
            }
        })
        .sum()
}

fn envelope(request: &NetworkRequest, spec: &NetworkSpec, iterations: usize) -> Computed {
    let mut computed = Computed::new(
        sci::derive_id(
            "nw",
            &[
                KIND.as_bytes(),
                &[u8::from(spec.directed)],
                &spec.seed.to_le_bytes(),
                &request.bytes,
            ],
        ),
        KIND,
        "Network metrics",
        sci::engine(GRAPH_ENGINE_NAME, BACKEND),
    );
    computed.method.why = Some(format!(
        "Degree; betweenness by Brandes (hop counts, normalised 1/((n−1)(n−2)){}); closeness on \
         hop counts{} with Wasserman–Faust scaling; PageRank by power iteration (damping 0.85, \
         {iterations} iterations{}); communities by asynchronous label propagation (seed {}) \
         with Newman modularity; components by union-find{}",
        if spec.betweenness_sources.is_some() {
            ", sampled sources"
        } else {
            ", exact"
        },
        if spec.directed {
            " towards the node"
        } else {
            ""
        },
        if spec.weighted { ", weighted" } else { "" },
        spec.seed,
        if spec.directed {
            " (weak) and Kosaraju (strong)"
        } else {
            ""
        }
    ));
    computed.seed = Some(spec.seed);
    computed.rng = Some("splitmix64".into());
    let mut params = BTreeMap::new();
    params.insert("directed".into(), Value::Bool(spec.directed));
    params.insert("weighted".into(), Value::Bool(spec.weighted));
    if let Some(k) = spec.betweenness_sources {
        params.insert("betweenness_sources".into(), Value::from(k));
    }
    computed.method.params = params;
    computed.inputs = vec![chem::input(
        "edges",
        "data",
        &request.bytes,
        request.path.as_deref(),
    )];
    let v = &mut computed.values;
    #[allow(clippy::cast_precision_loss)]
    for (key, x) in [
        ("nodes", spec.nodes.len() as f64),
        ("edges", spec.edges.len() as f64),
        ("density", spec.density()),
        ("components", spec.components as f64),
        ("communities", spec.communities as f64),
        ("modularity", spec.modularity),
        (
            "average_degree",
            spec.nodes.iter().map(|n| n.degree).sum::<f64>() / spec.nodes.len().max(1) as f64,
        ),
    ] {
        v.insert(key.into(), ComputedValue::new(x));
    }
    if let Some(s) = spec.strong_components {
        #[allow(clippy::cast_precision_loss)]
        v.insert("strong_components".into(), ComputedValue::new(s as f64));
    }
    let mut used = std::collections::HashSet::new();
    for (ix, node) in spec.nodes.iter().enumerate().take(ENVELOPE_NODES) {
        let mut key = value_key(&node.id);
        if !used.insert(key.clone()) {
            key = format!("{key}_{}", ix + 1);
            used.insert(key.clone());
        }
        for (name, x) in [
            ("degree", node.degree),
            ("betweenness", node.betweenness),
            ("closeness", node.closeness),
            ("pagerank", node.pagerank),
        ] {
            v.insert(format!("{key}.{name}"), ComputedValue::new(x));
        }
    }
    computed
}
