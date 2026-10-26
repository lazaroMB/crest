//! Jupyter MIME bundles drawn with crest's blocks.
//!
//! [`render`] picks the richest media type of a [`MimeBundle`] that crest
//! can draw, in the order of [`PREFERENCE`]: crest's own `Computed` card,
//! Vega-Lite (a chart or plot when the spec is simple), A2UI and Adaptive
//! Cards (read-only), images, markdown, LaTeX (as display math), HTML
//! (never rendered: shown as source), JSON (a tree) and plain text. A type
//! that fails (an invalid envelope, a composite Vega-Lite view) falls
//! through to the next one, with a note saying why; a bundle with only
//! types crest does not know shows a placeholder.
//!
//! A `Computed` envelope from a tool is not crest's engine's work: its
//! verification is cleared and a warning says where it came from, so the
//! card's Recompute (for crest's own engines) is what verifies it.

use base64::Engine as _;
use domain::block::{BlockKind, CustomNode, ImageData, ImageFit};
use domain::computed::{Computed, ComputedBody, ComputedWarning, Verification, WarningSeverity};
use domain::interop::{self as types, MimeBundle};
use serde_json::Value;

use super::card::{CardError, MAX_IMAGE_BYTES, placeholder, svg_is_self_contained};
use super::{a2ui, adaptive, vega};

/// Media types crest draws, richest first.
pub const PREFERENCE: [&str; 15] = [
    types::CREST_COMPUTED,
    types::VEGA_LITE_V5,
    types::VEGA_LITE_V4,
    types::A2UI,
    types::ADAPTIVE_CARD,
    types::ADAPTIVE_CARD_JSON,
    types::PNG,
    types::JPEG,
    types::SVG,
    types::MARKDOWN,
    types::LATEX,
    types::HTML,
    types::JSON,
    types::PLAIN,
    "image/gif",
];

/// Longest text drawn from one media type, in bytes.
pub const MAX_DOCUMENT: usize = 512 * 1024;

/// Code of the warning on a `Computed` card that came from a tool.
pub const EXTERNAL_WARNING: &str = "external_source";

/// What a bundle is drawn as.
#[derive(Debug, Clone, PartialEq)]
pub struct MimeView {
    /// The media type drawn.
    pub media_type: String,
    /// The block: a `Computed` card, or a `Custom` node tree.
    pub kind: Drawn,
    /// Why richer types were passed over.
    pub notes: Vec<String>,
}

/// The drawing of a [`MimeView`].
#[derive(Debug, Clone, PartialEq)]
pub enum Drawn {
    /// crest's card for a `Computed` envelope.
    Computed {
        /// The envelope, marked as coming from a tool.
        envelope: Box<Computed>,
        /// The card body.
        body: Box<ComputedBody>,
    },
    /// Anything else.
    Node(CustomNode),
}

impl MimeView {
    /// The block to show, named after the type and `origin` for a node
    /// tree (with the notes under it).
    #[must_use]
    pub fn block(&self, origin: &str) -> BlockKind {
        match &self.kind {
            Drawn::Computed { envelope, body } => BlockKind::Computed {
                envelope: envelope.clone(),
                body: body.clone(),
            },
            Drawn::Node(node) => super::block(
                format!("{} · {origin}", self.media_type),
                with_notes(node.clone(), &self.notes),
            ),
        }
    }

    /// The drawing as one node (a `Computed` card becomes a table of its
    /// values), for embedding in a larger tree.
    #[must_use]
    pub fn node(&self) -> CustomNode {
        match &self.kind {
            Drawn::Computed { envelope, .. } => values_table(envelope),
            Drawn::Node(node) => with_notes(node.clone(), &self.notes),
        }
    }
}

fn with_notes(node: CustomNode, notes: &[String]) -> CustomNode {
    if notes.is_empty() {
        return node;
    }
    CustomNode::Col {
        children: vec![
            node,
            CustomNode::List {
                items: notes
                    .iter()
                    .map(|n| CustomNode::Text { text: n.clone() })
                    .collect(),
            },
        ],
    }
}

/// The values of an envelope as a table (key, value, unit).
fn values_table(envelope: &Computed) -> CustomNode {
    use domain::block::{Cell, Column, TableSpec};
    CustomNode::Table(TableSpec {
        columns: vec![
            Column::new("Value"),
            Column::new("Result"),
            Column::new("Unit"),
        ],
        rows: envelope
            .values
            .iter()
            .map(|(key, value)| {
                vec![
                    Cell::Text(key.clone()),
                    Cell::Text(value.display.clone().unwrap_or_else(|| value.v.to_string())),
                    Cell::Text(value.unit.clone().unwrap_or_default()),
                ]
            })
            .collect(),
    })
}

/// Draws `bundle`, sent by `origin` (a tool name, for the `Computed`
/// warning).
///
/// # Errors
///
/// A [`CardError`] when the bundle is invalid (no types, too large).
pub fn render(bundle: &MimeBundle, origin: &str) -> Result<MimeView, CardError> {
    bundle
        .validate()
        .map_err(|e| CardError::new(e.to_string()))?;
    let mut notes = Vec::new();
    for media_type in PREFERENCE {
        let Some(value) = bundle.data.get(media_type) else {
            continue;
        };
        match draw(media_type, value, bundle, origin) {
            Ok(kind) => {
                return Ok(MimeView {
                    media_type: media_type.to_owned(),
                    kind,
                    notes,
                });
            }
            Err(why) => notes.push(format!("{media_type} not drawn: {why}")),
        }
    }
    let (media_type, value) = bundle
        .data
        .iter()
        .next()
        .ok_or_else(|| CardError::new("the bundle is empty"))?;
    notes.push("crest draws none of the bundle's media types".into());
    Ok(MimeView {
        media_type: media_type.clone(),
        kind: Drawn::Node(placeholder(
            format!("media type {media_type}"),
            Some(format!(
                "types in the bundle: {}",
                bundle.data.keys().cloned().collect::<Vec<_>>().join(", ")
            )),
            value,
        )),
        notes,
    })
}

/// `value` as text, for the text media types.
fn text(bundle: &MimeBundle, media_type: &str) -> Result<String, String> {
    let text = bundle.text(media_type).ok_or("the value is not text")?;
    if text.len() > MAX_DOCUMENT {
        let mut end = MAX_DOCUMENT;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        return Ok(format!(
            "{}\n\n… (cut at {} KiB)",
            &text[..end],
            MAX_DOCUMENT / 1024
        ));
    }
    Ok(text)
}

fn draw(
    media_type: &str,
    value: &Value,
    bundle: &MimeBundle,
    origin: &str,
) -> Result<Drawn, String> {
    let node = match media_type {
        types::CREST_COMPUTED => return computed(value, origin),
        types::VEGA_LITE_V5 | types::VEGA_LITE_V4 => vega::chart(value)?,
        types::A2UI => a2ui::map(value, false).map_err(|e| e.0)?.card.tree(),
        types::ADAPTIVE_CARD | types::ADAPTIVE_CARD_JSON => {
            adaptive::map(value, false).map_err(|e| e.0)?.card.tree()
        }
        types::PNG | types::JPEG | "image/gif" => {
            let data: String = text(bundle, media_type)?
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect();
            if data.len() > MAX_IMAGE_BYTES {
                return Err("the image is too large".into());
            }
            base64::engine::general_purpose::STANDARD
                .decode(&data)
                .map_err(|_| "the image is not base64")?;
            CustomNode::Image {
                source: ImageData::Base64 { data },
                media_type: Some(media_type.to_owned()),
                alt: None,
                fit: ImageFit::Contain,
            }
        }
        types::SVG => {
            let svg = text(bundle, media_type)?;
            if svg.len() > MAX_IMAGE_BYTES {
                return Err("the SVG is too large".into());
            }
            if !svg_is_self_contained(&svg) {
                return Err("the SVG links files or addresses outside itself".into());
            }
            CustomNode::Image {
                source: ImageData::Base64 {
                    data: base64::engine::general_purpose::STANDARD.encode(svg.as_bytes()),
                },
                media_type: Some(types::SVG.to_owned()),
                alt: None,
                fit: ImageFit::Contain,
            }
        }
        types::MARKDOWN => CustomNode::Markdown {
            source: text(bundle, media_type)?,
        },
        types::LATEX => {
            let tex = text(bundle, media_type)?;
            let trimmed = tex.trim();
            let delimited = trimmed.starts_with('$')
                || trimmed.starts_with("\\[")
                || trimmed.starts_with("\\(")
                || trimmed.starts_with("\\begin");
            CustomNode::Markdown {
                source: if delimited {
                    tex
                } else {
                    format!("$$\n{trimmed}\n$$")
                },
            }
        }
        types::HTML => CustomNode::Col {
            children: vec![
                CustomNode::Text {
                    text: "HTML output, shown as source (crest does not render HTML from tools)"
                        .into(),
                },
                CustomNode::Code {
                    lang: Some("html".into()),
                    source: text(bundle, media_type)?,
                },
            ],
        },
        types::JSON => CustomNode::Json {
            value: value.clone(),
            expand_depth: 2,
        },
        types::PLAIN => CustomNode::Code {
            lang: None,
            source: text(bundle, media_type)?,
        },
        _ => return Err("not drawn by crest".into()),
    };
    node.validate().map_err(|e| e.to_string())?;
    Ok(Drawn::Node(node))
}

/// A `Computed` envelope with its body (`{envelope, body}`, as the
/// `computed` block serialises), validated and marked as the tool's.
fn computed(value: &Value, origin: &str) -> Result<Drawn, String> {
    let envelope = value
        .get("envelope")
        .ok_or("expected {\"envelope\", \"body\"}")?;
    let body = value.get("body").ok_or("the envelope has no card body")?;
    let mut envelope: Computed =
        serde_json::from_value(envelope.clone()).map_err(|e| format!("invalid envelope: {e}"))?;
    let body: ComputedBody =
        serde_json::from_value(body.clone()).map_err(|e| format!("invalid body: {e}"))?;
    envelope.validate().map_err(|e| e.to_string())?;
    body.validate().map_err(|e| e.to_string())?;
    // A claimed match is the tool's word, not a recompute crest ran.
    envelope.verification = Verification::default();
    envelope.warnings.retain(|w| w.code != EXTERNAL_WARNING);
    envelope.warnings.insert(
        0,
        ComputedWarning {
            code: EXTERNAL_WARNING.into(),
            severity: WarningSeverity::Warn,
            text: format!(
                "Computed by {origin}, not by crest's engine (it names {:?}); Recompute checks it \
                 when that engine is crest's.",
                envelope.engine.name
            ),
        },
    );
    Ok(Drawn::Computed {
        envelope: Box::new(envelope),
        body: Box::new(body),
    })
}

#[cfg(test)]
mod tests;
