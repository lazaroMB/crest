//! A reduced Graphviz DOT reader into a [`DiagramSpec`].
//!
//! Our own parser (Graphviz is EPL, and `layout-rs` keeps its parser
//! behind its renderer). It reads the DOT grammar's statements:
//! `[strict] (graph|digraph) [id] { ... }` with node statements, edge
//! chains (`a -> b -> c`, `a -> {b c}`), `graph` / `node` / `edge`
//! defaults, `id = value` graph attributes and subgraphs. Identifiers may
//! be bare, numerals, quoted strings (with `\"` escapes and `+`
//! concatenation) or HTML strings (`<...>`, tags dropped); ports
//! (`a:p:n`) are dropped. Comments (`//`, `/* */`, `#` lines) are skipped.
//!
//! What is drawn: `label` (newlines folded to spaces), node `color` (or
//! `fillcolor`) as the node kind, so nodes of one color share a palette
//! color, edge `label` and `weight` / `penwidth`, `rankdir` (`LR`/`RL` →
//! left to right) and clusters (`subgraph cluster_x { label = ... }`) as
//! groups, a node in nested clusters belonging to the innermost. Other
//! attributes are kept on the node (shown on focus). Positions, shapes,
//! styles and record fields are not drawn.

use std::collections::HashMap;

use domain::devops::diagram::{DiagramSource, DiagramSpec, MAX_WARNINGS};
use domain::widget::graph::{
    FlowDirection, GraphEdge, GraphGroup, GraphNode, GraphSpec, LayoutKind, MAX_EDGES, MAX_NODES,
};
use serde_json::{Map, Value};

/// Deepest subgraph nesting read.
const MAX_NESTING: usize = 64;

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Id(String),
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Equals,
    Semi,
    Comma,
    Colon,
    Edge,
}

fn lex(text: &str) -> Result<Vec<Token>, String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    let mut line_start = true;
    while i < chars.len() {
        let c = chars[i];
        if c == '\n' {
            line_start = true;
            i += 1;
            continue;
        }
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        // `#` lines are C preprocessor output.
        if c == '#' && line_start {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        line_start = false;
        match c {
            '/' if chars.get(i + 1) == Some(&'/') => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            '/' if chars.get(i + 1) == Some(&'*') => {
                i += 2;
                while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                    i += 1;
                }
                i += 2;
            }
            '{' | '}' | '[' | ']' | '=' | ';' | ',' | ':' => {
                out.push(match c {
                    '{' => Token::LBrace,
                    '}' => Token::RBrace,
                    '[' => Token::LBracket,
                    ']' => Token::RBracket,
                    '=' => Token::Equals,
                    ';' => Token::Semi,
                    ',' => Token::Comma,
                    _ => Token::Colon,
                });
                i += 1;
            }
            '-' if matches!(chars.get(i + 1), Some('>' | '-')) => {
                out.push(Token::Edge);
                i += 2;
            }
            '"' => {
                let (mut s, mut next) = lex_string(&chars, i)?;
                // "a" + "b" concatenates.
                loop {
                    let mut j = next;
                    while chars.get(j).is_some_and(|c| c.is_whitespace()) {
                        j += 1;
                    }
                    if chars.get(j) != Some(&'+') {
                        break;
                    }
                    j += 1;
                    while chars.get(j).is_some_and(|c| c.is_whitespace()) {
                        j += 1;
                    }
                    if chars.get(j) != Some(&'"') {
                        break;
                    }
                    let (more, after) = lex_string(&chars, j)?;
                    s.push_str(&more);
                    next = after;
                }
                out.push(Token::Id(s));
                i = next;
            }
            '<' => {
                let mut depth = 0_usize;
                let mut s = String::new();
                loop {
                    match chars.get(i) {
                        None => return Err("an HTML label is not closed".into()),
                        Some('<') => {
                            depth += 1;
                            s.push('<');
                        }
                        Some('>') => {
                            depth -= 1;
                            s.push('>');
                            if depth == 0 {
                                i += 1;
                                break;
                            }
                        }
                        Some(&ch) => s.push(ch),
                    }
                    i += 1;
                }
                out.push(Token::Id(strip_html(&s)));
            }
            c if c.is_alphanumeric() || c == '_' || c == '.' || c == '-' || !c.is_ascii() => {
                let start = i;
                while i < chars.len()
                    && (chars[i].is_alphanumeric()
                        || chars[i] == '_'
                        || chars[i] == '.'
                        || !chars[i].is_ascii()
                        || (chars[i] == '-'
                            && i == start
                            && chars
                                .get(i + 1)
                                .is_some_and(|n| n.is_ascii_digit() || *n == '.')))
                {
                    i += 1;
                }
                if i == start {
                    return Err(format!("unexpected character {c:?}"));
                }
                out.push(Token::Id(chars[start..i].iter().collect()));
            }
            other => return Err(format!("unexpected character {other:?}")),
        }
    }
    Ok(out)
}

/// The quoted string starting at `chars[start]` (a `"`): its text and
/// the index after the closing quote. `\"` is a quote and a backslash
/// before a newline continues the line; other escapes stay as written.
fn lex_string(chars: &[char], start: usize) -> Result<(String, usize), String> {
    let mut s = String::new();
    let mut i = start + 1;
    loop {
        match chars.get(i) {
            None => return Err("a quoted string is not closed".into()),
            Some('"') => return Ok((s, i + 1)),
            Some('\\') if chars.get(i + 1) == Some(&'"') => {
                s.push('"');
                i += 2;
            }
            Some('\\') if chars.get(i + 1) == Some(&'\n') => i += 2,
            Some(&ch) => {
                s.push(ch);
                i += 1;
            }
        }
    }
}

/// The text of an HTML label: tags dropped, entities for `<`, `>`, `&`.
fn strip_html(s: &str) -> String {
    let inner = s
        .strip_prefix('<')
        .and_then(|s| s.strip_suffix('>'))
        .unwrap_or(s);
    let mut out = String::new();
    let mut in_tag = false;
    for c in inner.chars() {
        match c {
            '<' => {
                in_tag = true;
                out.push(' ');
            }
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    let out = out
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
        .replace("&nbsp;", " ");
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A label for one line: `\n`, `\l`, `\r` escapes and newlines become
/// spaces.
fn one_line(label: &str) -> String {
    let label = label
        .replace("\\n", " ")
        .replace("\\l", " ")
        .replace("\\r", " ");
    label.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[derive(Debug, Clone, Default)]
struct Scope {
    node: Map<String, Value>,
    edge: Map<String, Value>,
    cluster: Option<String>,
}

struct Parser {
    tokens: Vec<Token>,
    at: usize,
    directed: bool,
    nodes: Vec<GraphNode>,
    index: HashMap<String, usize>,
    /// Attributes given to each node, for `label` / `color`.
    attrs: Vec<Map<String, Value>>,
    edges: Vec<GraphEdge>,
    groups: Vec<GraphGroup>,
    graph_attrs: Map<String, Value>,
    warnings: Vec<String>,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at)
    }

    fn next(&mut self) -> Option<Token> {
        let t = self.tokens.get(self.at).cloned();
        self.at += 1;
        t
    }

    fn eat(&mut self, token: &Token) -> bool {
        if self.peek() == Some(token) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    fn id(&mut self) -> Result<String, String> {
        match self.next() {
            Some(Token::Id(s)) => Ok(s),
            other => Err(format!(
                "expected a name, found {}",
                describe(other.as_ref())
            )),
        }
    }

    fn warn(&mut self, message: String) {
        if self.warnings.len() < MAX_WARNINGS && !self.warnings.contains(&message) {
            self.warnings.push(message);
        }
    }

    /// `[a=b, c=d][e=f]`.
    fn attr_list(&mut self) -> Result<Map<String, Value>, String> {
        let mut out = Map::new();
        while self.eat(&Token::LBracket) {
            loop {
                match self.peek() {
                    Some(Token::RBracket) => {
                        self.at += 1;
                        break;
                    }
                    Some(Token::Comma | Token::Semi) => self.at += 1,
                    Some(Token::Id(_)) => {
                        let key = self.id()?;
                        let value = if self.eat(&Token::Equals) {
                            self.id()?
                        } else {
                            "true".to_owned()
                        };
                        out.insert(key, Value::String(value));
                    }
                    other => {
                        return Err(format!("expected an attribute, found {}", describe(other)));
                    }
                }
            }
        }
        Ok(out)
    }

    fn node(&mut self, id: &str, scope: &Scope, attrs: &Map<String, Value>) -> usize {
        if let Some(&ix) = self.index.get(id) {
            if self.nodes[ix].group.is_none() {
                self.nodes[ix].group.clone_from(&scope.cluster);
            }
            for (k, v) in attrs {
                self.attrs[ix].insert(k.clone(), v.clone());
            }
            return ix;
        }
        let ix = self.nodes.len();
        let mut all = scope.node.clone();
        for (k, v) in attrs {
            all.insert(k.clone(), v.clone());
        }
        self.nodes.push(GraphNode {
            id: id.to_owned(),
            label: None,
            group: scope.cluster.clone(),
            kind: None,
            attrs: Map::new(),
        });
        self.attrs.push(all);
        self.index.insert(id.to_owned(), ix);
        ix
    }

    /// A node id with its port dropped.
    fn node_id(&mut self) -> Result<String, String> {
        let id = self.id()?;
        if self.eat(&Token::Colon) {
            self.id()?;
            if self.eat(&Token::Colon) {
                self.id()?;
            }
        }
        Ok(id)
    }

    fn stmt_list(&mut self, scope: &mut Scope, depth: usize) -> Result<Vec<usize>, String> {
        let mut members = Vec::new();
        loop {
            match self.peek() {
                None => return Err("the graph is not closed with `}`".into()),
                Some(Token::RBrace) => {
                    self.at += 1;
                    return Ok(members);
                }
                Some(Token::Semi | Token::Comma) => self.at += 1,
                _ => members.extend(self.stmt(scope, depth)?),
            }
        }
    }

    /// One statement; the nodes it names.
    fn stmt(&mut self, scope: &mut Scope, depth: usize) -> Result<Vec<usize>, String> {
        if let Some(Token::Id(word)) = self.peek().cloned() {
            let lower = word.to_ascii_lowercase();
            let next = self.tokens.get(self.at + 1);
            if matches!(lower.as_str(), "graph" | "node" | "edge") && next == Some(&Token::LBracket)
            {
                self.at += 1;
                let attrs = self.attr_list()?;
                match lower.as_str() {
                    "node" => scope.node.extend(attrs),
                    "edge" => scope.edge.extend(attrs),
                    _ => self.graph_attr(scope, attrs),
                }
                return Ok(Vec::new());
            }
            if next == Some(&Token::Equals) && lower != "subgraph" {
                self.at += 2;
                let value = self.id()?;
                let mut attrs = Map::new();
                attrs.insert(word, Value::String(value));
                self.graph_attr(scope, attrs);
                return Ok(Vec::new());
            }
        }
        let first = self.endpoint(scope, depth)?;
        if self.peek() != Some(&Token::Edge) {
            // A node statement (or a lone subgraph).
            if first.1 {
                let attrs = self.attr_list()?;
                if let Some(&ix) = first.0.first() {
                    let id = self.nodes[ix].id.clone();
                    self.node(&id, scope, &attrs);
                }
            }
            return Ok(first.0);
        }
        let mut chain = vec![first.0];
        while self.eat(&Token::Edge) {
            chain.push(self.endpoint(scope, depth)?.0);
        }
        let mut attrs = scope.edge.clone();
        attrs.extend(self.attr_list()?);
        let label = attrs
            .get("label")
            .and_then(Value::as_str)
            .map(one_line)
            .filter(|l| !l.is_empty());
        let weight = ["penwidth", "weight"]
            .iter()
            .find_map(|k| attrs.get(*k).and_then(Value::as_str)?.parse::<f64>().ok())
            .filter(|w| w.is_finite() && *w > 0.0);
        let mut named = Vec::new();
        for pair in chain.windows(2) {
            for &a in &pair[0] {
                for &b in &pair[1] {
                    if self.edges.len() >= MAX_EDGES {
                        return Err(format!("more than {MAX_EDGES} edges"));
                    }
                    self.edges.push(GraphEdge {
                        from: self.nodes[a].id.clone(),
                        to: self.nodes[b].id.clone(),
                        label: label.clone(),
                        weight,
                    });
                }
            }
        }
        for ends in chain {
            named.extend(ends);
        }
        Ok(named)
    }

    fn graph_attr(&mut self, scope: &Scope, attrs: Map<String, Value>) {
        match &scope.cluster {
            // A cluster's label captions its group.
            Some(cluster) if attrs.contains_key("label") => {
                if let Some(label) = attrs.get("label").and_then(Value::as_str)
                    && let Some(group) = self.groups.iter_mut().find(|g| &g.id == cluster)
                {
                    group.label = Some(one_line(label)).filter(|l| !l.is_empty());
                }
            }
            Some(_) => {}
            None => self.graph_attrs.extend(attrs),
        }
    }

    /// A node id or a subgraph: the nodes it stands for, and whether it
    /// was a single node.
    fn endpoint(&mut self, scope: &mut Scope, depth: usize) -> Result<(Vec<usize>, bool), String> {
        let is_subgraph = match self.peek() {
            Some(Token::LBrace) => true,
            Some(Token::Id(w)) => w.eq_ignore_ascii_case("subgraph"),
            _ => false,
        };
        if !is_subgraph {
            if self.nodes.len() >= MAX_NODES {
                return Err(format!("more than {MAX_NODES} nodes"));
            }
            let id = self.node_id()?;
            let ix = self.node(&id, scope, &Map::new());
            return Ok((vec![ix], true));
        }
        if depth >= MAX_NESTING {
            return Err(format!("subgraphs nested deeper than {MAX_NESTING}"));
        }
        let mut name = None;
        if let Some(Token::Id(w)) = self.peek()
            && w.eq_ignore_ascii_case("subgraph")
        {
            self.at += 1;
            if let Some(Token::Id(_)) = self.peek() {
                name = Some(self.id()?);
            }
        }
        if !self.eat(&Token::LBrace) {
            return Err(format!(
                "expected `{{` after subgraph, found {}",
                describe(self.peek())
            ));
        }
        let mut inner = scope.clone();
        if let Some(name) = name.as_deref().filter(|n| n.starts_with("cluster")) {
            let id = name.to_owned();
            if !self.groups.iter().any(|g| g.id == id) {
                self.groups.push(GraphGroup {
                    id: id.clone(),
                    label: None,
                });
            }
            inner.cluster = Some(id);
        }
        let members = self.stmt_list(&mut inner, depth + 1)?;
        Ok((members, false))
    }
}

fn describe(token: Option<&Token>) -> String {
    match token {
        None => "the end".into(),
        Some(Token::Id(s)) => format!("{s:?}"),
        Some(t) => format!("{t:?}"),
    }
}

/// Whether the first word (after comments) opens a DOT graph.
fn starts_like_dot(text: &str) -> bool {
    let word = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("//") && !l.starts_with('#'))
        .find_map(|l| l.split_whitespace().next())
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(word.as_str(), "strict" | "graph" | "digraph" | "/*")
        || word.starts_with("digraph")
        || word.starts_with("graph")
}

/// Parses the first graph of a DOT file.
///
/// # Errors
///
/// A message for text that is not a DOT graph or breaks the grammar.
pub fn parse(text: &str, path: Option<&str>) -> Result<DiagramSpec, String> {
    let name = path.unwrap_or("the DOT source");
    let tokens = match lex(text) {
        Ok(tokens) => tokens,
        Err(e) if starts_like_dot(text) => return Err(format!("`{name}`: {e}")),
        Err(_) => {
            return Err(format!(
                "`{name}` is not a DOT graph: it must start with `digraph` or `graph`"
            ));
        }
    };
    let mut p = Parser {
        tokens,
        at: 0,
        directed: true,
        nodes: Vec::new(),
        index: HashMap::new(),
        attrs: Vec::new(),
        edges: Vec::new(),
        groups: Vec::new(),
        graph_attrs: Map::new(),
        warnings: Vec::new(),
    };
    if let Some(Token::Id(w)) = p.peek()
        && w.eq_ignore_ascii_case("strict")
    {
        p.at += 1;
    }
    p.directed = match p.next() {
        Some(Token::Id(w)) if w.eq_ignore_ascii_case("digraph") => true,
        Some(Token::Id(w)) if w.eq_ignore_ascii_case("graph") => false,
        other => {
            return Err(format!(
                "`{name}` is not a DOT graph: expected `digraph` or `graph`, found {}",
                describe(other.as_ref())
            ));
        }
    };
    let graph_name = match p.peek() {
        Some(Token::Id(_)) => Some(p.id()?),
        _ => None,
    };
    if !p.eat(&Token::LBrace) {
        return Err(format!(
            "`{name}`: expected `{{`, found {}",
            describe(p.peek())
        ));
    }
    let mut scope = Scope::default();
    p.stmt_list(&mut scope, 0)
        .map_err(|e| format!("`{name}`: {e}"))?;
    if p.peek().is_some() {
        p.warn("only the first graph of the file is drawn".into());
    }
    // Labels and kinds from the attributes each node ended with.
    let mut unsupported = Vec::new();
    for (node, attrs) in p.nodes.iter_mut().zip(&p.attrs) {
        node.label = attrs
            .get("label")
            .and_then(Value::as_str)
            .map(one_line)
            .filter(|l| !l.is_empty() && l != "\\N" && *l != node.id);
        node.kind = ["color", "fillcolor"]
            .iter()
            .find_map(|k| attrs.get(*k).and_then(Value::as_str))
            .map(str::to_owned);
        for (k, v) in attrs {
            match k.as_str() {
                "label" | "color" | "fillcolor" => {}
                "pos" | "width" | "height" | "fixedsize" => {
                    if !unsupported.contains(k) {
                        unsupported.push(k.clone());
                    }
                }
                _ => {
                    node.attrs.insert(k.clone(), v.clone());
                }
            }
        }
    }
    if !unsupported.is_empty() {
        p.warn(format!(
            "node attributes not drawn: {} (the layout is computed)",
            unsupported.join(", ")
        ));
    }
    let attr = |k: &str| p.graph_attrs.get(k).and_then(Value::as_str);
    let direction = match attr("rankdir").map(str::to_ascii_uppercase).as_deref() {
        Some("LR" | "RL") => FlowDirection::Right,
        _ => FlowDirection::Down,
    };
    let layout = match attr("layout") {
        Some("neato" | "fdp" | "sfdp" | "circo" | "twopi") => LayoutKind::Force,
        _ => LayoutKind::Layered,
    };
    let title = attr("label")
        .map(one_line)
        .filter(|l| !l.is_empty())
        .or(graph_name.filter(|n| !n.is_empty()));
    let graph = GraphSpec {
        title: title.clone(),
        directed: p.directed,
        layout,
        direction,
        seed: None,
        nodes: p.nodes,
        edges: p.edges,
        groups: p.groups,
    };
    Ok(DiagramSpec {
        title: None,
        path: path.map(str::to_owned),
        source: DiagramSource::Dot,
        dialect: None,
        graph,
        tables: Vec::new(),
        warnings: p.warnings,
    })
}
