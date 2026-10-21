//! How deep the fast pipeline digs: [`Depth::Quick`] for a small bug or
//! refactor, [`Depth::Standard`] for the rest.
//!
//! Triage picks it ([`choose`]) and stores it under [`DEPTH_KEY`]; the
//! later stages read it with
//! [`NodeContext::depth`](super::node::NodeContext::depth). A depth given
//! when the flow started (`/research --quick`, stored under
//! [`KEY_DEPTH`](super::gate::KEY_DEPTH)) wins over the rules.
//!
//! | Depth    | Discovery                                   | Agent turns | Hypotheses and Brainstorm |
//! | -------- | ------------------------------------------- | ----------- | ------------------------- |
//! | Quick    | repo map and history, no prior art, no web  | 12 calls    | main model                |
//! | Standard | map, history, prior art in the repo and web | 30 calls    | plan model                |

use serde::{Deserialize, Serialize};

use super::nodes::intake::{IntakeBrief, RequestKind, Size};

/// Flow data key of the [`Depth`] Triage chose.
pub const DEPTH_KEY: &str = "triage.depth";

/// How much work the fast pipeline does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Depth {
    /// A small, well-located change: fewer explorers and a cheaper model.
    Quick,
    /// Everything the fast pipeline has.
    #[default]
    Standard,
}

impl Depth {
    /// Name shown to the user (`quick` or `standard`).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Quick => "quick",
            Self::Standard => "standard",
        }
    }

    /// The depth named `text` (`quick`, `standard`; case ignored).
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_lowercase().as_str() {
            "quick" | "q" | "rapido" | "rápido" => Some(Self::Quick),
            "standard" | "normal" | "std" => Some(Self::Standard),
            _ => None,
        }
    }

    /// Most model calls of one agent turn of Discovery.
    #[must_use]
    pub const fn max_iterations(self) -> u32 {
        match self {
            Self::Quick => 12,
            Self::Standard => 30,
        }
    }
}

/// The depth Triage chose and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepthChoice {
    /// The depth.
    pub depth: Depth,
    /// Why, in a few words (e.g. "small bug").
    pub reason: String,
    /// Whether the request looks large or unmeasured enough that the deep
    /// pipeline may be worth it.
    pub suggest_deep: bool,
}

/// Picks the depth for `brief`:
///
/// 1. the depth asked for at the start (`requested`) wins;
/// 2. a small bug or refactor is [`Depth::Quick`];
/// 3. a large request, or a performance problem without numbers, is
///    [`Depth::Standard`] and suggests the deep pipeline;
/// 4. anything else is [`Depth::Standard`].
///
/// `problem` is the user's original request: with the brief, it tells
/// whether a performance problem comes with measurements.
#[must_use]
pub fn choose(requested: Option<&str>, brief: &IntakeBrief, problem: &str) -> DepthChoice {
    if let Some(depth) = requested.and_then(Depth::parse) {
        return DepthChoice {
            depth,
            reason: "asked for at the start".into(),
            suggest_deep: false,
        };
    }
    let kind = brief.kind.name().to_lowercase();
    match (brief.size, brief.kind) {
        (Some(Size::Small), RequestKind::Bug | RequestKind::Refactor) => DepthChoice {
            depth: Depth::Quick,
            reason: format!("small {kind}"),
            suggest_deep: false,
        },
        (Some(Size::Large), _) => DepthChoice {
            depth: Depth::Standard,
            reason: format!("large {kind}"),
            suggest_deep: true,
        },
        (_, RequestKind::Performance) if !has_numbers(brief, problem) => DepthChoice {
            depth: Depth::Standard,
            reason: "performance problem without measurements".into(),
            suggest_deep: true,
        },
        (size, _) => DepthChoice {
            depth: Depth::Standard,
            reason: match size {
                Some(size) => format!("{} {kind}", size.name()),
                None => kind,
            },
            suggest_deep: false,
        },
    }
}

/// Whether the request, its restatement or the user's answers carry a
/// number (a measurement such as "5 s" or "500 req/s").
fn has_numbers(brief: &IntakeBrief, problem: &str) -> bool {
    std::iter::once(problem)
        .chain(std::iter::once(brief.restated.as_str()))
        .chain(brief.answers.iter().map(String::as_str))
        .any(|text| text.chars().any(|c| c.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn brief(kind: RequestKind, size: Option<Size>, restated: &str) -> IntakeBrief {
        IntakeBrief {
            kind,
            restated: restated.into(),
            questions: vec![],
            answers: vec![],
            size,
            blocking: false,
        }
    }

    #[test]
    fn the_rules_pick_the_depth() {
        use RequestKind::{Bug, Feature, Performance, Refactor};
        use Size::{Large, Medium, Small};
        let cases = [
            (Bug, Some(Small), "x", Depth::Quick, false),
            (Refactor, Some(Small), "x", Depth::Quick, false),
            (Feature, Some(Small), "x", Depth::Standard, false),
            (Bug, Some(Medium), "x", Depth::Standard, false),
            (Bug, None, "x", Depth::Standard, false),
            (Feature, Some(Large), "x", Depth::Standard, true),
            (Bug, Some(Large), "x", Depth::Standard, true),
            (
                Performance,
                Some(Medium),
                "login is slow",
                Depth::Standard,
                true,
            ),
            (
                Performance,
                Some(Medium),
                "login takes 5 s",
                Depth::Standard,
                false,
            ),
            (
                Performance,
                Some(Small),
                "login takes 5 s",
                Depth::Standard,
                false,
            ),
        ];
        for (kind, size, restated, depth, deep) in cases {
            let choice = choose(None, &brief(kind, size, restated), "slow");
            assert_eq!(
                (choice.depth, choice.suggest_deep),
                (depth, deep),
                "{kind:?} {size:?} {restated}"
            );
        }
    }

    #[test]
    fn the_depth_asked_for_at_the_start_wins() {
        let large = brief(RequestKind::Feature, Some(Size::Large), "x");
        let choice = choose(Some("quick"), &large, "x");
        assert_eq!((choice.depth, choice.suggest_deep), (Depth::Quick, false));
        let small = brief(RequestKind::Bug, Some(Size::Small), "x");
        assert_eq!(choose(Some("standard"), &small, "x").depth, Depth::Standard);
        // An unknown value is ignored.
        assert_eq!(choose(Some("deep"), &small, "x").depth, Depth::Quick);
    }

    #[test]
    fn names_parse_back_and_limit_the_turns() {
        for depth in [Depth::Quick, Depth::Standard] {
            assert_eq!(Depth::parse(depth.name()), Some(depth));
        }
        assert_eq!(Depth::default(), Depth::Standard);
        assert!(Depth::Quick.max_iterations() < Depth::Standard.max_iterations());
    }
}
