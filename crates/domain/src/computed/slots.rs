//! Number slots: how prose cites an engine value.
//!
//! The primary form is `⟦id.key⟧` (U+27E6, U+27E7): `id` is the
//! [`Computed`](super::Computed) envelope id and `key` one of its `values`.
//! An optional `calc:` prefix on the id is accepted and dropped, so
//! `⟦calc:7f3a.p_value⟧` names envelope `7f3a`. Models that cannot emit the
//! brackets may write the ASCII fallback `[[calc:id.key]]`, where the
//! `calc:` prefix is required so wiki-style `[[links]]` are left alone.
//!
//! Ids are letters, digits, `_` and `-` (no `.`); keys are the same plus
//! `.`, so a key like `coef.wt` works. The renderer replaces each slot with
//! the value's [`display_text`](super::ComputedValue::display_text); a slot
//! that names no known value stays as written so the user sees it.

use std::ops::Range;

use super::Computed;

/// Opening bracket of the primary form.
pub const OPEN: char = '⟦';
/// Closing bracket of the primary form.
pub const CLOSE: char = '⟧';
/// Opening of the ASCII fallback, including its required prefix.
pub const ASCII_OPEN: &str = "[[calc:";
/// Closing of the ASCII fallback.
pub const ASCII_CLOSE: &str = "]]";
/// Optional id prefix of the primary form.
const PREFIX: &str = "calc:";

/// Which way a slot was written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotForm {
    /// `⟦id.key⟧`.
    Brackets,
    /// `[[calc:id.key]]`.
    Ascii,
}

/// A slot found in a text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slot<'a> {
    /// Byte range of the whole slot, brackets included.
    pub range: Range<usize>,
    /// Envelope id, without the `calc:` prefix.
    pub id: &'a str,
    /// Value key.
    pub key: &'a str,
    /// How it was written.
    pub form: SlotForm,
}

/// Whether `id` can be a slot id: non-empty letters, digits, `_` or `-`.
#[must_use]
pub fn is_slot_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn is_slot_key(key: &str) -> bool {
    !key.is_empty()
        && !key.starts_with('.')
        && !key.ends_with('.')
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
}

/// The primary-form slot for `id` and `key`.
#[must_use]
pub fn slot(id: &str, key: &str) -> String {
    format!("{OPEN}{id}.{key}{CLOSE}")
}

/// Splits `id.key` at the first dot, if both halves are valid.
fn split_inner(inner: &str) -> Option<(&str, &str)> {
    let (id, key) = inner.split_once('.')?;
    (is_slot_id(id) && is_slot_key(key)).then_some((id, key))
}

/// Every well-formed slot in `text`, in order. Malformed ones are skipped.
#[must_use]
pub fn parse_slots(text: &str) -> Vec<Slot<'_>> {
    let mut slots = Vec::new();
    let mut pos = 0;
    while pos < text.len() {
        let rest = &text[pos..];
        let next_brackets = rest.find(OPEN);
        let next_ascii = rest.find(ASCII_OPEN);
        let (start, form) = match (next_brackets, next_ascii) {
            (Some(b), Some(a)) if a < b => (pos + a, SlotForm::Ascii),
            (Some(b), _) => (pos + b, SlotForm::Brackets),
            (None, Some(a)) => (pos + a, SlotForm::Ascii),
            (None, None) => break,
        };
        let (open_len, close) = match form {
            SlotForm::Brackets => (OPEN.len_utf8(), CLOSE.to_string()),
            SlotForm::Ascii => (ASCII_OPEN.len(), ASCII_CLOSE.to_owned()),
        };
        let body_start = start + open_len;
        let found = text[body_start..].find(close.as_str()).and_then(|len| {
            let inner = &text[body_start..body_start + len];
            let inner = match form {
                SlotForm::Brackets => inner.strip_prefix(PREFIX).unwrap_or(inner),
                SlotForm::Ascii => inner,
            };
            split_inner(inner).map(|(id, key)| (id, key, body_start + len + close.len()))
        });
        match found {
            Some((id, key, end)) => {
                slots.push(Slot {
                    range: start..end,
                    id,
                    key,
                    form,
                });
                pos = end;
            }
            None => pos = body_start,
        }
    }
    slots
}

/// `text` with each slot replaced by `resolve(slot)`; a slot for which it
/// returns `None` is kept as written.
pub fn replace_slots(text: &str, mut resolve: impl FnMut(&Slot<'_>) -> Option<String>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for slot in parse_slots(text) {
        if let Some(value) = resolve(&slot) {
            out.push_str(&text[last..slot.range.start]);
            out.push_str(&value);
            last = slot.range.end;
        }
    }
    out.push_str(&text[last..]);
    out
}

/// `text` with each slot that names a value of one of `envelopes` replaced
/// by that value's display text.
#[must_use]
pub fn resolve_slots<'a>(text: &str, envelopes: impl IntoIterator<Item = &'a Computed>) -> String {
    let envelopes: Vec<&Computed> = envelopes.into_iter().collect();
    replace_slots(text, |slot| {
        envelopes
            .iter()
            .find(|e| e.id == slot.id)
            .and_then(|e| e.value(slot.key))
            .map(super::ComputedValue::display_text)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::computed::{ComputedValue, Engine, ValueFormat};

    #[test]
    fn parses_both_forms_and_the_optional_prefix() {
        let text = "t = ⟦7f3a.t⟧, p ⟦calc:7f3a.p_value⟧ and [[calc:b-2.coef.wt]].";
        let slots = parse_slots(text);
        let found: Vec<_> = slots.iter().map(|s| (s.id, s.key, s.form)).collect();
        assert_eq!(
            found,
            [
                ("7f3a", "t", SlotForm::Brackets),
                ("7f3a", "p_value", SlotForm::Brackets),
                ("b-2", "coef.wt", SlotForm::Ascii),
            ]
        );
        assert_eq!(&text[slots[0].range.clone()], "⟦7f3a.t⟧");
        assert_eq!(&text[slots[2].range.clone()], "[[calc:b-2.coef.wt]]");
    }

    #[test]
    fn malformed_slots_are_skipped() {
        for text in [
            "⟦nokey⟧",
            "⟦a.⟧",
            "⟦.k⟧",
            "⟦a b.k⟧",
            "⟦a.k",
            "[[a.k]]",
            "[[calc:a]]",
        ] {
            assert!(parse_slots(text).is_empty(), "{text}");
        }
        // A broken slot does not hide a good one after it.
        let slots = parse_slots("⟦broken ⟦a.k⟧");
        assert_eq!(slots.len(), 1);
        assert_eq!(slots[0].key, "k");
    }

    #[test]
    fn replaces_known_slots_and_keeps_unknown_ones() {
        let text = "a ⟦x.k⟧ b [[calc:y.k]] c";
        let out = replace_slots(text, |s| (s.id == "x").then(|| "1.5".to_owned()));
        assert_eq!(out, "a 1.5 b [[calc:y.k]] c");
        assert_eq!(slot("x", "k"), "⟦x.k⟧");
    }

    #[test]
    fn resolves_against_envelopes() {
        let mut c = Computed::new(
            "c1",
            "stats.ttest_welch",
            "Welch",
            Engine {
                name: "e".into(),
                version: "1".into(),
                backend: None,
                platform: None,
            },
        );
        c.values.insert(
            "p_value".into(),
            ComputedValue {
                fmt: Some(ValueFormat::Apa),
                ..ComputedValue::new(0.0004)
            },
        );
        c.values.insert("t".into(), ComputedValue::new(-3.767));
        assert_eq!(
            resolve_slots("t = ⟦c1.t⟧, p ⟦c1.p_value⟧, ⟦c1.nope⟧", [&c]),
            "t = −3.77, p < .001, ⟦c1.nope⟧"
        );
    }
}
