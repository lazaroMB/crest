//! Modal windows that plugins open over the crest window.
//!
//! A [`ModalSpec`] says what a modal shows (a [`CustomNode`] tree), how big
//! it is (a [`Percent`] of the window per axis) and where it sits
//! ([`ModalPosition`]). [`modal_bounds`] turns that into a rectangle for a
//! given window size:
//!
//! - a [`MODAL_MARGIN`] (20 px) is always kept free on every side, so even
//!   `100%` never covers the window;
//! - the size is the percentage of the room inside that margin;
//! - an anchor pins the modal to the margin of its side (`top`, `bottom`,
//!   `left`, `right`, or two of them such as `top-right`); without one the
//!   modal is centered on that axis.
//!
//! ```text
//! ┌──────────────── window ────────────────┐
//! │ 20 px                                  │
//! │   ┌──────── available ───────────┐     │
//! │   │                  ┌─────────┐ │     │
//! │   │                  │top-right│ │     │
//! │   │                  └─────────┘ │     │
//! │   │        ┌────────┐            │     │
//! │   │        │ center │            │     │
//! │   │        └────────┘            │     │
//! │   └──────────────────────────────┘     │
//! │                                  20 px │
//! └────────────────────────────────────────┘
//! ```

use std::fmt;
use std::str::FromStr;

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::block::CustomNode;

/// Space kept free between a modal and every edge of the window, in pixels.
pub const MODAL_MARGIN: f32 = 20.0;

/// Size of a modal when the plugin gives none, in percent.
pub const DEFAULT_MODAL_PERCENT: f32 = 70.0;

/// Identifier of an open modal, unique per plugin (the app scopes it as
/// `plugin/id`).
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(transparent)]
pub struct ModalId(pub String);

impl ModalId {
    /// An id from any string.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// The id as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ModalId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Why a size or a position is not valid.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ModalError {
    /// A percentage outside `(0, 100]`, or text that is not one.
    #[error("invalid size {0:?}: use a number or \"NN%\" greater than 0 and at most 100")]
    Percent(String),
    /// A position that is not `center` or a combination of anchors.
    #[error(
        "unknown position {0:?}: use center, top, bottom, left, right or a combination \
         such as top-right"
    )]
    UnknownPosition(String),
    /// Two anchors of the same axis (`left-right`) or a repeated one.
    #[error("contradictory position {0:?}: use at most one of top/bottom and one of left/right")]
    ContradictoryPosition(String),
}

/// A share of the room available to a modal, greater than 0 and at most
/// 100.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, JsonSchema)]
#[schemars(with = "f32")]
pub struct Percent(f32);

impl Percent {
    /// The whole room.
    pub const FULL: Self = Self(100.0);

    /// `value` percent.
    ///
    /// # Errors
    ///
    /// [`ModalError::Percent`] unless `0 < value <= 100`.
    pub fn new(value: f32) -> Result<Self, ModalError> {
        if value.is_finite() && value > 0.0 && value <= 100.0 {
            Ok(Self(value))
        } else {
            Err(ModalError::Percent(value.to_string()))
        }
    }

    /// The percentage, in `(0, 100]`.
    #[must_use]
    pub fn get(self) -> f32 {
        self.0
    }

    /// The fraction, in `(0, 1]`.
    #[must_use]
    pub fn fraction(self) -> f32 {
        self.0 / 100.0
    }
}

impl Default for Percent {
    fn default() -> Self {
        Self(DEFAULT_MODAL_PERCENT)
    }
}

impl FromStr for Percent {
    type Err = ModalError;

    /// Parses `"60"`, `"60%"` or `" 60.5 % "`.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let text = s.trim();
        let number = text.strip_suffix('%').unwrap_or(text).trim();
        let value: f32 = number
            .parse()
            .map_err(|_| ModalError::Percent(s.to_owned()))?;
        Self::new(value).map_err(|_| ModalError::Percent(s.to_owned()))
    }
}

impl Serialize for Percent {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_f32(self.0)
    }
}

impl<'de> Deserialize<'de> for Percent {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = f32::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

/// Where a modal sits on the vertical axis.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum VerticalAnchor {
    /// Pinned to the top margin.
    Top,
    /// Centered.
    #[default]
    Center,
    /// Pinned to the bottom margin.
    Bottom,
}

/// Where a modal sits on the horizontal axis.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum HorizontalAnchor {
    /// Pinned to the left margin.
    Left,
    /// Centered.
    #[default]
    Center,
    /// Pinned to the right margin.
    Right,
}

/// Placement of a modal: one anchor per axis. Written as `center` or as
/// anchors joined by `-` (`top`, `bottom-left`, `right-top`...); it is
/// shown vertical first (`top-right`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, JsonSchema)]
#[schemars(with = "String")]
pub struct ModalPosition {
    /// Vertical anchor.
    pub vertical: VerticalAnchor,
    /// Horizontal anchor.
    pub horizontal: HorizontalAnchor,
}

impl ModalPosition {
    /// Centered on both axes (the default).
    pub const CENTER: Self = Self {
        vertical: VerticalAnchor::Center,
        horizontal: HorizontalAnchor::Center,
    };
}

impl FromStr for ModalPosition {
    type Err = ModalError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let text = s.trim().to_ascii_lowercase();
        if text == "center" || text == "centre" {
            return Ok(Self::CENTER);
        }
        let mut vertical = None;
        let mut horizontal = None;
        for part in text.split(['-', ' ', '_']).filter(|p| !p.is_empty()) {
            let contradiction = || ModalError::ContradictoryPosition(s.to_owned());
            match part {
                "top" | "bottom" => {
                    if vertical.is_some() {
                        return Err(contradiction());
                    }
                    vertical = Some(if part == "top" {
                        VerticalAnchor::Top
                    } else {
                        VerticalAnchor::Bottom
                    });
                }
                "left" | "right" => {
                    if horizontal.is_some() {
                        return Err(contradiction());
                    }
                    horizontal = Some(if part == "left" {
                        HorizontalAnchor::Left
                    } else {
                        HorizontalAnchor::Right
                    });
                }
                _ => return Err(ModalError::UnknownPosition(s.to_owned())),
            }
        }
        if vertical.is_none() && horizontal.is_none() {
            return Err(ModalError::UnknownPosition(s.to_owned()));
        }
        Ok(Self {
            vertical: vertical.unwrap_or_default(),
            horizontal: horizontal.unwrap_or_default(),
        })
    }
}

impl fmt::Display for ModalPosition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let vertical = match self.vertical {
            VerticalAnchor::Top => Some("top"),
            VerticalAnchor::Center => None,
            VerticalAnchor::Bottom => Some("bottom"),
        };
        let horizontal = match self.horizontal {
            HorizontalAnchor::Left => Some("left"),
            HorizontalAnchor::Center => None,
            HorizontalAnchor::Right => Some("right"),
        };
        match (vertical, horizontal) {
            (None, None) => f.write_str("center"),
            (Some(v), None) => f.write_str(v),
            (None, Some(h)) => f.write_str(h),
            (Some(v), Some(h)) => write!(f, "{v}-{h}"),
        }
    }
}

impl Serialize for ModalPosition {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for ModalPosition {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

/// A modal as a plugin asked for it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ModalSpec {
    /// Its id; opening an id that is already open replaces that modal.
    pub id: ModalId,
    /// Plugin that opened it.
    pub plugin: String,
    /// Title in the header.
    pub title: String,
    /// Width, in percent of the room inside the margins.
    #[serde(default)]
    pub width: Percent,
    /// Height, in percent of the room inside the margins.
    #[serde(default)]
    pub height: Percent,
    /// Placement inside the margins.
    #[serde(default)]
    pub position: ModalPosition,
    /// What it shows; the body scrolls when this is larger.
    pub content: CustomNode,
}

/// A rectangle in window pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModalRect {
    /// Left edge.
    pub x: f32,
    /// Top edge.
    pub y: f32,
    /// Width.
    pub width: f32,
    /// Height.
    pub height: f32,
}

/// Start and length of a modal on one axis of `window` pixels.
fn place(window: f32, share: Percent, start: bool, end: bool) -> (f32, f32) {
    let available = (window - 2.0 * MODAL_MARGIN).max(0.0);
    let length = (share.fraction() * available).clamp(0.0, available);
    let offset = if start {
        0.0
    } else if end {
        available - length
    } else {
        (available - length) / 2.0
    };
    (MODAL_MARGIN + offset, length)
}

/// Where `spec` is drawn in a window of `window_width` × `window_height`
/// pixels; see the [module docs](self) for the rules.
#[must_use]
pub fn modal_bounds(window_width: f32, window_height: f32, spec: &ModalSpec) -> ModalRect {
    let position = spec.position;
    let (x, width) = place(
        window_width,
        spec.width,
        position.horizontal == HorizontalAnchor::Left,
        position.horizontal == HorizontalAnchor::Right,
    );
    let (y, height) = place(
        window_height,
        spec.height,
        position.vertical == VerticalAnchor::Top,
        position.vertical == VerticalAnchor::Bottom,
    );
    ModalRect {
        x,
        y,
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(width: f32, height: f32, position: &str) -> ModalSpec {
        ModalSpec {
            id: ModalId::new("m"),
            plugin: "p".into(),
            title: "t".into(),
            width: Percent::new(width).unwrap(),
            height: Percent::new(height).unwrap(),
            position: position.parse().unwrap(),
            content: CustomNode::Text { text: "x".into() },
        }
    }

    fn rect(x: f32, y: f32, width: f32, height: f32) -> ModalRect {
        ModalRect {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn full_size_keeps_a_20_px_margin_on_every_side() {
        for (w, h) in [
            (1280.0, 800.0),
            (800.0, 600.0),
            (3840.0, 2160.0),
            (41.0, 41.0),
        ] {
            for position in ["center", "top-left", "bottom-right", "top", "right"] {
                let r = modal_bounds(w, h, &spec(100.0, 100.0, position));
                assert_eq!(
                    r,
                    rect(20.0, 20.0, w - 40.0, h - 40.0),
                    "{w}x{h} {position}"
                );
            }
        }
    }

    #[test]
    fn sizes_are_shares_of_the_room_inside_the_margins() {
        let r = modal_bounds(1040.0, 840.0, &spec(50.0, 25.0, "center"));
        assert_eq!(r, rect(270.0, 320.0, 500.0, 200.0));
    }

    #[test]
    fn each_anchor_pins_its_side_to_the_margin() {
        // Room inside the margins: 1000 × 800; the modal is 500 × 200.
        let cases = [
            ("center", 270.0, 320.0),
            ("top", 270.0, 20.0),
            ("bottom", 270.0, 620.0),
            ("left", 20.0, 320.0),
            ("right", 520.0, 320.0),
            ("top-left", 20.0, 20.0),
            ("top-right", 520.0, 20.0),
            ("bottom-left", 20.0, 620.0),
            ("bottom-right", 520.0, 620.0),
            ("right-top", 520.0, 20.0),
            ("left bottom", 20.0, 620.0),
        ];
        for (position, x, y) in cases {
            let r = modal_bounds(1040.0, 840.0, &spec(50.0, 25.0, position));
            assert_eq!(r, rect(x, y, 500.0, 200.0), "{position}");
        }
    }

    #[test]
    fn tiny_windows_clamp_to_nothing_inside_the_margin() {
        let r = modal_bounds(30.0, 10.0, &spec(80.0, 80.0, "bottom-right"));
        assert_eq!(r, rect(20.0, 20.0, 0.0, 0.0));
    }

    #[test]
    fn positions_parse_and_print_canonically() {
        let cases = [
            ("center", "center"),
            ("CENTER", "center"),
            ("top", "top"),
            ("bottom", "bottom"),
            ("left", "left"),
            ("right", "right"),
            ("top-left", "top-left"),
            ("left-top", "top-left"),
            ("Bottom-Right", "bottom-right"),
            (" top right ", "top-right"),
        ];
        for (text, canonical) in cases {
            let position: ModalPosition = text.parse().unwrap();
            assert_eq!(position.to_string(), canonical, "{text}");
            assert_eq!(canonical.parse::<ModalPosition>().unwrap(), position);
        }
        assert_eq!(ModalPosition::default(), ModalPosition::CENTER);
    }

    #[test]
    fn contradictory_and_unknown_positions_are_rejected() {
        for text in ["left-right", "top-bottom", "top-top", "right-left-top"] {
            assert_eq!(
                text.parse::<ModalPosition>(),
                Err(ModalError::ContradictoryPosition(text.into())),
                "{text}"
            );
        }
        for text in ["", "middle", "top-center", "-", "upper-left"] {
            assert_eq!(
                text.parse::<ModalPosition>(),
                Err(ModalError::UnknownPosition(text.into())),
                "{text}"
            );
        }
    }

    #[test]
    fn percentages_parse_from_numbers_and_text() {
        assert_eq!("60".parse::<Percent>().unwrap().get(), 60.0);
        assert_eq!(" 60.5 % ".parse::<Percent>().unwrap().get(), 60.5);
        assert_eq!("100%".parse::<Percent>().unwrap(), Percent::FULL);
        for bad in ["0", "0%", "-5%", "101%", "abc", "%", "NaN"] {
            assert!(bad.parse::<Percent>().is_err(), "{bad}");
        }
        assert!(Percent::new(f32::INFINITY).is_err());
        assert_eq!(Percent::default().get(), DEFAULT_MODAL_PERCENT);
    }

    #[test]
    fn spec_roundtrips_through_json_with_defaults() {
        let spec = spec(60.0, 80.0, "top-right");
        let value = serde_json::to_value(&spec).unwrap();
        assert_eq!(value["position"], "top-right");
        assert_eq!(value["width"], 60.0);
        let back: ModalSpec = serde_json::from_value(value).unwrap();
        assert_eq!(back, spec);
        let minimal: ModalSpec = serde_json::from_value(serde_json::json!({
            "id": "a", "plugin": "p", "title": "t",
            "content": {"type": "text", "text": "x"}
        }))
        .unwrap();
        assert_eq!(minimal.width, Percent::default());
        assert_eq!(minimal.position, ModalPosition::CENTER);
        let bad = serde_json::from_value::<ModalSpec>(serde_json::json!({
            "id": "a", "plugin": "p", "title": "t", "width": 0,
            "content": {"type": "text", "text": "x"}
        }));
        assert!(bad.is_err());
    }
}
