//! Mermaid `usecase-beta` (Mermaid 11.12) translated to a flowchart,
//! because rusty-mermaid does not draw it. The doc keeps the real
//! `usecase-beta` source; only the drawing goes through the flowchart.
//!
//! Supported: `direction LR|RL|TB|TD|BT`; `actor Id`, `actor Id("Label")`,
//! `actor Id(Label)`; use cases `Id("Label")`, `Id(Label)` or a bare `Id`;
//! `systemBoundary "Name"` (or `Id["Name"]`) ... `end` as a subgraph;
//! relations `-->`, `->`, `-.->`, with a label as `A -->|uses| B`,
//! `A --> B : uses` or `A --includes--> B` (also `A -- uses --> B`). Nodes
//! used in a relation without a declaration are use cases labelled with
//! their id. `title` and `%%` comments are ignored. Anything else makes
//! [`to_flowchart`] return `None` and the renderer reports its own error.
//! Actors draw as circles, use cases as stadiums.

use std::collections::HashMap;

/// A declared node: its label and whether it is an actor.
struct Node {
    label: String,
    actor: bool,
}

/// The flowchart of a `usecase` source, `None` when the source is not one
/// or has a line this translator does not know.
#[must_use]
pub fn to_flowchart(source: &str) -> Option<String> {
    let mut lines = source
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with("%%"));
    let header = lines.next()?;
    if !(header == "usecase" || header == "usecase-beta") {
        return None;
    }
    let mut direction = "TB";
    let mut order: Vec<String> = Vec::new();
    let mut nodes: HashMap<String, Node> = HashMap::new();
    // Output lines after the header, nodes declared where they first show.
    let mut body: Vec<String> = Vec::new();
    let mut open = 0usize;
    let mut boundaries = 0usize;
    for line in lines {
        let (word, rest) = split_word(line);
        match word {
            "direction" => {
                direction = match rest.trim() {
                    "LR" => "LR",
                    "RL" => "RL",
                    "TB" | "TD" => "TB",
                    "BT" => "BT",
                    _ => return None,
                };
            }
            "title" | "accTitle" | "accDescr" => {}
            "actor" => {
                for part in split_top(rest, ',') {
                    let (id, label) = node_ref(part)?;
                    declare(&mut nodes, &mut order, &id, label, true);
                }
            }
            "systemBoundary" => {
                boundaries += 1;
                let name = rest.trim();
                let (id, label) = match node_ref(name) {
                    Some((id, Some(label))) if !name.starts_with('"') => (id, label),
                    _ => (format!("boundary{boundaries}"), unquote(name)),
                };
                if label.is_empty() {
                    return None;
                }
                body.push(format!("subgraph {id}[{}]", clean(&label)));
                open += 1;
            }
            "end" if rest.trim().is_empty() => {
                open = open.checked_sub(1)?;
                body.push("end".to_owned());
            }
            _ => match relation(line) {
                Some((from, to, label, dashed)) => {
                    let mut ends = [from, to];
                    for end in &mut ends {
                        let (id, lbl) = node_ref(end)?;
                        declare(&mut nodes, &mut order, &id, lbl, false);
                        *end = id;
                    }
                    let dash = dashed || label.as_deref().is_some_and(is_keyword);
                    let arrow = match (dash, label) {
                        (true, Some(l)) => format!("-.->|{}|", clean(&l)),
                        (true, None) => "-.->".to_owned(),
                        (false, Some(l)) => format!("-->|{}|", clean(&l)),
                        (false, None) => "-->".to_owned(),
                    };
                    body.push(format!("{} {arrow} {}", ends[0], ends[1]));
                }
                // A use case on its own line: `Id("Label")`.
                None => {
                    let (id, label) = node_ref(line)?;
                    declare(&mut nodes, &mut order, &id, label, false);
                    // Declared in place so a boundary holds it.
                    body.push(node_decl(&id, &nodes[&id]));
                }
            },
        }
    }
    if open != 0 {
        return None;
    }
    let mut out = format!("flowchart {direction}\n");
    // Actors outside any boundary first, then the rest as written; a node
    // never declared in place is declared up front.
    let placed: Vec<&String> = body
        .iter()
        .filter(|l| {
            !l.contains("-->") && !l.contains("-.->") && !l.starts_with("subgraph") && *l != "end"
        })
        .collect();
    for id in &order {
        let decl = node_decl(id, &nodes[id]);
        if !placed.contains(&&decl) {
            out.push_str("  ");
            out.push_str(&decl);
            out.push('\n');
        }
    }
    for line in body {
        out.push_str("  ");
        out.push_str(&line);
        out.push('\n');
    }
    Some(out)
}

/// Remembers a node; a later label wins, and an actor stays an actor.
fn declare(
    nodes: &mut HashMap<String, Node>,
    order: &mut Vec<String>,
    id: &str,
    label: Option<String>,
    actor: bool,
) {
    match nodes.get_mut(id) {
        Some(node) => {
            node.actor |= actor;
            if let Some(label) = label {
                node.label = label;
            }
        }
        None => {
            order.push(id.to_owned());
            nodes.insert(
                id.to_owned(),
                Node {
                    label: label.unwrap_or_else(|| id.to_owned()),
                    actor,
                },
            );
        }
    }
}

fn node_decl(id: &str, node: &Node) -> String {
    let label = clean(&node.label);
    if node.actor {
        format!("{id}(({label}))")
    } else {
        format!("{id}([{label}])")
    }
}

/// Labels go bare (the renderer keeps quotes): no characters of the flowchart
/// syntax or line breaks inside.
fn clean(label: &str) -> String {
    label
        .replace(['"', '(', ')', '[', ']', '{', '}', '|', '\n', '\r'], " ")
        .trim()
        .to_owned()
}

fn is_keyword(label: &str) -> bool {
    matches!(label.trim(), "include" | "extend" | "includes" | "extends")
}

fn split_word(line: &str) -> (&str, &str) {
    match line.find(char::is_whitespace) {
        Some(at) => (&line[..at], line[at..].trim_start()),
        None => (line, ""),
    }
}

/// Splits at `sep` outside quotes and parentheses.
fn split_top(text: &str, sep: char) -> Vec<&str> {
    let (mut parts, mut start, mut depth, mut quoted) = (Vec::new(), 0, 0i32, false);
    for (at, ch) in text.char_indices() {
        match ch {
            '"' => quoted = !quoted,
            '(' | '[' if !quoted => depth += 1,
            ')' | ']' if !quoted => depth -= 1,
            c if c == sep && !quoted && depth == 0 => {
                parts.push(&text[start..at]);
                start = at + c.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(&text[start..]);
    parts
}

fn unquote(text: &str) -> String {
    text.trim().trim_matches('"').trim().to_owned()
}

/// `Id`, `Id(Label)`, `Id("Label")` or `Id["Label"]`: the id and the label.
fn node_ref(text: &str) -> Option<(String, Option<String>)> {
    let text = text.trim();
    let end = text
        .find(|c: char| !(c.is_alphanumeric() || c == '_'))
        .unwrap_or(text.len());
    let id = &text[..end];
    if id.is_empty() {
        return None;
    }
    let rest = text[end..].trim();
    if rest.is_empty() {
        return Some((id.to_owned(), None));
    }
    let inner = rest
        .strip_prefix('(')
        .and_then(|r| r.strip_suffix(')'))
        .or_else(|| rest.strip_prefix('[').and_then(|r| r.strip_suffix(']')))?;
    let label = unquote(inner);
    Some((id.to_owned(), Some(label).filter(|l| !l.is_empty())))
}

/// `(from, to, label, dashed)` of a relation line, `None` when the line has
/// no arrow.
fn relation(line: &str) -> Option<(String, String, Option<String>, bool)> {
    // The trailing `: label` of the line, outside quotes.
    let (line, tail) = match split_top(line, ':').as_slice() {
        [only] => (*only, None),
        [head, label] => (*head, Some(label.trim().to_owned())),
        _ => return None,
    };
    let at = line.find("->")?;
    let (left, right) = (&line[..at], &line[at + 2..]);
    let dashed = left.ends_with("-.");
    let left = left.trim_end_matches(['-', '.']).trim_end();
    // `A --text` or `A -- text`: the text between the tail and the arrow.
    let (from, inner) = match left.find(" --").or_else(|| left.find(" -.")) {
        Some(split) => (
            &left[..split],
            Some(
                left[split..]
                    .trim_start_matches([' ', '-', '.'])
                    .trim()
                    .to_owned(),
            ),
        ),
        None => (left, None),
    };
    let right = right.trim();
    let (piped, to) = match right.strip_prefix('|') {
        Some(after) => {
            let close = after.find('|')?;
            (
                Some(after[..close].trim().to_owned()),
                after[close + 1..].trim(),
            )
        }
        None => (None, right),
    };
    let label = [piped, inner, tail]
        .into_iter()
        .flatten()
        .find(|l| !l.is_empty());
    if from.trim().is_empty() || to.is_empty() {
        return None;
    }
    Some((from.trim().to_owned(), to.to_owned(), label, dashed))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEMPLATE: &str = "usecase-beta\ndirection LR\nactor Customer(\"Customer\")\nsystemBoundary \"Order system\"\n  Checkout(\"Place order\")\nend\nCustomer --> Checkout";

    #[test]
    fn translates_the_template() {
        let out = to_flowchart(TEMPLATE).unwrap();
        assert_eq!(
            out,
            "flowchart LR\n  Customer((Customer))\n  subgraph boundary1[Order system]\n  Checkout([Place order])\n  end\n  Customer --> Checkout\n"
        );
    }

    #[test]
    fn only_usecase_sources_are_translated() {
        assert!(to_flowchart("flowchart LR\n A --> B").is_none());
        assert!(to_flowchart("").is_none());
    }

    #[test]
    fn labels_come_from_every_syntax() {
        let src = "usecase\n%% note\nactor A(Admin)\nactor B\nUC1(Log in)\nA -->|uses| UC1\nB --> UC1 : sees\nA --includes--> UC2\nB -.-> UC1\nB -- plain --> UC2\nUC1 -> UC2";
        let out = to_flowchart(src).unwrap();
        for want in [
            "flowchart TB",
            "A((Admin))",
            "B((B))",
            "UC1([Log in])",
            "UC2([UC2])",
            "A -->|uses| UC1",
            "B -->|sees| UC1",
            "A -.->|includes| UC2",
            "B -.-> UC1",
            "B -->|plain| UC2",
            "UC1 --> UC2",
        ] {
            assert!(out.contains(want), "{want} missing in\n{out}");
        }
    }

    #[test]
    fn unknown_lines_and_unbalanced_boundaries_are_refused() {
        assert!(to_flowchart("usecase-beta\n???").is_none());
        assert!(to_flowchart("usecase-beta\nsystemBoundary \"X\"\nA --> B").is_none());
        assert!(to_flowchart("usecase-beta\nend").is_none());
        assert!(to_flowchart("usecase-beta\ndirection XX").is_none());
    }

    #[test]
    fn the_templates_draw_their_labels() {
        use crate::blocks::mermaid::diagram::Shape;
        for (name, labels) in [
            ("Use case", &["Customer", "Order system", "Place order"][..]),
            ("Bar chart", &["Files changed by crate", "ui", "docs"][..]),
        ] {
            let source = domain::doc::DIAGRAM_TEMPLATES
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, s)| *s)
                .unwrap();
            let diagram = crate::blocks::mermaid::diagram::parse(source).unwrap();
            let text: Vec<String> = diagram
                .shapes
                .iter()
                .filter_map(|s| match s {
                    Shape::Text(l) => Some(l.lines.join(" ")),
                    Shape::Path { .. } => None,
                })
                .collect();
            for label in labels {
                assert!(
                    text.iter().any(|t| t == label),
                    "{name}: {label} in {text:?}"
                );
            }
        }
    }
    #[test]
    fn every_template_renders_through_the_doc_page_path() {
        for (name, source) in domain::doc::DIAGRAM_TEMPLATES {
            let diagram = crate::blocks::mermaid::diagram::parse(source)
                .unwrap_or_else(|err| panic!("{name}: {err}"));
            assert!(!diagram.shapes.is_empty(), "{name} draws nothing");
        }
    }
    #[test]
    fn variants_render_through_rusty_mermaid() {
        let theme = rusty_mermaid_core::Theme::default();
        for src in [
            TEMPLATE,
            "usecase-beta\nactor A\nactor B\nsystemBoundary Shop[\"Shop\"]\n  Buy(Buy)\n  Pay(Pay)\nend\nA --> Buy\nBuy --include--> Pay\nB -.-> Pay : audits",
        ] {
            let flow = to_flowchart(src).unwrap();
            let scene =
                rusty_mermaid::render(&flow, &theme).unwrap_or_else(|e| panic!("{e}\n{flow}"));
            assert!(!scene.elements().is_empty());
        }
    }
}
