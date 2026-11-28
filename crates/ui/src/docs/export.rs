//! Exporting a doc: the Markdown file with its images inlined, the Web page
//! with the print stylesheet of the prototype, and the file names.
//!
//! The panel only asks (`DocPanelEvent::Export`); the host saves the file
//! or writes the clipboard with what these functions build. The PDF is the
//! same page with a script that opens the print dialog ([`pdf_export`]).
//!
//! Diagrams: each `mermaid` fence is drawn by crest's own renderer
//! ([`parse_with`] and [`to_svg`]) and embedded as an inline SVG in a
//! `<figure class="diagram">`, with a light theme whatever the app's, since
//! the page is read on white paper. A source the parser rejects stays as
//! `<pre class="mermaid-source">` with the reason in `<p class="diagram-error">`.

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

use domain::doc::{ExportFormat, file_stem, inline_assets};
use domain::theme::{Appearance, Theme};

use crate::app::ColorTheme;
use crate::blocks::mermaid::{diagram::parse_with, raster::to_svg};

/// The font list of the diagrams: the page body's, with a concrete first name
/// (`to_svg` only writes it into `font-family` attributes).
const DIAGRAM_FONT: &str = "Helvetica, Arial, sans-serif";

/// The print stylesheet of the prototype (`PRINT_CSS` in `doc-actions.js`).
pub const PRINT_CSS: &str = r#"
  body { font: 15px/1.6 -apple-system, "Segoe UI", sans-serif; color: #222; max-width: 760px; margin: 40px auto; padding: 0 24px; }
  h1, h2, h3 { line-height: 1.25; } h1 { font-size: 1.8em; } h2 { font-size: 1.35em; margin-top: 1.6em; }
  code { font: 0.9em "JetBrains Mono", Menlo, monospace; background: #f3f1ed; padding: 0 4px; border-radius: 3px; }
  pre { background: #f3f1ed; padding: 12px; border-radius: 6px; overflow-x: auto; } pre code { background: none; padding: 0; }
  table { border-collapse: collapse; margin: 12px 0; } th, td { border: 1px solid #ddd; padding: 4px 10px; } .md-num { text-align: right; }
  blockquote { margin: 0; padding-left: 12px; border-left: 3px solid #ddd; color: #555; } img { max-width: 100%; }
  figure.diagram { margin: 12px 0; text-align: center; break-inside: avoid; } figure.diagram svg { max-width: 100%; height: auto; }
  .diagram-error { color: #b3261e; font-size: 0.9em; }
"#;

/// The markdown of a doc as one file: every `assets/<name>` image becomes a
/// data URI.
#[must_use]
pub fn markdown_export(text: &str, assets: &[(String, Vec<u8>)]) -> String {
    inline_assets(text, assets)
}

/// The file name an export is saved as: the title's file stem and the
/// format's extension (`md` for the formats without one).
#[must_use]
pub fn export_file_name(title: &str, format: ExportFormat) -> String {
    let extension = format
        .extension()
        .map_or("md", |ext| ext.trim_start_matches('.'));
    format!("{}.{extension}", file_stem(title))
}

/// The doc as one self-contained web page: `title`, the print stylesheet,
/// and `text` as HTML with its images inlined.
#[must_use]
pub fn html_export(title: &str, text: &str, assets: &[(String, Vec<u8>)]) -> String {
    let body = to_html(&inline_assets(text, assets));
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>{}</title><style>{PRINT_CSS}</style></head><body>{body}</body></html>\n",
        escape(title)
    )
}

/// The doc as the web page of [`html_export`] plus a script that opens the
/// browser's print dialog once the page has loaded, to save it as a PDF.
#[must_use]
pub fn pdf_export(title: &str, text: &str, assets: &[(String, Vec<u8>)]) -> String {
    let html = html_export(title, text, assets);
    let script = "<script>window.addEventListener('load', () => window.print());</script>";
    match html.rfind("</body>") {
        Some(at) => format!("{}{script}{}", &html[..at], &html[at..]),
        None => html + script,
    }
}

/// The `file://` URL of a local path, with the characters a URL cannot hold
/// (space, `%`, `#`, `?`) percent-encoded.
#[must_use]
pub fn file_url(path: &std::path::Path) -> String {
    let mut url = String::from("file://");
    for c in path.display().to_string().chars() {
        match c {
            ' ' => url.push_str("%20"),
            '%' => url.push_str("%25"),
            '#' => url.push_str("%23"),
            '?' => url.push_str("%3F"),
            c => url.push(c),
        }
    }
    url
}

/// The light, print-friendly theme of the diagrams (dark text on white),
/// derived from the light defaults whatever the app's current theme.
fn light_theme() -> &'static ColorTheme {
    static THEME: OnceLock<ColorTheme> = OnceLock::new();
    THEME.get_or_init(|| {
        Arc::new(Theme::derive(
            "export",
            Appearance::Light,
            &BTreeMap::new(),
            &BTreeMap::new(),
        ))
    })
}

/// A `mermaid` fence as an inline SVG, or its source and the parser's
/// reason when it does not parse.
fn diagram(source: &str) -> String {
    match parse_with(source, light_theme()) {
        Ok(parsed) => format!(
            "<figure class=\"diagram\">{}</figure>\n",
            to_svg(&parsed, DIAGRAM_FONT)
        ),
        Err(err) => format!(
            "<pre class=\"mermaid-source\">{}</pre>\n<p class=\"diagram-error\">{}</p>\n",
            escape(source),
            escape(&err)
        ),
    }
}

/// Escapes text for an HTML body or a quoted attribute.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

/// Markdown to HTML for what a doc holds: headings, paragraphs, lists,
/// quotes, fenced code, tables, rules, images, links and emphasis. Raw HTML
/// in the text is escaped, never passed through.
fn to_html(markdown: &str) -> String {
    let lines: Vec<&str> = markdown.lines().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim_start();
        if line.trim().is_empty() {
            i += 1;
        } else if let Some((fence, info)) = fence_open(trimmed) {
            let mut body = Vec::new();
            i += 1;
            while i < lines.len() && !is_fence_close(lines[i], &fence) {
                body.push(lines[i]);
                i += 1;
            }
            i += 1;
            out.push_str(&fenced(info, &body.join("\n")));
        } else if let Some((level, text)) = heading(trimmed) {
            out.push_str(&format!("<h{level}>{}</h{level}>\n", inline(text)));
            i += 1;
        } else if is_rule(trimmed) {
            out.push_str("<hr>\n");
            i += 1;
        } else if trimmed.starts_with('>') {
            let mut inner = Vec::new();
            while i < lines.len() && lines[i].trim_start().starts_with('>') {
                let l = lines[i].trim_start().trim_start_matches('>');
                inner.push(l.strip_prefix(' ').unwrap_or(l));
                i += 1;
            }
            out.push_str(&format!(
                "<blockquote>{}</blockquote>\n",
                to_html(&inner.join("\n"))
            ));
        } else if list_marker(trimmed).is_some() {
            let ordered = list_marker(trimmed) == Some(true);
            let tag = if ordered { "ol" } else { "ul" };
            out.push_str(&format!("<{tag}>"));
            while i < lines.len() && list_marker(lines[i].trim_start()).is_some() {
                out.push_str(&format!(
                    "<li>{}</li>",
                    inline(strip_marker(lines[i].trim_start()))
                ));
                i += 1;
            }
            out.push_str(&format!("</{tag}>\n"));
        } else if is_table_start(&lines, i) {
            let start = i;
            while i < lines.len() && lines[i].contains('|') && !lines[i].trim().is_empty() {
                i += 1;
            }
            out.push_str(&table(&lines[start..i]));
        } else {
            let start = i;
            while i < lines.len()
                && !lines[i].trim().is_empty()
                && (i == start || !starts_block(lines[i].trim_start()))
            {
                i += 1;
            }
            let text = lines[start..i]
                .iter()
                .map(|l| l.trim())
                .collect::<Vec<_>>()
                .join("\n");
            out.push_str(&format!("<p>{}</p>\n", inline(&text)));
        }
    }
    out
}

fn starts_block(line: &str) -> bool {
    fence_open(line).is_some()
        || heading(line).is_some()
        || line.starts_with('>')
        || list_marker(line).is_some()
        || is_rule(line)
}

fn fenced(info: &str, body: &str) -> String {
    let lang = info.split_whitespace().next().unwrap_or_default();
    if lang.eq_ignore_ascii_case("mermaid") {
        return diagram(body);
    }
    let class = if lang.is_empty() {
        String::new()
    } else {
        format!(" class=\"language-{}\"", escape(lang))
    };
    format!("<pre><code{class}>{}</code></pre>\n", escape(body))
}

/// The fence a line opens (its characters) and its info string.
fn fence_open(line: &str) -> Option<(String, &str)> {
    let first = line.chars().next().filter(|c| *c == '`' || *c == '~')?;
    let len = line.chars().take_while(|c| *c == first).count();
    (len >= 3).then(|| (first.to_string().repeat(len), line[len..].trim()))
}

fn is_fence_close(line: &str, fence: &str) -> bool {
    let line = line.trim();
    line.len() >= fence.len() && line.chars().all(|c| fence.starts_with(c))
}

fn heading(line: &str) -> Option<(usize, &str)> {
    let level = line.chars().take_while(|c| *c == '#').count();
    if (1..=6).contains(&level) && line[level..].starts_with(' ') {
        Some((level, line[level..].trim().trim_end_matches('#').trim_end()))
    } else {
        None
    }
}

fn is_rule(line: &str) -> bool {
    let t: String = line.chars().filter(|c| !c.is_whitespace()).collect();
    t.len() >= 3 && (t.chars().all(|c| c == '-') || t.chars().all(|c| c == '*'))
}

/// `Some(ordered)` for a list item line.
fn list_marker(line: &str) -> Option<bool> {
    if line.starts_with("- ") || line.starts_with("* ") || line.starts_with("+ ") {
        return Some(false);
    }
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    (digits > 0 && line[digits..].starts_with(". ")).then_some(true)
}

fn strip_marker(line: &str) -> &str {
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 {
        line[digits + 2..].trim()
    } else {
        line[2..].trim()
    }
}

fn cells(line: &str) -> Vec<String> {
    let line = line.trim();
    let line = line.strip_prefix('|').unwrap_or(line);
    let line = line.strip_suffix('|').unwrap_or(line);
    let mut out = vec![String::new()];
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' && chars.peek() == Some(&'|') {
            chars.next();
            if let Some(cell) = out.last_mut() {
                cell.push('|');
            }
        } else if c == '|' {
            out.push(String::new());
        } else {
            if let Some(cell) = out.last_mut() {
                cell.push(c);
            }
        }
    }
    out.into_iter().map(|c| c.trim().to_owned()).collect()
}

fn is_separator(line: &str) -> bool {
    let cells = cells(line);
    !cells.is_empty()
        && cells.iter().all(|c| {
            let c = c.trim_matches(':');
            !c.is_empty() && c.chars().all(|ch| ch == '-')
        })
}

fn is_table_start(lines: &[&str], at: usize) -> bool {
    lines[at].contains('|') && lines.get(at + 1).is_some_and(|next| is_separator(next))
}

fn table(lines: &[&str]) -> String {
    let numeric: Vec<bool> = cells(lines[1])
        .iter()
        .map(|c| c.ends_with(':') && !c.starts_with(':'))
        .collect();
    let cell = |tag: &str, text: &str, ix: usize| {
        let class = if numeric.get(ix) == Some(&true) {
            " class=\"md-num\""
        } else {
            ""
        };
        format!("<{tag}{class}>{}</{tag}>", inline(text))
    };
    let mut out = String::from("<table><thead><tr>");
    for (ix, text) in cells(lines[0]).iter().enumerate() {
        out.push_str(&cell("th", text, ix));
    }
    out.push_str("</tr></thead><tbody>");
    for line in &lines[2..] {
        out.push_str("<tr>");
        for (ix, text) in cells(line).iter().enumerate() {
            out.push_str(&cell("td", text, ix));
        }
        out.push_str("</tr>");
    }
    out.push_str("</tbody></table>\n");
    out
}

/// A link target that is safe to put in an `href` or `src`.
fn safe_url(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    !lower.contains(':')
        || ["http:", "https:", "mailto:", "data:image/"]
            .iter()
            .any(|scheme| lower.starts_with(scheme))
}

/// Inline markdown: code spans, images, links, bold, italic, strike.
fn inline(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let rest = &chars[i..];
        if c == '`'
            && let Some(end) = find(rest, 1, &['`'])
        {
            let code: String = rest[1..end].iter().collect();
            out.push_str(&format!("<code>{}</code>", escape(&code)));
            i += end + 1;
        } else if let Some((html, used)) = link_or_image(rest) {
            out.push_str(&html);
            i += used;
        } else if let Some((tag, len)) = emphasis(rest)
            && let Some(end) = find_run(rest, len, c)
        {
            let inner: String = rest[len..end].iter().collect();
            out.push_str(&format!("<{tag}>{}</{tag}>", inline(&inner)));
            i += end + len;
        } else if c == '\n' {
            out.push_str("<br>\n");
            i += 1;
        } else if c == '\\' && chars.get(i + 1).is_some_and(|n| "*_`[]()\\#|".contains(*n)) {
            out.push_str(&escape(&chars[i + 1].to_string()));
            i += 2;
        } else {
            out.push_str(&escape(&c.to_string()));
            i += 1;
        }
    }
    out
}

/// Index in `chars`, from `from`, of the first of `needle`.
fn find(chars: &[char], from: usize, needle: &[char]) -> Option<usize> {
    (from..chars.len()).find(|&ix| needle.contains(&chars[ix]))
}

/// Index of the closing run of `len` copies of `mark` (opened at 0), with
/// text between and no space right after the opening mark.
fn find_run(chars: &[char], len: usize, mark: char) -> Option<usize> {
    if chars.get(len).is_none_or(|c| c.is_whitespace()) {
        return None;
    }
    let mut ix = len + 1;
    while ix + len <= chars.len() {
        if chars[ix..ix + len].iter().all(|c| *c == mark) && !chars[ix - 1].is_whitespace() {
            // `_` inside a word does not close.
            if mark != '_' || chars.get(ix + len).is_none_or(|c| !c.is_alphanumeric()) {
                return Some(ix);
            }
        }
        ix += 1;
    }
    None
}

/// The tag and mark length of an emphasis run opening `chars`.
fn emphasis(chars: &[char]) -> Option<(&'static str, usize)> {
    let c = chars[0];
    let double = chars.get(1) == Some(&c);
    match c {
        '*' | '_' if double => Some(("strong", 2)),
        '*' | '_' => Some(("em", 1)),
        '~' if double => Some(("del", 2)),
        _ => None,
    }
}

/// `![alt](url)` or `[text](url)` at the start of `chars`: its HTML and
/// how many characters it used.
fn link_or_image(chars: &[char]) -> Option<(String, usize)> {
    let image = chars[0] == '!';
    let open = usize::from(image);
    if chars.get(open) != Some(&'[') {
        return None;
    }
    let close = find(chars, open + 1, &[']'])?;
    if chars.get(close + 1) != Some(&'(') {
        return None;
    }
    let end = find(chars, close + 2, &[')'])?;
    let label: String = chars[open + 1..close].iter().collect();
    let url: String = chars[close + 2..end].iter().collect();
    let url = url.trim();
    let html = if !safe_url(url) {
        escape(&label)
    } else if image {
        format!("<img src=\"{}\" alt=\"{}\">", escape(url), escape(&label))
    } else {
        format!("<a href=\"{}\">{}</a>", escape(url), inline(&label))
    };
    Some((html, end + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = "# Release notes\n\nSome **bold** and `code` text.\n\n- one\n- two\n\n```mermaid\nflowchart TD\n  A --> B\n```\n\n| a | b |\n| --- | ---: |\n| 1 | 2 |\n\n![shot](assets/shot.png)\n";

    #[test]
    fn file_names_follow_the_title_and_the_format() {
        assert_eq!(
            export_file_name("Release Notes!", ExportFormat::Markdown),
            "release-notes.md"
        );
        assert_eq!(
            export_file_name("Release Notes!", ExportFormat::Html),
            "release-notes.html"
        );
        assert_eq!(export_file_name("", ExportFormat::Html), "doc.html");
    }

    #[test]
    fn html_holds_the_heading_the_diagram_and_the_style() {
        let html = html_export("Notes <1>", DOC, &[("shot.png".into(), vec![1, 2, 3])]);
        assert!(html.contains("<title>Notes &lt;1&gt;</title>"));
        assert!(html.contains("<h1>Release notes</h1>"));
        assert!(html.contains("<strong>bold</strong>"));
        assert!(html.contains("<code>code</code>"));
        assert!(html.contains("<li>two</li>"));
        assert!(html.contains("<figure class=\"diagram\"><svg"));
        assert!(!html.contains("A --&gt; B"));
        assert!(html.contains("<th class=\"md-num\">b</th>"));
        assert!(html.contains("<img src=\"data:image/png;base64,"));
        assert!(html.contains("max-width: 760px"));
    }

    #[test]
    fn raw_html_and_script_links_are_not_passed_through() {
        let html = html_export("t", "<script>x</script> [a](javascript:alert(1))", &[]);
        assert!(!html.contains("<script>x"));
        assert!(!html.contains("href=\"javascript"));
    }

    #[test]
    fn an_unparsable_diagram_keeps_its_source_and_the_reason() {
        let html = html_export("t", "```mermaid\nnot a <diagram>\n```\n", &[]);
        assert!(html.contains("<pre class=\"mermaid-source\">not a &lt;diagram&gt;</pre>"));
        assert!(html.contains("<p class=\"diagram-error\">"));
        assert!(!html.contains("<figure class=\"diagram\">"));
    }

    #[test]
    fn diagram_labels_are_escaped_in_the_svg() {
        let html = html_export(
            "t",
            "```mermaid\nflowchart TD\n  A[\"<script>alert(1)</script> & <b onload=x>\"] --> B\n```\n",
            &[],
        );
        assert!(html.contains("<figure class=\"diagram\">"), "{html}");
        assert!(!html.contains("<script>alert"));
        // The parser drops the tags; what is left is escaped by `to_svg`.
        assert!(html.contains("&amp; onload=x&gt;"));
        assert!(!html.contains("<b onload"));
    }

    #[test]
    fn the_pdf_page_is_the_web_page_with_a_print_script() {
        let html = html_export("t", DOC, &[]);
        let pdf = pdf_export("t", DOC, &[]);
        assert!(pdf.contains("window.print()"));
        assert!(!html.contains("window.print"));
        assert_eq!(
            pdf.replacen(
                "<script>window.addEventListener('load', () => window.print());</script>",
                "",
                1
            ),
            html
        );
        assert_eq!(export_file_name("Plan", ExportFormat::Pdf), "plan.html");
    }

    #[test]
    fn the_file_url_encodes_what_a_url_cannot_hold() {
        let url = file_url(std::path::Path::new("/tmp/a b/c#1%?.html"));
        assert_eq!(url, "file:///tmp/a%20b/c%231%25%3F.html");
    }

    /// Writes a sample page to open in a browser by hand.
    #[test]
    #[ignore = "writes a sample file; set EXPORT_SAMPLE_OUT"]
    fn write_a_sample_page() {
        let Ok(out) = std::env::var("EXPORT_SAMPLE_OUT") else {
            return;
        };
        let png: Vec<u8> = vec![
            0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00,
            0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78,
            0x9c, 0x63, 0xf8, 0xcf, 0xc0, 0xf0, 0x1f, 0x00, 0x05, 0x00, 0x01, 0xff, 0x89, 0x99,
            0x3d, 0x1d, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
        ];
        let html = html_export("Sample", DOC, &[("shot.png".into(), png)]);
        std::fs::write(out, html).unwrap();
    }

    #[test]
    fn the_markdown_export_inlines_the_images() {
        let md = markdown_export("![a](assets/a.png)\n", &[("a.png".into(), vec![9])]);
        assert!(md.contains("data:image/png;base64,"));
        assert!(!md.contains("assets/a.png"));
    }
}
