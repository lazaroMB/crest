//! Content in other hosts' formats drawn with crest's blocks, as pure
//! mappings: A2UI message streams ([`a2ui`]) and Adaptive Cards
//! ([`adaptive`]) onto `Custom` nodes and forms ([`card`]), and Jupyter
//! MIME bundles ([`mime`], with simple Vega-Lite charts) onto the block of
//! their richest media type.
//!
//! Every mapping is strict and bounded: payload size, node count, nesting
//! and list lengths are capped ([`card::MAX_PAYLOAD_BYTES`] and friends),
//! nothing is fetched (remote images become links the user may click),
//! no script or HTML runs, and what cannot be drawn becomes a placeholder
//! with its raw JSON instead of being dropped.

pub mod a2ui;
pub mod adaptive;
pub mod card;
pub mod mime;
mod vega;

pub use card::{Card, CardError, CardFormat, plain_text};

use std::collections::BTreeMap;

use domain::block::{BlockKind, CustomNode};
use domain::interop::INTEROP_PLUGIN;
use serde_json::Value;

/// The card formats crest reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CardKind {
    /// A2UI v0.9 messages (v0.8 read too).
    A2ui,
    /// An Adaptive Card, schema 1.0 to 1.6.
    AdaptiveCard,
}

/// A card mapped from any format, with what answering it needs.
#[derive(Debug, Clone, PartialEq)]
pub enum Mapped {
    /// From A2UI.
    A2ui(a2ui::Rendered),
    /// From an Adaptive Card.
    AdaptiveCard(adaptive::Rendered),
}

impl Mapped {
    /// The card.
    #[must_use]
    pub fn card(&self) -> &Card {
        match self {
            Self::A2ui(r) => &r.card,
            Self::AdaptiveCard(r) => &r.card,
        }
    }

    /// Checks what the fields cannot (date and time formats, patterns).
    ///
    /// # Errors
    ///
    /// What is wrong with the first bad value.
    pub fn check(
        &self,
        answer: &BTreeMap<String, Value>,
        action: Option<&str>,
    ) -> Result<(), String> {
        match self {
            Self::A2ui(r) => r.check(answer, action),
            Self::AdaptiveCard(r) => r.check(answer, action),
        }
    }

    /// What goes back to the sender for `answer` submitted with `action`,
    /// in the card's own format.
    #[must_use]
    pub fn response(&self, answer: &BTreeMap<String, Value>, action: Option<&str>) -> Value {
        match self {
            Self::A2ui(r) => r.response(answer, action),
            Self::AdaptiveCard(r) => r.response(answer, action),
        }
    }
}

/// Maps `payload` in `kind`; see [`a2ui::map`] and [`adaptive::map`].
///
/// # Errors
///
/// The [`CardError`] of the format's mapping.
pub fn map_card(kind: CardKind, payload: &Value, interactive: bool) -> Result<Mapped, CardError> {
    match kind {
        CardKind::A2ui => a2ui::map(payload, interactive).map(Mapped::A2ui),
        CardKind::AdaptiveCard => adaptive::map(payload, interactive).map(Mapped::AdaptiveCard),
    }
}

/// `tree` as a `Custom` block of [`INTEROP_PLUGIN`] named `name`.
#[must_use]
pub fn block(name: impl Into<String>, tree: CustomNode) -> BlockKind {
    BlockKind::Custom {
        plugin: INTEROP_PLUGIN.to_owned(),
        name: name.into(),
        tree,
    }
}
