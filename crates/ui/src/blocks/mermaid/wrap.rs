//! Line breaks for long flowchart labels.
//!
//! rusty-mermaid sizes a node from its longest label line and never wraps,
//! so a sentence-long label makes a box as wide as the sentence. Before
//! layout, [`wrap_labels`] inserts `<br>` at word boundaries in the node
//! and edge labels of a `flowchart`/`graph` source; rusty-mermaid already
//! sizes and centers multi-line labels. Other diagram kinds, and lines that
//! are not node or edge definitions (`%%`, `classDef`, `style`...), are
//! left untouched. A word longer than the limit is never split.

use std::borrow::Cow;

/// Columns of a label line before it wraps.
pub const LABEL_COLUMNS: usize = 24;

/// Statements whose brackets are not labels.
const VERBATIM: [&str; 9] = [
    "%%",
    "classDef",
    "class ",
    "style ",
    "linkStyle",
    "click ",
    "subgraph",
    "direction",
    "accDescr",
];

/// `source` with the long labels of a flowchart wrapped at `columns`.
pub fn wrap_labels(source: &str, columns: usize) -> Cow<'_, str> {
    let header = source
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with("%%"));
    let is_flowchart =
        header.is_some_and(|line| line.starts_with("flowchart") || line.starts_with("graph"));
    if !is_flowchart {
        return Cow::Borrowed(source);
    }
    let mut out = String::with_capacity(source.len());
    let mut changed = false;
    for line in source.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if VERBATIM.iter().any(|prefix| trimmed.starts_with(prefix)) {
            out.push_str(line);
            continue;
        }
        let wrapped = wrap_line(line, columns);
        changed |= matches!(wrapped, Cow::Owned(_));
        out.push_str(&wrapped);
    }
    if changed {
        Cow::Owned(out)
    } else {
        Cow::Borrowed(source)
    }
}

/// Wraps the labels of one statement line.
fn wrap_line(line: &str, columns: usize) -> Cow<'_, str> {
    let chars: Vec<(usize, char)> = line.char_indices().collect();
    let mut spans = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let (_, c) = chars[i];
        let after_id = i > 0 && is_id_char(chars[i - 1].1);
        match c {
            '|' => {
                let start = i + 1;
                let Some(end) = find(&chars, start, |c| c == '|') else {
                    break;
                };
                spans.push(span(&chars, line, start, end));
                i = end + 1;
            }
            '[' | '(' | '{' | '>' if after_id => {
                let mut start = i + 1;
                while start < chars.len() && "[({/\\".contains(chars[start].1) {
                    start += 1;
                }
                let end = if chars.get(start).is_some_and(|(_, c)| *c == '"') {
                    start += 1;
                    find(&chars, start, |c| c == '"')
                } else {
                    find(&chars, start, |c| "])}".contains(c))
                };
                let Some(end) = end else {
                    break;
                };
                spans.push(span(&chars, line, start, end));
                i = end + 1;
            }
            '"' => {
                // A quoted string outside a shape: skip it whole.
                i = find(&chars, i + 1, |c| c == '"').map_or(chars.len(), |end| end + 1);
            }
            _ => i += 1,
        }
    }
    let mut out = String::new();
    let mut last = 0;
    for (start, end) in spans {
        let label = &line[start..end];
        // Markdown strings (`"`...`"`) are left to the parser.
        if label.starts_with('`') {
            continue;
        }
        if let Cow::Owned(wrapped) = wrap_text(label, columns) {
            out.push_str(&line[last..start]);
            out.push_str(&wrapped);
            last = end;
        }
    }
    if last == 0 {
        return Cow::Borrowed(line);
    }
    out.push_str(&line[last..]);
    Cow::Owned(out)
}

/// Whether a shape opener right after `c` starts a label (`A[`), as
/// opposed to an arrow (`-->`) or other syntax.
fn is_id_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Index of the first char from `from` that matches `pred`.
fn find(chars: &[(usize, char)], from: usize, pred: impl Fn(char) -> bool) -> Option<usize> {
    (from..chars.len()).find(|&j| pred(chars[j].1))
}

/// Byte range of the chars `start..end` of `line`.
fn span(chars: &[(usize, char)], line: &str, start: usize, end: usize) -> (usize, usize) {
    let byte = |j: usize| chars.get(j).map_or(line.len(), |(b, _)| *b);
    (byte(start), byte(end))
}

/// `text` with `<br>` inserted so no line exceeds `columns` characters
/// (except a single longer word). Existing `<br>`s are kept and start a
/// new line.
pub fn wrap_text(text: &str, columns: usize) -> Cow<'_, str> {
    let mut out = String::new();
    let mut changed = false;
    let mut rest = text;
    loop {
        let (segment, brk, next) = match find_br(rest) {
            Some((at, len)) => (&rest[..at], &rest[at..at + len], Some(&rest[at + len..])),
            None => (rest, "", None),
        };
        if segment.chars().count() > columns {
            changed = true;
            let mut width = 0;
            for word in segment.split(' ').filter(|word| !word.is_empty()) {
                let len = word.chars().count();
                if width > 0 && width + 1 + len > columns {
                    out.push_str("<br>");
                    width = 0;
                } else if width > 0 {
                    out.push(' ');
                    width += 1;
                }
                out.push_str(word);
                width += len;
            }
        } else {
            out.push_str(segment);
        }
        out.push_str(brk);
        match next {
            Some(next) => rest = next,
            None => break,
        }
    }
    if changed {
        Cow::Owned(out)
    } else {
        Cow::Borrowed(text)
    }
}

/// Byte offset and length of the first `<br>`, `<br/>` or `<br />` in
/// `text`, in any case.
pub fn find_br(text: &str) -> Option<(usize, usize)> {
    let lower = text.to_ascii_lowercase();
    let mut from = 0;
    while let Some(at) = lower[from..].find("<br") {
        let at = from + at;
        let tail = &lower[at + 3..];
        let len = if tail.starts_with('>') {
            Some(4)
        } else if tail.starts_with("/>") {
            Some(5)
        } else if tail.starts_with(" />") {
            Some(6)
        } else {
            None
        };
        if let Some(len) = len {
            return Some((at, len));
        }
        from = at + 3;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const LONG: &str = "Validate the user input and check every permission";

    #[test]
    fn long_node_labels_wrap_at_word_boundaries() {
        let source = format!("flowchart TD\n  A[{LONG}] --> B(short)\n");
        let wrapped = wrap_labels(&source, 24);
        assert_eq!(
            wrapped,
            "flowchart TD\n  A[Validate the user input<br>and check every<br>permission] --> B(short)\n"
        );
    }

    #[test]
    fn shapes_quotes_and_edge_labels_wrap() {
        let source =
            format!("graph LR\n  A([{LONG}]) -->|{LONG}| B{{\"{LONG}\"}}\n  C{{{{{LONG}}}}}\n");
        let wrapped = wrap_labels(&source, 24);
        let breaks = wrapped.matches("<br>").count();
        assert_eq!(
            breaks, 8,
            "two breaks in each of the four labels: {wrapped}"
        );
        assert!(wrapped.contains("A([Validate"), "{wrapped}");
        assert!(wrapped.contains("B{\"Validate"), "{wrapped}");
        assert!(wrapped.contains("C{{Validate"), "{wrapped}");
    }

    #[test]
    fn wrapping_is_idempotent() {
        let source = format!("flowchart TD\n  A[{LONG}] -->|{LONG}| B\n");
        let once = wrap_labels(&source, 24).into_owned();
        assert_eq!(wrap_labels(&once, 24), once.as_str());
    }

    #[test]
    fn verbatim_statements_and_other_diagrams_are_untouched() {
        let flow = format!(
            "flowchart TD\n  %% A[{LONG}]\n  classDef big fill:#f9f\n  subgraph S [{LONG}]\n  A --> B\n  end\n"
        );
        assert!(matches!(wrap_labels(&flow, 24), Cow::Borrowed(_)));
        let sequence = format!("sequenceDiagram\n  A->>B: {LONG}\n");
        assert!(matches!(wrap_labels(&sequence, 24), Cow::Borrowed(_)));
    }

    #[test]
    fn existing_breaks_restart_the_count_and_long_words_stay_whole() {
        assert_eq!(
            wrap_text("short<BR/>also short", 10),
            "short<BR/>also short"
        );
        assert_eq!(
            wrap_text("a supercalifragilistic word", 10),
            "a<br>supercalifragilistic<br>word"
        );
    }

    #[test]
    fn br_is_found_in_any_case() {
        assert_eq!(find_br("a<BR>b"), Some((1, 4)));
        assert_eq!(find_br("a<br />b"), Some((1, 6)));
        assert_eq!(find_br("a<brx>b"), None);
    }
}
