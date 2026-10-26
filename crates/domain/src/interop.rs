//! Content in formats of other hosts, drawn with crest's own blocks.
//!
//! Tools and plugins may answer with a Jupyter MIME bundle (one value per
//! media type, richest first chosen by the app), an A2UI message stream or
//! an Adaptive Card. This module holds the media types crest recognises
//! and the [`MimeBundle`] itself; the mapping onto blocks is pure code in
//! the application layer (`application::interop`). Every block such a
//! mapping produces is a `Custom` block of plugin [`INTEROP_PLUGIN`] (or a
//! `Computed` card), so the headless mode and copy-as can tell them apart.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::block::BlockError;

/// Plugin name of the `Custom` blocks drawn from an interop format.
pub const INTEROP_PLUGIN: &str = "interop";

/// A2UI messages (one JSON object per message, or a list of them).
pub const A2UI: &str = "application/a2ui+json";
/// An Adaptive Card, as Bot Framework attachments name it.
pub const ADAPTIVE_CARD: &str = "application/vnd.microsoft.card.adaptive";
/// Adaptive Card, the JSON-suffixed name some hosts use.
pub const ADAPTIVE_CARD_JSON: &str = "application/vnd.microsoft.card.adaptive+json";
/// crest's `Computed` envelope with the body of its card.
pub const CREST_COMPUTED: &str = "application/vnd.crest.computed+json";
/// Vega-Lite 5 specification.
pub const VEGA_LITE_V5: &str = "application/vnd.vegalite.v5+json";
/// Vega-Lite 4 specification (read like 5 for the simple charts crest maps).
pub const VEGA_LITE_V4: &str = "application/vnd.vegalite.v4+json";
/// Markdown text.
pub const MARKDOWN: &str = "text/markdown";
/// LaTeX text.
pub const LATEX: &str = "text/latex";
/// HTML (never rendered as HTML).
pub const HTML: &str = "text/html";
/// Plain text.
pub const PLAIN: &str = "text/plain";
/// JSON.
pub const JSON: &str = "application/json";
/// PNG image (base64).
pub const PNG: &str = "image/png";
/// JPEG image (base64).
pub const JPEG: &str = "image/jpeg";
/// SVG image (XML text).
pub const SVG: &str = "image/svg+xml";

/// Largest bundle accepted, summing the JSON size of its values.
pub const MAX_BUNDLE_BYTES: usize = 16 * 1024 * 1024;
/// Most media types in one bundle.
pub const MAX_BUNDLE_TYPES: usize = 32;

/// A Jupyter MIME bundle: the same output in several media types, keyed
/// by type (`display_data`'s `data`), with per-type `metadata`.
///
/// Text types may be a string or a list of lines, as in notebooks.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MimeBundle {
    /// The output by media type.
    pub data: BTreeMap<String, Value>,
    /// Metadata by media type (e.g. an image's `width`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, Value>,
}

/// Whether `key` looks like a media type: `type/subtype`, lower-case
/// letters, digits and `.+-_` only, no parameters.
#[must_use]
pub fn is_media_type(key: &str) -> bool {
    let Some((kind, sub)) = key.split_once('/') else {
        return false;
    };
    let token = |s: &str| {
        !s.is_empty()
            && s.len() <= 127
            && s.chars().all(|c| {
                c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '+' | '-' | '_')
            })
    };
    token(kind) && token(sub)
}

/// A media type without its parameters, lower-cased (`Text/HTML;
/// charset=utf-8` reads `text/html`).
#[must_use]
pub fn essence(media_type: &str) -> String {
    media_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
}

impl MimeBundle {
    /// The bundle `value` holds: a `display_data`-like object with a `data`
    /// map of media types (and optional `metadata`), or a flat object whose
    /// keys are all media types. `None` for anything else.
    #[must_use]
    pub fn from_value(value: &Value) -> Option<Self> {
        let map = value.as_object()?;
        if let Some(Value::Object(data)) = map.get("data")
            && !data.is_empty()
            && data.keys().all(|k| is_media_type(k))
            && map.keys().all(|k| {
                matches!(
                    k.as_str(),
                    "data" | "metadata" | "output_type" | "transient"
                )
            })
        {
            let metadata = map
                .get("metadata")
                .and_then(Value::as_object)
                .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                .unwrap_or_default();
            return Some(Self {
                data: data.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
                metadata,
            });
        }
        (!map.is_empty() && map.keys().all(|k| is_media_type(k))).then(|| Self {
            data: map.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            metadata: BTreeMap::new(),
        })
    }

    /// The text of `media_type`: a string, or a list of strings joined as
    /// notebooks store multi-line text. `None` when absent or not text.
    #[must_use]
    pub fn text(&self, media_type: &str) -> Option<String> {
        match self.data.get(media_type)? {
            Value::String(s) => Some(s.clone()),
            Value::Array(lines) => lines
                .iter()
                .map(|line| line.as_str().map(str::to_owned))
                .collect::<Option<Vec<_>>>()
                .map(|lines| lines.concat()),
            _ => None,
        }
    }

    /// Checks that the bundle has between one and [`MAX_BUNDLE_TYPES`]
    /// media types, every key is one, and the values fit in
    /// [`MAX_BUNDLE_BYTES`].
    ///
    /// # Errors
    ///
    /// [`BlockError::Invalid`] naming the problem.
    pub fn validate(&self) -> Result<(), BlockError> {
        let invalid = |reason: String| BlockError::Invalid {
            kind: "mime bundle",
            reason,
        };
        if self.data.is_empty() {
            return Err(invalid("no media types".into()));
        }
        if self.data.len() > MAX_BUNDLE_TYPES {
            return Err(invalid(format!(
                "{} media types, at most {MAX_BUNDLE_TYPES}",
                self.data.len()
            )));
        }
        if let Some(bad) = self.data.keys().find(|k| !is_media_type(k)) {
            return Err(invalid(format!("{bad:?} is not a media type")));
        }
        let size: usize = self.data.values().map(json_size).sum();
        if size > MAX_BUNDLE_BYTES {
            return Err(invalid(format!("{size} bytes, at most {MAX_BUNDLE_BYTES}")));
        }
        Ok(())
    }
}

/// About how many bytes `value` takes as JSON (strings counted by length,
/// without escapes), without serialising it.
#[must_use]
pub fn json_size(value: &Value) -> usize {
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) => 8,
        Value::String(s) => s.len() + 2,
        Value::Array(items) => 2 + items.iter().map(|v| json_size(v) + 1).sum::<usize>(),
        Value::Object(map) => {
            2 + map
                .iter()
                .map(|(k, v)| k.len() + 4 + json_size(v))
                .sum::<usize>()
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn media_types_are_recognised() {
        for good in [PNG, SVG, VEGA_LITE_V5, CREST_COMPUTED, A2UI, ADAPTIVE_CARD] {
            assert!(is_media_type(good), "{good}");
        }
        for bad in [
            "text",
            "a/b/c",
            "Text/Plain",
            "text/plain; charset=utf-8",
            "/x",
            "",
        ] {
            assert!(!is_media_type(bad), "{bad}");
        }
        assert_eq!(essence("Text/HTML; charset=utf-8"), "text/html");
    }

    #[test]
    fn bundles_are_read_flat_or_as_display_data() {
        let flat =
            MimeBundle::from_value(&json!({"text/plain": "1", "text/markdown": ["# a\n", "b"]}))
                .unwrap();
        assert_eq!(flat.text(MARKDOWN).as_deref(), Some("# a\nb"));
        assert_eq!(flat.validate(), Ok(()));
        let display = MimeBundle::from_value(&json!({
            "output_type": "display_data",
            "data": {"image/png": "iVBO"},
            "metadata": {"image/png": {"width": 10}}
        }))
        .unwrap();
        assert_eq!(display.data.len(), 1);
        assert_eq!(display.metadata[PNG]["width"], 10);
        assert_eq!(display.text(PNG).as_deref(), Some("iVBO"));
        assert!(MimeBundle::from_value(&json!({"rows": [1, 2]})).is_none());
        assert!(MimeBundle::from_value(&json!({"text/plain": "a", "other": 1})).is_none());
        assert!(MimeBundle::from_value(&json!({})).is_none());
        assert!(MimeBundle::from_value(&json!([1])).is_none());
    }

    #[test]
    fn mime_bundle_schema() {
        insta::assert_json_snapshot!("mime_bundle_schema", schemars::schema_for!(MimeBundle));
    }

    #[test]
    fn oversized_or_odd_bundles_are_invalid() {
        assert!(MimeBundle::default().validate().is_err());
        let big = MimeBundle {
            data: BTreeMap::from([(PLAIN.to_owned(), json!("x".repeat(MAX_BUNDLE_BYTES)))]),
            metadata: BTreeMap::new(),
        };
        assert!(big.validate().is_err());
        let many = MimeBundle {
            data: (0..=MAX_BUNDLE_TYPES)
                .map(|i| (format!("text/x{i}"), json!("a")))
                .collect(),
            metadata: BTreeMap::new(),
        };
        assert!(many.validate().is_err());
        let bad_key = MimeBundle {
            data: BTreeMap::from([("Text".to_owned(), json!("a"))]),
            metadata: BTreeMap::new(),
        };
        assert!(bad_key.validate().is_err());
    }
}
