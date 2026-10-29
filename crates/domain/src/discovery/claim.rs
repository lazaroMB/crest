//! Causal claims: `origin → destination`, a direction, a stated sign and a
//! verbatim span from a stored source.
//!
//! A claim is accepted only if its span is found, after whitespace
//! normalisation, in the text of the source it cites. A claim with an
//! empty span is an **open question**: it is listed, never drawn. The sign
//! of the edge comes from the direction; a stated sign that disagrees is a
//! rejection, not an edge with the wrong sign.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use super::event::SourceId;

/// Id of a claim (`CL3`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ClaimId(pub u32);

impl fmt::Display for ClaimId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CL{}", self.0)
    }
}

/// The four canonical directions of a causal claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    /// More origin, more destination.
    IncreasingIncreases,
    /// Less origin, less destination.
    DecreasingDecreases,
    /// More origin, less destination.
    IncreasingDecreases,
    /// Less origin, more destination.
    DecreasingIncreases,
}

impl Direction {
    /// The sign of the edge this direction implies.
    #[must_use]
    pub const fn sign(self) -> Sign {
        match self {
            Self::IncreasingIncreases | Self::DecreasingDecreases => Sign::Plus,
            Self::IncreasingDecreases | Self::DecreasingIncreases => Sign::Minus,
        }
    }

    /// The canonical phrase.
    #[must_use]
    pub const fn phrase(self) -> &'static str {
        match self {
            Self::IncreasingIncreases => "increasing origin increases destination",
            Self::DecreasingDecreases => "decreasing origin decreases destination",
            Self::IncreasingDecreases => "increasing origin decreases destination",
            Self::DecreasingIncreases => "decreasing origin increases destination",
        }
    }
}

impl fmt::Display for Direction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.phrase())
    }
}

/// Error of [`Direction::from_str`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("`{0}` is not one of the four canonical directions")]
pub struct UnknownDirection(pub String);

impl FromStr for Direction {
    type Err = UnknownDirection;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let key = normalize(s).to_lowercase();
        [
            Self::IncreasingIncreases,
            Self::DecreasingDecreases,
            Self::IncreasingDecreases,
            Self::DecreasingIncreases,
        ]
        .into_iter()
        .find(|d| d.phrase() == key)
        .ok_or_else(|| UnknownDirection(s.to_owned()))
    }
}

/// The sign of a causal edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sign {
    /// Same direction.
    Plus,
    /// Opposite direction.
    Minus,
}

impl Sign {
    /// Parses `+`, `-`, `−`, `positive`, `negative`.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "+" | "positive" | "plus" => Some(Self::Plus),
            "-" | "\u{2212}" | "\u{2013}" | "negative" | "minus" => Some(Self::Minus),
            _ => None,
        }
    }

    /// `+` or `−`.
    #[must_use]
    pub const fn symbol(self) -> &'static str {
        match self {
            Self::Plus => "+",
            Self::Minus => "\u{2212}",
        }
    }
}

/// A causal claim between two variables.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Claim {
    /// Its id.
    pub id: ClaimId,
    /// The variable that changes first.
    pub origin: String,
    /// The variable that follows.
    pub destination: String,
    /// How.
    pub direction: Direction,
    /// The sign the author wrote; must agree with the direction.
    pub stated_sign: Sign,
    /// Verbatim text from the source; empty for an open question.
    pub span: String,
    /// The source the span is from.
    pub source: SourceId,
}

/// How a claim stands against its source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum ClaimCheck {
    /// The span is in the source and the signs agree: an edge.
    Keep {
        /// The edge's sign.
        sign: Sign,
    },
    /// Empty span, or the source text is not available: an open question.
    Open {
        /// Why.
        why: String,
    },
    /// Not an edge and not a question: the claim is wrong as written.
    Reject {
        /// Why.
        why: String,
    },
}

/// Collapses whitespace runs into one space and trims.
#[must_use]
pub fn normalize(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// [`check_claim`] requiring `claim.origin` and `claim.destination` to be
/// names of `variables` (the confirmed `Variables` record), compared with
/// case and whitespace ignored. Claims that name the same variable in two
/// ways fragment the diagram, so the check refuses them.
#[must_use]
pub fn check_claim_with(
    claim: &Claim,
    source_text: Option<&str>,
    variables: &[String],
) -> ClaimCheck {
    let known: Vec<String> = variables
        .iter()
        .map(|v| normalize(v).to_lowercase())
        .collect();
    for (role, name) in [
        ("origin", &claim.origin),
        ("destination", &claim.destination),
    ] {
        if !known.contains(&normalize(name).to_lowercase()) {
            return ClaimCheck::Reject {
                why: format!("the {role} \"{name}\" is not a name of the `variables` record"),
            };
        }
    }
    check_claim(claim, source_text)
}

/// Checks `claim` against the stored text of its source (`None` when the
/// source could not be read).
#[must_use]
pub fn check_claim(claim: &Claim, source_text: Option<&str>) -> ClaimCheck {
    let sign = claim.direction.sign();
    if claim.stated_sign != sign {
        return ClaimCheck::Reject {
            why: format!(
                "the direction \"{}\" implies {} but the stated sign is {}",
                claim.direction,
                sign.symbol(),
                claim.stated_sign.symbol()
            ),
        };
    }
    let span = normalize(&claim.span);
    if span.is_empty() {
        return ClaimCheck::Open {
            why: "the claim has no span".to_owned(),
        };
    }
    let Some(text) = source_text else {
        return ClaimCheck::Open {
            why: format!("the text of {} is not available", claim.source),
        };
    };
    if normalize(text).contains(&span) {
        ClaimCheck::Keep { sign }
    } else {
        ClaimCheck::Reject {
            why: format!("the span is not found verbatim in {}", claim.source),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claim(direction: Direction, sign: Sign, span: &str) -> Claim {
        Claim {
            id: ClaimId(1),
            origin: "cache size".into(),
            destination: "lookup latency".into(),
            direction,
            stated_sign: sign,
            span: span.into(),
            source: SourceId(2),
        }
    }

    const SOURCE: &str =
        "Section 4.\n\nAs the cache   grows, lookups\nslow down because the table rehashes.";

    #[test]
    fn a_verbatim_span_keeps_the_claim_with_the_direction_sign() {
        let c = claim(
            Direction::IncreasingIncreases,
            Sign::Plus,
            "As the cache grows, lookups slow down",
        );
        assert_eq!(
            check_claim(&c, Some(SOURCE)),
            ClaimCheck::Keep { sign: Sign::Plus }
        );
    }

    #[test]
    fn whitespace_differences_do_not_matter_but_words_do() {
        let c = claim(
            Direction::IncreasingIncreases,
            Sign::Plus,
            "the   table\nrehashes",
        );
        assert!(matches!(
            check_claim(&c, Some(SOURCE)),
            ClaimCheck::Keep { .. }
        ));
        let c = claim(
            Direction::IncreasingIncreases,
            Sign::Plus,
            "the table re-hashes",
        );
        assert!(matches!(
            check_claim(&c, Some(SOURCE)),
            ClaimCheck::Reject { .. }
        ));
    }

    #[test]
    fn an_empty_span_is_an_open_question_not_a_rejection() {
        let c = claim(Direction::IncreasingDecreases, Sign::Minus, "   ");
        assert!(matches!(
            check_claim(&c, Some(SOURCE)),
            ClaimCheck::Open { .. }
        ));
    }

    #[test]
    fn a_missing_source_is_an_open_question() {
        let c = claim(
            Direction::IncreasingDecreases,
            Sign::Minus,
            "lookups slow down",
        );
        assert!(matches!(check_claim(&c, None), ClaimCheck::Open { .. }));
    }

    #[test]
    fn a_sign_that_disagrees_with_the_direction_is_rejected_first() {
        let c = claim(
            Direction::IncreasingDecreases,
            Sign::Plus,
            "lookups slow down",
        );
        let ClaimCheck::Reject { why } = check_claim(&c, Some(SOURCE)) else {
            panic!("expected a rejection");
        };
        assert!(why.contains("implies −"), "{why}");
    }

    #[test]
    fn directions_parse_from_their_phrases() {
        for d in [
            Direction::IncreasingIncreases,
            Direction::DecreasingDecreases,
            Direction::IncreasingDecreases,
            Direction::DecreasingIncreases,
        ] {
            assert_eq!(d.phrase().parse::<Direction>(), Ok(d));
            assert_eq!(d.phrase().to_uppercase().parse::<Direction>(), Ok(d));
        }
        assert!("more is more".parse::<Direction>().is_err());
        assert_eq!(Sign::parse("−"), Some(Sign::Minus));
        assert_eq!(Sign::parse("positive"), Some(Sign::Plus));
        assert_eq!(Sign::parse("?"), None);
    }
}
