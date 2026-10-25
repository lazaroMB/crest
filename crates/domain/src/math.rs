//! TeX math in markdown: finding it before a CommonMark parser sees it.
//!
//! A CommonMark parser treats `_`, `*` and `\` as markup, so a formula
//! such as `$a_1 \cdot b_1$` loses its subscripts and backslashes when it
//! is parsed as prose, and `\(x\)` turns into `(x)`. This module finds the
//! math first, with the delimiters agents actually write:
//!
//! - display: `$$..$$`, `\[..\]` and ```` ```math ```` fences;
//! - inline: `$..$`, `\(..\)`, and a bare mhchem `\ce{..}`.
//!
//! It skips code fences, code spans and escaped `\$`. A single `$` follows
//! pandoc's rule: the opening `$` is followed by a non-space, and the
//! closing one is preceded by a non-space and not followed by a digit, so
//! prices (`$5 and $20`) stay prose. Inline math and `$$` stay within one
//! paragraph (no blank line inside).
//!
//! [`find`] returns the formulas, [`split`] the text cut into prose and
//! math, and [`normalize`] rewrites the markdown into the one form a
//! CommonMark parser with the math extensions keeps intact (`$..$` inline,
//! a `$$` fence on its own lines for display, and `\$` for every dollar
//! that is not math).

use std::ops::Range;

/// How a formula was written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delimiter {
    /// `$..$`
    Dollar,
    /// `$$..$$`
    DoubleDollar,
    /// `\(..\)`
    Paren,
    /// `\[..\]`
    Bracket,
    /// A ```` ```math ```` fence.
    Fence,
    /// A bare mhchem `\ce{..}` outside other math.
    Chem,
}

/// One formula in a markdown text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MathSpan {
    /// Byte range in the text, delimiters included.
    pub range: Range<usize>,
    /// The TeX source without delimiters (for [`Delimiter::Chem`] the whole
    /// `\ce{..}`), with the container prefix (`> `, list indentation) of
    /// continuation lines removed.
    pub source: String,
    /// Display (its own centered block) or inline (in the text flow).
    pub display: bool,
    /// How it was written.
    pub delimiter: Delimiter,
}

/// A piece of a markdown text: prose or a formula.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    /// Markdown prose, as a byte range of the text.
    Text(Range<usize>),
    /// A formula.
    Math(MathSpan),
}

/// The formulas of `markdown`, in order.
#[must_use]
pub fn find(markdown: &str) -> Vec<MathSpan> {
    scan(markdown).spans.into_iter().map(|s| s.span).collect()
}

/// `markdown` cut into prose and formulas, in order. The prose ranges and
/// formula ranges cover the whole text without overlapping.
#[must_use]
pub fn split(markdown: &str) -> Vec<Segment> {
    let mut segments = Vec::new();
    let mut cursor = 0;
    for span in find(markdown) {
        if span.range.start > cursor {
            segments.push(Segment::Text(cursor..span.range.start));
        }
        cursor = span.range.end;
        segments.push(Segment::Math(span));
    }
    if cursor < markdown.len() {
        segments.push(Segment::Text(cursor..markdown.len()));
    }
    segments
}

/// Rewrites `markdown` so a CommonMark parser with the math extensions
/// (`math_text`, `math_flow`) sees exactly the formulas [`find`] found:
/// inline math as `$..$` (more dollars when the source holds one), display
/// math as a `$$` fence on its own lines, ```` ```math ```` fences as they
/// are, and every other unescaped `$` outside code as `\$`.
///
/// Known gap: an inline formula whose source holds a `$` and that starts a
/// line gets a `$$` fence there, which the parser reads as display math.
#[must_use]
pub fn normalize(markdown: &str) -> String {
    let scan = scan(markdown);
    if scan.spans.is_empty() && scan.dollars.is_empty() {
        return markdown.to_owned();
    }
    let mut out = String::with_capacity(markdown.len() + 16);
    let mut cursor = 0;
    let mut dollars = scan.dollars.iter().copied().peekable();
    for found in &scan.spans {
        while let Some(d) = dollars.next_if(|d| *d < found.span.range.start) {
            out.push_str(&markdown[cursor..d]);
            out.push_str("\\$");
            cursor = d + 1;
        }
        out.push_str(&markdown[cursor..found.span.range.start]);
        cursor = found.span.range.end;
        let span = &found.span;
        match (span.delimiter, span.display) {
            (Delimiter::Fence, _) => out.push_str(&markdown[span.range.clone()]),
            (_, true) => {
                let prefix = &found.prefix;
                if !found.at_line_start {
                    // Break the paragraph: a blank line, then the fence.
                    out.push('\n');
                    out.push_str(prefix.trim_end());
                    out.push('\n');
                    out.push_str(prefix);
                }
                out.push_str("$$\n");
                for line in span.source.trim().lines() {
                    out.push_str(prefix);
                    out.push_str(line);
                    out.push('\n');
                }
                out.push_str(prefix);
                out.push_str("$$");
                let rest = markdown[cursor..].split('\n').next().unwrap_or_default();
                if !rest.trim().is_empty() {
                    out.push('\n');
                    out.push_str(prefix);
                    cursor += rest.len() - rest.trim_start().len();
                }
            }
            (_, false) => {
                let source = span.source.replace(['\r', '\n'], " ");
                let fence = "$".repeat(longest_run(&source, b'$') + 1);
                // A source that starts or ends with `$` would merge with the
                // fence; the parser strips one space from each side.
                let pad = if source.starts_with('$') || source.ends_with('$') {
                    " "
                } else {
                    ""
                };
                out.push_str(&fence);
                out.push_str(pad);
                out.push_str(&source);
                out.push_str(pad);
                out.push_str(&fence);
            }
        }
    }
    for d in dollars {
        out.push_str(&markdown[cursor..d]);
        out.push_str("\\$");
        cursor = d + 1;
    }
    out.push_str(&markdown[cursor..]);
    out
}

/// A formula plus where its block sits, for [`normalize`].
struct Found {
    span: MathSpan,
    /// Whether only the container prefix precedes the opening delimiter on
    /// its line.
    at_line_start: bool,
    /// The container prefix for continuation lines (`> `, list
    /// indentation as spaces).
    prefix: String,
}

struct Scan {
    spans: Vec<Found>,
    /// Byte offsets of `$` signs that are prose, not math.
    dollars: Vec<usize>,
}

fn scan(text: &str) -> Scan {
    let bytes = text.as_bytes();
    let fences = fences(text);
    let mut spans = Vec::new();
    let mut dollars = Vec::new();
    let mut next_fence = 0;
    let mut i = 0;
    while i < bytes.len() {
        if let Some(fence) = fences.get(next_fence)
            && i >= fence.range.start
        {
            if let Some(inner) = &fence.math {
                spans.push(Found {
                    span: MathSpan {
                        range: fence.range.clone(),
                        source: text[inner.clone()].to_owned(),
                        display: true,
                        delimiter: Delimiter::Fence,
                    },
                    at_line_start: true,
                    prefix: String::new(),
                });
            }
            i = fence.range.end;
            next_fence += 1;
            continue;
        }
        let limit = fences
            .get(next_fence)
            .map_or(bytes.len(), |f| f.range.start);
        let paragraph = paragraph_end(bytes, i, limit);
        match bytes[i] {
            b'\\' => {
                let rest = &bytes[i..];
                let found = if rest.starts_with(b"\\(") {
                    closing(bytes, i + 2, paragraph, b"\\)")
                        .map(|end| (end + 2, i + 2..end, false, Delimiter::Paren))
                } else if rest.starts_with(b"\\[") {
                    closing(bytes, i + 2, paragraph, b"\\]")
                        .map(|end| (end + 2, i + 2..end, true, Delimiter::Bracket))
                } else if rest.starts_with(b"\\ce{") {
                    balanced_brace(bytes, i + 3, paragraph)
                        .map(|end| (end, i..end, false, Delimiter::Chem))
                } else {
                    None
                };
                match found {
                    Some((end, inner, display, delimiter)) if !blank(&bytes[inner.clone()]) => {
                        spans.push(found_span(text, i..end, inner, display, delimiter));
                        i = end;
                    }
                    // Any other backslash escapes the next character.
                    _ => i += 2,
                }
            }
            b'`' => {
                let run = run_len(bytes, i, b'`');
                i = code_span_end(bytes, i, run, paragraph).unwrap_or(i + run);
            }
            b'$' if bytes.get(i + 1) == Some(&b'$') => {
                match closing(bytes, i + 2, paragraph, b"$$") {
                    Some(end) if !blank(&bytes[i + 2..end]) => {
                        spans.push(found_span(
                            text,
                            i..end + 2,
                            i + 2..end,
                            true,
                            Delimiter::DoubleDollar,
                        ));
                        i = end + 2;
                    }
                    _ => {
                        dollars.extend([i, i + 1]);
                        i += 2;
                    }
                }
            }
            b'$' => match inline_dollar_end(bytes, i, paragraph) {
                Some(end) => {
                    spans.push(found_span(
                        text,
                        i..end + 1,
                        i + 1..end,
                        false,
                        Delimiter::Dollar,
                    ));
                    i = end + 1;
                }
                None => {
                    dollars.push(i);
                    i += 1;
                }
            },
            _ => i += 1,
        }
    }
    Scan { spans, dollars }
}

fn found_span(
    text: &str,
    range: Range<usize>,
    inner: Range<usize>,
    display: bool,
    delimiter: Delimiter,
) -> Found {
    let line_start = text[..range.start].rfind('\n').map_or(0, |n| n + 1);
    let (container, prefix) = container_prefix(&text[line_start..range.start]);
    let at_line_start = container == range.start - line_start;
    let mut source = String::with_capacity(inner.len());
    for (n, line) in text[inner].split('\n').enumerate() {
        if n > 0 {
            source.push('\n');
            source.push_str(strip_prefix(line, prefix.len()));
        } else {
            source.push_str(line);
        }
    }
    Found {
        span: MathSpan {
            range,
            source,
            display,
            delimiter,
        },
        at_line_start,
        prefix,
    }
}

/// The container prefix at the start of `line` (whitespace and `>`, then
/// one list marker): its length in bytes, and the prefix continuation
/// lines carry (the list marker turned into spaces).
fn container_prefix(line: &str) -> (usize, String) {
    let quote = line
        .bytes()
        .take_while(|b| matches!(b, b' ' | b'\t' | b'>'))
        .count();
    let rest = &line.as_bytes()[quote..];
    let digits = rest.iter().take_while(|b| b.is_ascii_digit()).count();
    let marker = match rest.get(digits) {
        Some(b'-' | b'*' | b'+') if digits == 0 => 1,
        Some(b'.' | b')') if digits > 0 => digits + 1,
        _ => 0,
    };
    let spaces = if marker > 0 {
        rest[marker..].iter().take_while(|b| **b == b' ').count()
    } else {
        0
    };
    if marker > 0 && spaces > 0 {
        let len = quote + marker + spaces;
        let prefix = format!("{}{}", &line[..quote], " ".repeat(marker + spaces));
        (len, prefix)
    } else {
        (quote, line[..quote].to_owned())
    }
}

/// `line` without up to `width` leading bytes of container prefix.
fn strip_prefix(line: &str, width: usize) -> &str {
    let n = line
        .bytes()
        .take(width)
        .take_while(|b| matches!(b, b' ' | b'\t' | b'>'))
        .count();
    &line[n..]
}

/// A fenced code block: its whole range and, for a `math` fence, the range
/// of its content.
struct Fence {
    range: Range<usize>,
    math: Option<Range<usize>>,
}

/// The fenced code blocks of `text` (``` or ~~~, any container prefix). An
/// unclosed fence runs to the end, as CommonMark has it.
fn fences(text: &str) -> Vec<Fence> {
    let mut fences = Vec::new();
    // (marker, run, start of the opening line, start of the content, math)
    let mut open: Option<(u8, usize, usize, usize, bool)> = None;
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let start = offset;
        offset += line.len();
        let body = line.trim_start_matches([' ', '\t', '>']);
        let marker = body.bytes().next().filter(|b| matches!(b, b'`' | b'~'));
        let run = marker.map_or(0, |m| run_len(body.as_bytes(), 0, m));
        match (open, marker) {
            (None, Some(m)) if run >= 3 => {
                let info = body[run..].trim();
                // A backtick fence's info string cannot hold a backtick.
                if m == b'`' && info.contains('`') {
                    continue;
                }
                let math = info.split_whitespace().next() == Some("math");
                open = Some((m, run, start, offset, math));
            }
            (Some((m, len, from, content, math)), Some(c))
                if c == m && run >= len && body[run..].trim().is_empty() =>
            {
                fences.push(Fence {
                    range: from..offset,
                    math: math.then_some(content..start),
                });
                open = None;
            }
            _ => {}
        }
    }
    if let Some((_, _, from, content, math)) = open {
        fences.push(Fence {
            range: from..text.len(),
            math: math.then_some(content..text.len()),
        });
    }
    fences
}

/// Where the paragraph holding `from` ends: the next blank line, or
/// `limit`.
fn paragraph_end(bytes: &[u8], from: usize, limit: usize) -> usize {
    let mut i = from;
    while i < limit {
        if bytes[i] == b'\n' {
            let mut j = i + 1;
            while j < limit && matches!(bytes[j], b' ' | b'\t' | b'\r' | b'>') {
                j += 1;
            }
            if j >= limit || bytes[j] == b'\n' {
                return i;
            }
        }
        i += 1;
    }
    limit
}

/// The offset of the next unescaped `close` in `from..limit`.
fn closing(bytes: &[u8], from: usize, limit: usize, close: &[u8]) -> Option<usize> {
    let mut i = from;
    while i + close.len() <= limit {
        if bytes[i..].starts_with(close) {
            return Some(i);
        }
        // `\` escapes the next byte, except that `\)` / `\]` is the closer.
        i += if bytes[i] == b'\\' { 2 } else { 1 };
    }
    None
}

/// The end (exclusive) of the `{..}` group opening at `open`, counting
/// nested braces and skipping escaped ones.
fn balanced_brace(bytes: &[u8], open: usize, limit: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut i = open;
    while i < limit {
        match bytes[i] {
            b'\\' => i += 1,
            b'{' => depth += 1,
            b'}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// The closing `$` of a single-dollar formula opening at `open`, under
/// pandoc's rule.
fn inline_dollar_end(bytes: &[u8], open: usize, limit: usize) -> Option<usize> {
    if bytes.get(open + 1).is_none_or(u8::is_ascii_whitespace) {
        return None;
    }
    let mut i = open + 1;
    while i < limit {
        match bytes[i] {
            b'\\' => i += 2,
            b'$' => {
                if !bytes[i - 1].is_ascii_whitespace()
                    && !bytes.get(i + 1).is_some_and(u8::is_ascii_digit)
                {
                    return Some(i);
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    None
}

/// The end of the code span whose opening backtick run (`run` long) starts
/// at `open`: after the next run of exactly the same length.
fn code_span_end(bytes: &[u8], open: usize, run: usize, limit: usize) -> Option<usize> {
    let mut i = open + run;
    while i < limit {
        if bytes[i] == b'`' {
            let len = run_len(bytes, i, b'`');
            if len == run {
                return Some(i + len);
            }
            i += len;
        } else {
            i += 1;
        }
    }
    None
}

fn run_len(bytes: &[u8], from: usize, byte: u8) -> usize {
    bytes[from..].iter().take_while(|b| **b == byte).count()
}

fn longest_run(text: &str, byte: u8) -> usize {
    text.as_bytes()
        .split(|b| *b != byte)
        .map(<[u8]>::len)
        .max()
        .unwrap_or_default()
}

fn blank(bytes: &[u8]) -> bool {
    bytes.iter().all(u8::is_ascii_whitespace)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sources(text: &str) -> Vec<(String, bool)> {
        find(text)
            .into_iter()
            .map(|s| (s.source, s.display))
            .collect()
    }

    fn inline(s: &str) -> (String, bool) {
        (s.to_owned(), false)
    }

    fn display(s: &str) -> (String, bool) {
        (s.to_owned(), true)
    }

    #[test]
    fn finds_every_delimiter_and_keeps_underscores_and_backslashes() {
        let text = "Let $a_1 \\cdot b_1$ and \\(x_i^2\\) hold:\n\n\
                    $$\\sum_{i=1}^n x_i$$\n\n\\[ \\frac{a}{b} \\]\n\n\
                    ```math\nE = mc^2\n```\n";
        assert_eq!(
            sources(text),
            [
                inline("a_1 \\cdot b_1"),
                inline("x_i^2"),
                display("\\sum_{i=1}^n x_i"),
                display(" \\frac{a}{b} "),
                display("E = mc^2\n"),
            ]
        );
        let delimiters: Vec<_> = find(text).iter().map(|s| s.delimiter).collect();
        assert_eq!(
            delimiters,
            [
                Delimiter::Dollar,
                Delimiter::Paren,
                Delimiter::DoubleDollar,
                Delimiter::Bracket,
                Delimiter::Fence,
            ]
        );
    }

    #[test]
    fn prices_are_prose_under_the_pandoc_rule() {
        assert!(find("It costs $5 and $20 today").is_empty());
        assert!(find("between $ 5 and 6$").is_empty());
        assert!(find("$x $").is_empty());
        assert!(find("from $10 to $20, or $5").is_empty());
        assert_eq!(sources("price $5 vs $x$"), [inline("5 vs $x")]);
    }

    #[test]
    fn code_and_escapes_hide_dollars() {
        assert!(find("run `echo $HOME $PATH` now").is_empty());
        assert!(find("``a ` $x$ ``").is_empty());
        assert!(find("\\$x\\$ is literal").is_empty());
        assert!(find("```sh\necho $a $b\n$$x$$\n```\n").is_empty());
        assert!(find("~~~\n\\(x\\)\n~~~").is_empty());
        assert!(find("a \\\\(x) b").is_empty());
    }

    #[test]
    fn inline_math_stays_in_its_paragraph() {
        assert!(find("$a\n\nb$").is_empty());
        assert_eq!(sources("$a\nb$"), [inline("a\nb")]);
        assert!(find("$$a\n\nb$$").is_empty());
    }

    #[test]
    fn escaped_dollar_inside_math_does_not_close_it() {
        assert_eq!(sources("$a \\$ b$"), [inline("a \\$ b")]);
    }

    #[test]
    fn bare_mhchem_is_inline_math() {
        assert_eq!(
            sources("Water \\ce{H2O} and $\\ce{CO2}$ and \\ce{A_{b}}"),
            [
                inline("\\ce{H2O}"),
                inline("\\ce{CO2}"),
                inline("\\ce{A_{b}}"),
            ]
        );
        assert!(find("\\ce{unclosed").is_empty());
    }

    #[test]
    fn container_prefix_is_removed_from_display_lines() {
        let text = "> $$\n> a +\n> b\n> $$\n";
        assert_eq!(sources(text), [display("\na +\nb\n")]);
    }

    #[test]
    fn split_covers_the_text() {
        let text = "a $x$ b $$y$$ c";
        let segments = split(text);
        assert_eq!(segments.len(), 5);
        assert_eq!(segments[0], Segment::Text(0..2));
        assert!(matches!(&segments[1], Segment::Math(m) if m.source == "x" && m.range == (2..5)));
        assert_eq!(segments[4], Segment::Text(13..15));
        assert_eq!(split(""), []);
        assert_eq!(split("plain"), [Segment::Text(0..5)]);
    }

    #[test]
    fn normalize_canonicalizes_delimiters() {
        assert_eq!(normalize("no math"), "no math");
        assert_eq!(normalize("a \\(x_1\\) b"), "a $x_1$ b");
        assert_eq!(normalize("costs $5 and $20"), "costs \\$5 and \\$20");
        assert_eq!(normalize("`$x$` \\$y"), "`$x$` \\$y");
        // Math text is literal like a code span, so `\$` needs a longer fence.
        assert_eq!(normalize("$a \\$ b$"), "$$a \\$ b$$");
        assert_eq!(normalize("$a\nb$"), "$a b$");
        assert_eq!(normalize("\\ce{H2O}"), "$\\ce{H2O}$");
    }

    #[test]
    fn normalize_puts_display_math_on_its_own_lines() {
        assert_eq!(normalize("$$x^2$$"), "$$\nx^2\n$$");
        assert_eq!(normalize("\\[\nx\n\\]\n"), "$$\nx\n$$\n");
        assert_eq!(normalize("see $$x$$ then"), "see \n\n$$\nx\n$$\nthen");
        assert_eq!(normalize("> $$\n> a\n> $$"), "> $$\n> a\n> $$");
        assert_eq!(normalize("- $$a$$\n"), "- $$\n  a\n  $$\n");
        let fence = "```math\nx_1\n```\n";
        assert_eq!(normalize(fence), fence);
    }

    #[test]
    fn unclosed_display_math_while_streaming_is_escaped() {
        assert_eq!(normalize("$$\\frac{a"), "\\$\\$\\frac{a");
    }
}
