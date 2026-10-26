//! Netlists as connectivity: SPICE decks (own reader) and KiCad `.net`
//! exports (own s-expression reader).
//!
//! SPICE: the first line is the title, as in every SPICE (unless it reads
//! as an element with a reference such as `R1`, which is then kept with a
//! warning); `*` lines and `;`
//! tails are comments; `+` lines continue the previous one. Elements are
//! read by their first letter, which fixes their node count (`R`, `C`,
//! `L`, `D`, `V`, `I`, `B`: 2; `Q`, `J`, `Z`: 3; `M`, `E`, `G`, `S`, `T`:
//! 4; `X`: every token before the subcircuit name). `.subckt` blocks are
//! listed with their ports and element counts, not flattened, and an `X`
//! instance names its pins after the definition's ports. `.include` and
//! `.lib` are not followed (a warning says so).
//!
//! KiCad: `(export (design …) (components (comp (ref …) (value …)
//! (footprint …) (libsource (lib …) (part …)))) (nets (net (name …) (node
//! (ref …) (pin …) (pinfunction …)))))`, KiCad 5 to 8, quoted or bare
//! atoms.

use std::collections::HashMap;

use domain::engineering::netlist::{
    Component, MAX_COMPONENTS, NetlistFormat, NetlistSpec, PinNet, Subcircuit,
};

/// Warnings kept.
const MAX_WARNINGS: usize = 50;

fn warn(warnings: &mut Vec<String>, text: String) {
    if warnings.len() < MAX_WARNINGS {
        warnings.push(text);
    }
}

/// Reads `text` as `format`, or guesses it (`(export` is KiCad).
///
/// # Errors
///
/// A message when nothing could be read.
pub fn parse(
    text: &str,
    format: Option<NetlistFormat>,
    path: Option<&str>,
) -> Result<NetlistSpec, String> {
    let format = format.unwrap_or_else(|| {
        if text.trim_start().starts_with("(export") {
            NetlistFormat::Kicad
        } else {
            NetlistFormat::Spice
        }
    });
    let mut spec = match format {
        NetlistFormat::Spice => spice(text)?,
        NetlistFormat::Kicad => kicad(text)?,
    };
    spec.path = path.map(str::to_owned);
    if spec.components.is_empty() {
        return Err("no components found".into());
    }
    if spec.components.len() > MAX_COMPONENTS {
        return Err(format!(
            "{} components; at most {MAX_COMPONENTS}",
            spec.components.len()
        ));
    }
    Ok(spec)
}

// --- SPICE --------------------------------------------------------------

/// Logical lines: comments dropped, continuations joined, with the
/// number of the first physical line.
fn logical_lines(text: &str) -> Vec<(usize, String)> {
    let mut out: Vec<(usize, String)> = Vec::new();
    for (ix, raw) in text.lines().enumerate() {
        let line = raw.split(';').next().unwrap_or_default().trim_end();
        let trimmed = line.trim_start();
        if ix > 0 && trimmed.starts_with('*') {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix('+')
            && let Some(last) = out.last_mut()
        {
            last.1.push(' ');
            last.1.push_str(rest.trim());
            continue;
        }
        if trimmed.is_empty() {
            continue;
        }
        out.push((ix + 1, trimmed.to_owned()));
    }
    out
}

/// Node count, pin names and kind of an element by its letter.
fn element(letter: char) -> Option<(usize, &'static [&'static str], &'static str)> {
    Some(match letter {
        'R' => (2, &["1", "2"], "resistor"),
        'C' => (2, &["1", "2"], "capacitor"),
        'L' => (2, &["1", "2"], "inductor"),
        'D' => (2, &["A", "K"], "diode"),
        'V' => (2, &["+", "-"], "voltage source"),
        'I' => (2, &["+", "-"], "current source"),
        'B' => (2, &["+", "-"], "behavioral source"),
        'F' => (2, &["+", "-"], "current-controlled current source"),
        'H' => (2, &["+", "-"], "current-controlled voltage source"),
        'W' => (2, &["+", "-"], "current-controlled switch"),
        'Q' => (3, &["C", "B", "E"], "bjt"),
        'J' => (3, &["D", "G", "S"], "jfet"),
        'Z' => (3, &["D", "G", "S"], "mesfet"),
        'M' => (4, &["D", "G", "S", "B"], "mosfet"),
        'E' => (
            4,
            &["+", "-", "c+", "c-"],
            "voltage-controlled voltage source",
        ),
        'G' => (
            4,
            &["+", "-", "c+", "c-"],
            "voltage-controlled current source",
        ),
        'S' => (4, &["+", "-", "c+", "c-"], "voltage-controlled switch"),
        'T' => (4, &["1+", "1-", "2+", "2-"], "transmission line"),
        'K' => (0, &[], "coupling"),
        'X' => (0, &[], "subcircuit"),
        _ => return None,
    })
}

/// Whether `token` is a parameter (`w=1u`, `params:`).
fn is_param(token: &str) -> bool {
    token.contains('=') || token.eq_ignore_ascii_case("params:")
}

fn join(tokens: &[&str]) -> Option<String> {
    (!tokens.is_empty()).then(|| tokens.join(" "))
}

/// One element line, with the subcircuit ports known so far.
fn spice_element(
    tokens: &[&str],
    ports: &HashMap<String, Vec<String>>,
) -> Result<Component, String> {
    let reference = tokens[0];
    let letter = reference
        .chars()
        .next()
        .map(|c| c.to_ascii_uppercase())
        .unwrap_or(' ');
    let (count, names, kind) =
        element(letter).ok_or_else(|| format!("unknown element `{reference}`"))?;
    let rest = &tokens[1..];
    let (nodes, model, value): (&[&str], Option<String>, Option<String>) = match letter {
        'X' => {
            let end = rest.iter().position(|t| is_param(t)).unwrap_or(rest.len());
            if end == 0 {
                return Err(format!("`{reference}` names no subcircuit"));
            }
            let name = rest[end - 1];
            (&rest[..end - 1], Some(name.to_owned()), join(&rest[end..]))
        }
        'K' => (&[], None, join(rest)),
        _ => {
            if rest.len() < count {
                return Err(format!("`{reference}` needs {count} nodes"));
            }
            let (nodes, tail) = rest.split_at(count);
            let model_first = matches!(letter, 'D' | 'Q' | 'J' | 'Z' | 'M' | 'S' | 'W');
            let (model, value) = if model_first {
                let (m, v) = tail
                    .split_first()
                    .map_or((None, &[][..]), |(m, v)| (Some(*m), v));
                (m.map(str::to_owned), join(v))
            } else {
                (None, join(tail))
            };
            (nodes, model, value)
        }
    };
    let port_names = match letter {
        'X' => model
            .as_ref()
            .and_then(|m| ports.get(&m.to_ascii_lowercase()))
            .filter(|p| p.len() == nodes.len()),
        _ => None,
    };
    let pins = nodes
        .iter()
        .enumerate()
        .map(|(i, net)| PinNet {
            pin: port_names
                .map(|p| p[i].clone())
                .or_else(|| names.get(i).map(|s| (*s).to_owned()))
                .unwrap_or_else(|| (i + 1).to_string()),
            net: (*net).to_owned(),
            function: None,
        })
        .collect();
    Ok(Component {
        reference: reference.to_owned(),
        kind: kind.to_owned(),
        value,
        model,
        pins,
    })
}

fn spice(text: &str) -> Result<NetlistSpec, String> {
    let lines = logical_lines(text);
    let mut warnings = Vec::new();
    let mut design = None;
    let mut start = 0;
    if let Some((_, first)) = lines.first() {
        let tokens: Vec<&str> = first.split_whitespace().collect();
        // A title rarely starts with a reference such as `R1`.
        let reference = tokens.first().copied().unwrap_or_default();
        let element_like = reference
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
            && reference.chars().any(|c| c.is_ascii_digit())
            && spice_element(&tokens, &HashMap::new()).is_ok();
        if element_like {
            warn(
                &mut warnings,
                "line 1 reads as an element; SPICE would take it as the title".into(),
            );
        } else {
            let title = first.trim_start_matches('*').trim();
            let title = title.strip_prefix(".title").unwrap_or(title).trim();
            design = (!title.is_empty()).then(|| title.to_owned());
            start = 1;
        }
    }
    // Ports of every subcircuit first, so instances before the
    // definition still name their pins.
    let mut ports: HashMap<String, Vec<String>> = HashMap::new();
    for (_, line) in &lines[start..] {
        let tokens: Vec<&str> = line.split_whitespace().collect();
        if tokens.len() >= 2 && tokens[0].eq_ignore_ascii_case(".subckt") {
            let names = tokens[2..]
                .iter()
                .take_while(|t| !is_param(t))
                .map(|t| (*t).to_owned())
                .collect();
            ports.insert(tokens[1].to_ascii_lowercase(), names);
        }
    }
    let mut components: Vec<Component> = Vec::new();
    let mut subcircuits: Vec<Subcircuit> = Vec::new();
    let mut inside: Option<Subcircuit> = None;
    for (number, line) in &lines[start..] {
        let tokens: Vec<&str> = line.split_whitespace().collect();
        let Some(head) = tokens.first() else { continue };
        let lower = head.to_ascii_lowercase();
        if lower.starts_with('.') {
            match lower.as_str() {
                ".subckt" if tokens.len() >= 2 => {
                    inside = Some(Subcircuit {
                        name: tokens[1].to_owned(),
                        ports: ports
                            .get(&tokens[1].to_ascii_lowercase())
                            .cloned()
                            .unwrap_or_default(),
                        elements: 0,
                    });
                }
                ".ends" => {
                    if let Some(s) = inside.take() {
                        subcircuits.push(s);
                    }
                }
                ".end" => break,
                ".include" | ".inc" | ".lib" => warn(
                    &mut warnings,
                    format!("line {number}: `{line}` not followed"),
                ),
                _ => {}
            }
            continue;
        }
        if let Some(s) = inside.as_mut() {
            s.elements += 1;
            continue;
        }
        match spice_element(&tokens, &ports) {
            Ok(c)
                if components
                    .iter()
                    .any(|o| o.reference.eq_ignore_ascii_case(&c.reference)) =>
            {
                warn(
                    &mut warnings,
                    format!(
                        "line {number}: {} defined again; kept the first",
                        c.reference
                    ),
                );
            }
            Ok(c) => components.push(c),
            Err(e) => warn(&mut warnings, format!("line {number}: {e}")),
        }
    }
    if let Some(s) = inside {
        warn(&mut warnings, format!("subcircuit {} has no .ends", s.name));
        subcircuits.push(s);
    }
    Ok(NetlistSpec {
        title: None,
        path: None,
        format: NetlistFormat::Spice,
        design,
        components,
        subcircuits,
        warnings,
    })
}

// --- KiCad --------------------------------------------------------------

/// An s-expression.
#[derive(Debug, Clone, PartialEq)]
enum Sexp {
    Atom(String),
    List(Vec<Sexp>),
}

impl Sexp {
    fn head(&self) -> Option<&str> {
        match self {
            Self::List(items) => match items.first() {
                Some(Self::Atom(a)) => Some(a),
                _ => None,
            },
            Self::Atom(_) => None,
        }
    }

    fn items(&self) -> &[Sexp] {
        match self {
            Self::List(items) => items,
            Self::Atom(_) => &[],
        }
    }

    /// Child lists headed `name`.
    fn children<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Sexp> + 'a {
        self.items().iter().filter(move |c| c.head() == Some(name))
    }

    fn child(&self, name: &str) -> Option<&Sexp> {
        self.items().iter().find(|c| c.head() == Some(name))
    }

    /// The first atom after the head of child `name`.
    fn value(&self, name: &str) -> Option<String> {
        match self.child(name)?.items().get(1)? {
            Self::Atom(a) => Some(a.clone()),
            Self::List(_) => None,
        }
    }
}

/// Most nesting accepted.
const MAX_DEPTH: usize = 64;

fn sexp(text: &str) -> Result<Sexp, String> {
    let mut stack: Vec<Vec<Sexp>> = vec![Vec::new()];
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '(' => {
                if stack.len() > MAX_DEPTH {
                    return Err("nested too deeply".into());
                }
                stack.push(Vec::new());
            }
            ')' => {
                let done = stack.pop().ok_or("unbalanced `)`")?;
                stack
                    .last_mut()
                    .ok_or("unbalanced `)`")?
                    .push(Sexp::List(done));
            }
            '"' => {
                let mut s = String::new();
                loop {
                    match chars.next() {
                        Some('\\') => {
                            if let Some(e) = chars.next() {
                                s.push(match e {
                                    'n' => '\n',
                                    't' => '\t',
                                    other => other,
                                });
                            }
                        }
                        Some('"') => break,
                        Some(other) => s.push(other),
                        None => return Err("unterminated string".into()),
                    }
                }
                stack.last_mut().ok_or("unbalanced")?.push(Sexp::Atom(s));
            }
            c if c.is_whitespace() => {}
            c => {
                let mut s = String::from(c);
                while let Some(&n) = chars.peek() {
                    if n.is_whitespace() || n == '(' || n == ')' || n == '"' {
                        break;
                    }
                    s.push(n);
                    chars.next();
                }
                stack.last_mut().ok_or("unbalanced")?.push(Sexp::Atom(s));
            }
        }
        if stack.is_empty() {
            return Err("unbalanced `)`".into());
        }
    }
    if stack.len() != 1 {
        return Err("unbalanced `(`".into());
    }
    stack
        .pop()
        .and_then(|mut top| top.drain(..).find(|s| s.head() == Some("export")))
        .ok_or_else(|| "no `(export …)` form".into())
}

/// `pin` as a sort key: numbers in numeric order, then names.
fn pin_key(pin: &str) -> (u8, u64, String) {
    match pin.parse::<u64>() {
        Ok(n) => (0, n, String::new()),
        Err(_) => (1, 0, pin.to_owned()),
    }
}

fn kicad(text: &str) -> Result<NetlistSpec, String> {
    let root = sexp(text)?;
    let mut warnings = Vec::new();
    let design = root
        .child("design")
        .and_then(|d| d.value("source"))
        .filter(|s| !s.is_empty());
    let mut components: Vec<Component> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    if let Some(list) = root.child("components") {
        for comp in list.children("comp") {
            let Some(reference) = comp.value("ref") else {
                warn(&mut warnings, "a component has no ref".into());
                continue;
            };
            let kind = comp.child("libsource").map_or_else(
                || "component".to_owned(),
                |l| match (l.value("lib"), l.value("part")) {
                    (Some(lib), Some(part)) => format!("{lib}:{part}"),
                    (None, Some(part)) => part,
                    _ => "component".to_owned(),
                },
            );
            index.insert(reference.clone(), components.len());
            components.push(Component {
                reference,
                kind,
                value: comp.value("value").filter(|v| !v.is_empty() && v != "~"),
                model: comp.value("footprint").filter(|f| !f.is_empty()),
                pins: Vec::new(),
            });
        }
    }
    if let Some(nets) = root.child("nets") {
        for net in nets.children("net") {
            let name = net
                .value("name")
                .or_else(|| net.value("code").map(|c| format!("net {c}")))
                .unwrap_or_default();
            for node in net.children("node") {
                let (Some(reference), Some(pin)) = (node.value("ref"), node.value("pin")) else {
                    continue;
                };
                let ix = match index.get(&reference) {
                    Some(ix) => *ix,
                    None => {
                        warn(
                            &mut warnings,
                            format!("net {name} names {reference}, which is not in the components"),
                        );
                        index.insert(reference.clone(), components.len());
                        components.push(Component {
                            reference: reference.clone(),
                            kind: "component".into(),
                            value: None,
                            model: None,
                            pins: Vec::new(),
                        });
                        components.len() - 1
                    }
                };
                components[ix].pins.push(PinNet {
                    pin,
                    net: name.clone(),
                    function: node
                        .value("pinfunction")
                        .filter(|f| !f.is_empty() && f != "~"),
                });
            }
        }
    }
    for c in &mut components {
        c.pins.sort_by_key(|p| pin_key(&p.pin));
    }
    Ok(NetlistSpec {
        title: None,
        path: None,
        format: NetlistFormat::Kicad,
        design,
        components,
        subcircuits: Vec::new(),
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn continuation_and_comment_lines() {
        let lines = logical_lines("title\n* c\nR1 a\n+ b 1k ; tail\n\nC1 a 0 1u\n");
        assert_eq!(
            lines,
            [
                (1, "title".to_owned()),
                (3, "R1 a b 1k".to_owned()),
                (6, "C1 a 0 1u".to_owned())
            ]
        );
    }

    #[test]
    fn a_deck_without_title_keeps_its_first_element() {
        let spec = parse("R1 a b 1k\nC1 b 0 1n\n", None, None).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(spec.components.len(), 2);
        assert_eq!(spec.design, None);
        assert_eq!(spec.warnings.len(), 1);
    }

    #[test]
    fn sexp_reads_quoted_and_bare_atoms() {
        let s = sexp(r#"(export (version "E") (design (source "/a b.kicad_sch")))"#)
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            s.child("design").and_then(|d| d.value("source")).as_deref(),
            Some("/a b.kicad_sch")
        );
        assert!(sexp("(export (a)").is_err());
        assert!(sexp("(export))").is_err());
    }
}
