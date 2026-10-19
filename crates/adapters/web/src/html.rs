//! HTML to text: whole pages for `web_fetch` and small fragments (titles,
//! snippets) for the search backends.

use std::sync::LazyLock;

use application::ports::web::WebError;
use regex::Regex;

/// Columns of the text html2text wraps a page to.
const WIDTH: usize = 100;

/// Elements dropped before converting a page: page chrome, not content.
const CHROME: [&str; 9] = [
    "script", "style", "noscript", "svg", "nav", "header", "footer", "aside", "form",
];

#[allow(clippy::expect_used)] // static patterns, covered by the tests
static TITLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<title[^>]*>(.*?)</title>").expect("valid regex"));
#[allow(clippy::expect_used)]
static TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<[^>]*>").expect("valid regex"));
#[allow(clippy::expect_used)]
static CHROME_BLOCKS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    CHROME
        .iter()
        .map(|tag| Regex::new(&format!(r"(?is)<{tag}\b.*?</{tag}\s*>")).expect("valid regex"))
        .collect()
});
#[allow(clippy::expect_used)]
static BLANK_LINES: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\n[ \t]*(\n[ \t]*){2,}").expect("valid regex"));

/// Whether a body of `content_type` is HTML.
pub fn is_html(content_type: &str, body: &[u8]) -> bool {
    if content_type.contains("html") {
        return true;
    }
    if !content_type.is_empty() {
        return false;
    }
    let head = String::from_utf8_lossy(&body[..body.len().min(512)]).to_ascii_lowercase();
    let head = head.trim_start();
    head.starts_with("<!doctype html") || head.starts_with("<html")
}

/// Whether a body of `content_type` can be shown as it is.
pub fn is_text(content_type: &str) -> bool {
    content_type.starts_with("text/")
        || ["json", "xml", "javascript", "yaml", "toml", "csv"]
            .iter()
            .any(|kind| content_type.contains(kind))
}

/// The title and readable text of a page.
///
/// # Errors
///
/// [`WebError::Unsupported`] when the body is neither HTML nor text.
pub fn page_text(
    url: &str,
    content_type: &str,
    body: &[u8],
) -> Result<(Option<String>, String), WebError> {
    let raw = String::from_utf8_lossy(body);
    if is_html(content_type, body) {
        let title = TITLE
            .captures(&raw)
            .map(|c| fragment_text(&c[1]))
            .filter(|t| !t.is_empty());
        return Ok((title, html_to_text(&raw)));
    }
    if is_text(content_type) {
        return Ok((None, raw.into_owned()));
    }
    Err(WebError::Unsupported {
        url: url.to_owned(),
        content_type: if content_type.is_empty() {
            "of unknown type".to_owned()
        } else {
            content_type.to_owned()
        },
    })
}

/// A page's content as plain text, without its chrome.
pub fn html_to_text(html: &str) -> String {
    let mut html = html.to_owned();
    for block in CHROME_BLOCKS.iter() {
        html = block.replace_all(&html, "").into_owned();
    }
    let text = html2text::config::plain()
        .link_footnotes(false)
        .no_table_borders()
        .string_from_read(html.as_bytes(), WIDTH)
        .unwrap_or_else(|_| fragment_text(&html));
    let lines: Vec<&str> = text.lines().map(str::trim_end).collect();
    BLANK_LINES
        .replace_all(lines.join("\n").trim(), "\n\n")
        .into_owned()
}

/// A small HTML fragment as one line of text: tags dropped, entities
/// decoded, spaces collapsed.
pub fn fragment_text(html: &str) -> String {
    let text = TAG.replace_all(html, "");
    decode_entities(&text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Decodes the HTML entities of `text` (named ones of everyday text and
/// numeric ones).
pub fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        let entity = tail
            .find(';')
            .filter(|end| *end <= 10)
            .and_then(|end| decode_entity(&tail[1..end]).map(|c| (c, end)));
        match entity {
            Some((c, end)) => {
                out.push(c);
                rest = &tail[end + 1..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn decode_entity(name: &str) -> Option<char> {
    if let Some(num) = name.strip_prefix('#') {
        let code = match num.strip_prefix(['x', 'X']) {
            Some(hex) => u32::from_str_radix(hex, 16).ok()?,
            None => num.parse().ok()?,
        };
        return char::from_u32(code);
    }
    Some(match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => ' ',
        "ndash" => '–',
        "mdash" => '—',
        "hellip" => '…',
        "laquo" => '«',
        "raquo" => '»',
        "copy" => '©',
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = include_str!("../tests/fixtures/page.html");

    #[test]
    fn a_real_page_keeps_its_content_and_drops_its_chrome() {
        let (title, text) = page_text(
            "https://blog.rust-lang.org/2025/09/18/Rust-1.90.0/",
            "text/html; charset=utf-8",
            PAGE.as_bytes(),
        )
        .unwrap();
        assert_eq!(title.as_deref(), Some("Announcing Rust 1.90.0 | Rust Blog"));
        assert!(text.contains("Rust 1.90.0"), "{text}");
        assert!(text.contains("rustup update stable"), "{text}");
        assert!(
            !text.contains("<div") && !text.contains("<a "),
            "no tags left"
        );
        assert!(!text.contains("function("), "no scripts");
        assert!(!text.contains("\n\n\n"), "blank lines collapsed");
    }

    #[test]
    fn text_is_kept_and_binary_is_refused() {
        let (title, text) =
            page_text("https://a/x.json", "application/json", b"{\"a\":1}").unwrap();
        assert_eq!((title, text.as_str()), (None, "{\"a\":1}"));
        assert!(matches!(
            page_text("https://a/x.pdf", "application/pdf", b"%PDF-1.7"),
            Err(WebError::Unsupported { .. })
        ));
        assert!(is_html("", b"  <!DOCTYPE html><html>"));
    }

    #[test]
    fn fragments_lose_tags_and_entities() {
        assert_eq!(
            fragment_text("The <b>Rust</b> team &amp; you don&#x27;t\n  wait &copy; &bogus;"),
            "The Rust team & you don't wait © &bogus;"
        );
    }
}
