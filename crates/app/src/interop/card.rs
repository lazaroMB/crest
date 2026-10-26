//! What A2UI surfaces and Adaptive Cards map onto: a tree of nodes for
//! the static content, form fields for the inputs and form actions for the
//! buttons, plus the limits and helpers both mappings share.
//!
//! A card is mapped in one of two modes. Interactive, its inputs become
//! [`FormField`]s and its submit buttons [`FormAction`]s, to be shown as a
//! form whose answer goes back to whoever sent the card. Read-only (a card
//! in a tool result, which cannot be answered), inputs are drawn as their
//! label and current value and buttons as inert labels, with a note.

use domain::block::{CustomNode, ImageData, is_openable_url};
use domain::form::{FormAction, FormField, FormSpec, FormState};
use serde_json::Value;

/// Largest card accepted, as JSON.
pub const MAX_PAYLOAD_BYTES: usize = 1024 * 1024;
/// Most nodes drawn for one card; the rest is cut with a note.
pub const MAX_NODES: usize = 2000;
/// Deepest nesting followed.
pub const MAX_DEPTH: usize = 32;
/// Most inputs turned into fields.
pub const MAX_FIELDS: usize = 64;
/// Longest text kept from one string, in bytes.
pub const MAX_TEXT: usize = 64 * 1024;
/// Largest inline image, as base64 text.
pub const MAX_IMAGE_BYTES: usize = 4 * 1024 * 1024;
/// Most items a template (an A2UI list bound to an array) expands to.
pub const MAX_ITEMS: usize = 500;

/// Image types an inline `data:` URI may carry.
const IMAGE_TYPES: [&str; 5] = [
    "image/png",
    "image/jpeg",
    "image/gif",
    "image/webp",
    "image/svg+xml",
];

/// The format a card came in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardFormat {
    /// A2UI messages.
    A2ui,
    /// An Adaptive Card.
    AdaptiveCard,
}

impl CardFormat {
    /// Name shown to the user.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::A2ui => "A2UI",
            Self::AdaptiveCard => "Adaptive Card",
        }
    }
}

/// A card mapped onto crest's nodes.
#[derive(Debug, Clone, PartialEq)]
pub struct Card {
    /// The format it came in.
    pub format: CardFormat,
    /// Heading, when the card has an obvious one.
    pub title: Option<String>,
    /// The content: everything in read-only mode, the static part in
    /// interactive mode.
    pub body: CustomNode,
    /// The inputs (interactive mode only).
    pub fields: Vec<FormField>,
    /// The submit buttons (interactive mode only).
    pub actions: Vec<FormAction>,
    /// What was cut, changed or not drawn, for the user and the model.
    pub notes: Vec<String>,
    /// How many components (or elements) were read.
    pub components: usize,
}

/// Why a card could not be mapped at all.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct CardError(pub String);

impl CardError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl Card {
    /// Whether the card has something to submit.
    #[must_use]
    pub fn is_interactive(&self) -> bool {
        !self.actions.is_empty()
    }

    /// The content with the notes under it, as one node.
    #[must_use]
    pub fn tree(&self) -> CustomNode {
        if self.notes.is_empty() {
            return self.body.clone();
        }
        let mut children = vec![self.body.clone()];
        children.push(notes_node(&self.notes));
        CustomNode::Col { children }
    }

    /// The card as a pending form titled `title` (or the card's own
    /// heading, or the format's name).
    #[must_use]
    pub fn form(&self, title: Option<&str>) -> FormSpec {
        FormSpec {
            title: title
                .map(str::to_owned)
                .or_else(|| self.title.clone())
                .unwrap_or_else(|| self.format.label().to_owned()),
            description: None,
            fields: self.fields.clone(),
            submit_label: None,
            state: FormState::Pending,
            answer: None,
            reason: None,
            body: Some(Box::new(self.tree())),
            actions: self.actions.clone(),
            action: None,
        }
    }

    /// One line for the model: what was shown and what was not drawn.
    #[must_use]
    pub fn summary(&self) -> String {
        let tree = self.tree();
        let unsupported = tree.placeholder_count();
        let mut text = format!(
            "Shown to the user: {} with {} component{}",
            self.format.label(),
            self.components,
            if self.components == 1 { "" } else { "s" }
        );
        if !self.fields.is_empty() {
            text.push_str(&format!(", {} input(s)", self.fields.len()));
        }
        if unsupported > 0 {
            text.push_str(&format!(
                ", {unsupported} not drawn (shown as placeholders)"
            ));
        }
        text.push('.');
        for note in &self.notes {
            text.push_str("\n- ");
            text.push_str(note);
        }
        text
    }
}

/// The notes as a muted list under the card.
fn notes_node(notes: &[String]) -> CustomNode {
    CustomNode::Card {
        title: Some("Notes".into()),
        children: vec![CustomNode::List {
            items: notes
                .iter()
                .map(|n| CustomNode::Text { text: n.clone() })
                .collect(),
        }],
    }
}

/// `text` cut to [`MAX_TEXT`] bytes at a character boundary, with `…`.
#[must_use]
pub fn clip(text: &str) -> String {
    if text.len() <= MAX_TEXT {
        return text.to_owned();
    }
    let mut end = MAX_TEXT;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

/// A link node when `url` may be opened, else a text node saying why not.
#[must_use]
pub fn link(text: &str, url: &str) -> CustomNode {
    let text = if text.trim().is_empty() { url } else { text };
    if is_openable_url(url) {
        CustomNode::Link {
            text: clip(text),
            url: url.to_owned(),
        }
    } else {
        CustomNode::Text {
            text: clip(&format!(
                "{text} (link not opened: only http, https and mailto)"
            )),
        }
    }
}

/// An image node for `url`: inline bytes for a `data:` URI of an image
/// type within [`MAX_IMAGE_BYTES`]; a link the user may click for an
/// `http(s)` address (crest never fetches it by itself); a note otherwise.
#[must_use]
pub fn image(url: &str, alt: Option<&str>) -> CustomNode {
    let alt = alt.map(str::trim).filter(|a| !a.is_empty());
    if let Some(rest) = url.strip_prefix("data:") {
        let Some((meta, data)) = rest.split_once(',') else {
            return note_text("image", alt, "the data: URI has no data");
        };
        let mut parts = meta.split(';');
        let media_type = parts.next().unwrap_or_default().trim().to_ascii_lowercase();
        let base64 = parts.any(|p| p.trim().eq_ignore_ascii_case("base64"));
        if !IMAGE_TYPES.contains(&media_type.as_str()) {
            return note_text(
                "image",
                alt,
                &format!("{media_type:?} is not an image type"),
            );
        }
        if data.len() > MAX_IMAGE_BYTES {
            return note_text("image", alt, "the inline image is too large");
        }
        let data = if base64 {
            data.trim().to_owned()
        } else if media_type == "image/svg+xml" {
            base64_encode(&percent_decode(data))
        } else {
            return note_text("image", alt, "the data: URI is not base64");
        };
        if media_type == "image/svg+xml" {
            use base64::Engine as _;
            let svg = base64::engine::general_purpose::STANDARD
                .decode(&data)
                .ok()
                .and_then(|bytes| String::from_utf8(bytes).ok());
            if !svg.is_some_and(|svg| svg_is_self_contained(&svg)) {
                return note_text(
                    "image",
                    alt,
                    "the SVG links files or addresses outside itself",
                );
            }
        }
        return CustomNode::Image {
            source: ImageData::Base64 { data },
            media_type: Some(media_type),
            alt: alt.map(clip),
            fit: domain::block::ImageFit::Contain,
        };
    }
    let lower = url.to_ascii_lowercase();
    if lower.starts_with("https://") || lower.starts_with("http://") {
        let text = match alt {
            Some(alt) => format!("Image: {alt} (opens in the browser)"),
            None => "Image (opens in the browser)".to_owned(),
        };
        return link(&text, url);
    }
    note_text("image", alt, "only data: URIs and web addresses are shown")
}

fn note_text(what: &str, alt: Option<&str>, why: &str) -> CustomNode {
    CustomNode::Text {
        text: match alt {
            Some(alt) => format!("[{what} not shown: {alt}: {why}]"),
            None => format!("[{what} not shown: {why}]"),
        },
    }
}

/// Whether an SVG only refers to itself: every `href` (and `xlink:href`)
/// is a fragment (`#id`) or a `data:` URI, and no CSS `url(...)` or
/// `@import` points elsewhere. The SVG renderer would otherwise load
/// files named there.
#[must_use]
pub fn svg_is_self_contained(svg: &str) -> bool {
    let lower = svg.to_ascii_lowercase();
    let local = |value: &str| {
        let value = value.trim_start();
        value.is_empty() || value.starts_with('#') || value.starts_with("data:")
    };
    let mut rest = lower.as_str();
    while let Some(at) = rest.find("href") {
        let after = rest[at + 4..].trim_start();
        if let Some(after) = after.strip_prefix('=') {
            let after = after.trim_start();
            let value = after.trim_start_matches(['"', '\'']);
            if !local(value) {
                return false;
            }
        }
        rest = &rest[at + 4..];
    }
    let mut rest = lower.as_str();
    while let Some(at) = rest.find("url(") {
        let value = rest[at + 4..].trim_start().trim_start_matches(['"', '\'']);
        if !local(value) {
            return false;
        }
        rest = &rest[at + 4..];
    }
    !lower.contains("@import")
}

/// `%XX` escapes decoded (other bytes kept).
fn percent_decode(text: &str) -> Vec<u8> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Some(byte) = std::str::from_utf8(&bytes[i + 1..i + 3])
                .ok()
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(byte);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    out
}

/// Standard base64 with padding.
fn base64_encode(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// A labelled box for content that is not drawn, with its JSON.
#[must_use]
pub fn placeholder(label: String, note: Option<String>, raw: &Value) -> CustomNode {
    CustomNode::Placeholder {
        label,
        note,
        raw: shrink(raw),
    }
}

/// `raw` as kept in a placeholder: long strings clipped, so a huge payload
/// is not copied whole into the block.
fn shrink(raw: &Value) -> Value {
    const KEEP: usize = 4096;
    match raw {
        Value::String(s) if s.len() > KEEP => {
            let mut end = KEEP;
            while !s.is_char_boundary(end) {
                end -= 1;
            }
            Value::String(format!("{}… ({} bytes)", &s[..end], s.len()))
        }
        Value::Array(items) => Value::Array(items.iter().take(200).map(shrink).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .take(200)
                .map(|(k, v)| (k.clone(), shrink(v)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// The text of a node tree, one line per text-like node, for the
/// terminal and summaries.
#[must_use]
pub fn plain_text(node: &CustomNode) -> String {
    let mut lines = Vec::new();
    node.for_each(&mut |node| match node {
        CustomNode::Text { text } | CustomNode::Markdown { source: text } => {
            lines.push(text.clone());
        }
        CustomNode::Card {
            title: Some(title), ..
        } => lines.push(format!("## {title}")),
        CustomNode::Link { text, url } => lines.push(format!("{text} <{url}>")),
        CustomNode::Code { source, .. } => lines.push(source.clone()),
        CustomNode::Placeholder { label, .. } => lines.push(format!("[not drawn: {label}]")),
        CustomNode::Image { alt, .. } => {
            lines.push(format!(
                "[image{}]",
                alt.as_ref().map(|a| format!(": {a}")).unwrap_or_default()
            ));
        }
        CustomNode::Table(table) => lines.push(format!(
            "[table: {} columns, {} rows]",
            table.columns.len(),
            table.rows.len()
        )),
        _ => {}
    });
    lines.join("\n")
}

/// Counts nodes against [`MAX_NODES`] and remembers whether it ran out.
#[derive(Debug, Default)]
pub(crate) struct Budget {
    used: usize,
    pub(crate) exhausted: bool,
}

impl Budget {
    /// Takes one node; `false` once [`MAX_NODES`] are used.
    pub(crate) fn take(&mut self) -> bool {
        if self.used >= MAX_NODES {
            self.exhausted = true;
            return false;
        }
        self.used += 1;
        true
    }
}

/// A field name not in `taken`, from `wanted` (non-empty, at most 64
/// characters of `[A-Za-z0-9_.-]`).
pub(crate) fn unique_name(wanted: &str, taken: &[FormField]) -> String {
    let mut base: String = wanted
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-') {
                c
            } else {
                '_'
            }
        })
        .take(64)
        .collect();
    let trimmed = base.trim_matches('_');
    base = if trimmed.is_empty() {
        "field".to_owned()
    } else {
        trimmed.to_owned()
    };
    if !taken.iter().any(|f| f.name == base) {
        return base;
    }
    (2..)
        .map(|n| format!("{base}_{n}"))
        .find(|name| !taken.iter().any(|f| &f.name == name))
        .unwrap_or(base)
}

/// Whether `text` is a date `YYYY-MM-DD`.
#[must_use]
pub fn is_date(text: &str) -> bool {
    let b = text.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
        && matches!(text[5..7].parse::<u8>(), Ok(1..=12))
        && matches!(text[8..10].parse::<u8>(), Ok(1..=31))
}

/// Whether `text` is a time `HH:MM` (or `HH:MM:SS`).
#[must_use]
pub fn is_time(text: &str) -> bool {
    let parts: Vec<&str> = text.split(':').collect();
    let two = |s: &str, max: u8| s.len() == 2 && s.parse::<u8>().is_ok_and(|v| v <= max);
    match parts.as_slice() {
        [h, m] => two(h, 23) && two(m, 59),
        [h, m, s] => two(h, 23) && two(m, 59) && two(s, 59),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn images_are_inline_only_from_data_uris() {
        let png = image("data:image/png;base64,iVBORw0KGgo=", Some("dot"));
        assert!(
            matches!(png, CustomNode::Image { media_type: Some(ref t), .. } if t == "image/png")
        );
        let svg = image("data:image/svg+xml,%3Csvg%2F%3E", None);
        let CustomNode::Image {
            source: ImageData::Base64 { data },
            ..
        } = svg
        else {
            panic!("{svg:?}")
        };
        assert_eq!(data, "PHN2Zy8+");
        assert!(matches!(
            image("https://x.test/a.png", None),
            CustomNode::Link { .. }
        ));
        for bad in [
            "data:text/html;base64,PGI+",
            "javascript:alert(1)",
            "file:///etc/passwd",
            "data:image/png,raw",
        ] {
            assert!(matches!(image(bad, None), CustomNode::Text { .. }), "{bad}");
        }
        let huge = format!("data:image/png;base64,{}", "A".repeat(MAX_IMAGE_BYTES + 1));
        assert!(matches!(image(&huge, None), CustomNode::Text { .. }));
    }

    #[test]
    fn svgs_must_not_reach_outside() {
        assert!(svg_is_self_contained(
            r##"<svg><use href="#a"/><image href="data:image/png;base64,AA"/></svg>"##
        ));
        assert!(svg_is_self_contained(
            r##"<svg><rect fill="url(#g)"/></svg>"##
        ));
        for bad in [
            r#"<svg><image href="/home/u/.ssh/id_rsa"/></svg>"#,
            r#"<svg><image xlink:href = 'file:///etc/passwd'/></svg>"#,
            r#"<svg><style>rect { fill: url(https://x.test/a) }</style></svg>"#,
            r#"<svg><style>@import "x.css";</style></svg>"#,
        ] {
            assert!(!svg_is_self_contained(bad), "{bad}");
        }
        assert!(matches!(
            image(
                "data:image/svg+xml,%3Csvg%3E%3Cimage%20href%3D%22%2Fetc%2Fx%22%2F%3E%3C%2Fsvg%3E",
                None
            ),
            CustomNode::Text { .. }
        ));
    }

    #[test]
    fn links_only_open_safe_schemes() {
        assert!(matches!(
            link("a", "https://a.test"),
            CustomNode::Link { .. }
        ));
        assert!(matches!(link("a", "javascript:x"), CustomNode::Text { .. }));
    }

    #[test]
    fn text_is_clipped_and_names_are_unique() {
        let long = "é".repeat(MAX_TEXT);
        assert!(clip(&long).len() <= MAX_TEXT + 3);
        let field = |name: &str| FormField {
            name: name.into(),
            label: None,
            description: None,
            required: false,
            kind: domain::form::FieldKind::Boolean { default: None },
        };
        assert_eq!(unique_name("/user/name", &[]), "user_name");
        assert_eq!(unique_name("a", &[field("a"), field("a_2")]), "a_3");
        assert_eq!(unique_name("///", &[]), "field");
    }

    #[test]
    fn dates_and_times_are_checked() {
        assert!(is_date("2026-10-26"));
        assert!(!is_date("2026-13-01"));
        assert!(!is_date("26-10-2026"));
        assert!(is_time("09:30"));
        assert!(is_time("23:59:59"));
        assert!(!is_time("24:00"));
    }

    #[test]
    fn placeholders_keep_a_clipped_copy() {
        let raw = json!({"big": "x".repeat(10_000)});
        let CustomNode::Placeholder { raw, .. } = placeholder("x".into(), None, &raw) else {
            unreachable!()
        };
        assert!(raw["big"].as_str().unwrap().len() < 5000);
    }
}
