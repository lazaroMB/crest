//! Typed state of the research workflow (X-Workflow for software).
//!
//! Every record has a short, readable id (`E1`, `H2`, `C3`, `D1`) that the
//! Spec cites in its text. Ids are allocated sequentially per kind by an
//! [`IdAllocator`] and serialize as plain strings.

use std::borrow::Cow;
use std::fmt;
use std::str::FromStr;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::model::ModelRole;

/// Error returned when a string is not a valid research id.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid {kind} id `{value}`: expected `{prefix}` followed by a number from 1")]
pub struct InvalidId {
    /// Kind of id that was expected (e.g. `evidence`).
    pub kind: &'static str,
    /// Expected prefix letter.
    pub prefix: char,
    /// Rejected text.
    pub value: String,
}

/// Parses `<prefix><n>` with `n >= 1` and no leading zeros.
fn parse_id(s: &str, prefix: char, kind: &'static str) -> Result<u32, InvalidId> {
    let invalid = || InvalidId {
        kind,
        prefix,
        value: s.to_owned(),
    };
    let digits = s.strip_prefix(prefix).ok_or_else(invalid)?;
    if digits.is_empty() || digits.starts_with('0') || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(invalid());
    }
    digits.parse().map_err(|_| invalid())
}

macro_rules! research_id {
    ($(#[$doc:meta])* $name:ident, $prefix:literal, $kind:literal) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(u32);

        impl $name {
            /// Letter that starts the id.
            pub const PREFIX: char = $prefix;

            /// Creates the id with sequence number `n` (from 1).
            ///
            /// # Errors
            ///
            /// Returns [`InvalidId`] when `n` is 0.
            pub fn new(n: u32) -> Result<Self, InvalidId> {
                if n == 0 {
                    return Err(InvalidId {
                        kind: $kind,
                        prefix: $prefix,
                        value: format!("{}0", $prefix),
                    });
                }
                Ok(Self(n))
            }

            /// Sequence number of the id (from 1).
            #[must_use]
            pub const fn number(self) -> u32 {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}{}", $prefix, self.0)
            }
        }

        impl FromStr for $name {
            type Err = InvalidId;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                parse_id(s, $prefix, $kind).map(Self)
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.collect_str(self)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let s = Cow::<'de, str>::deserialize(deserializer)?;
                s.parse().map_err(serde::de::Error::custom)
            }
        }

        impl JsonSchema for $name {
            fn schema_name() -> Cow<'static, str> {
                stringify!($name).into()
            }

            fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
                json_schema!({
                    "description": concat!("Readable ", $kind, " id, e.g. `", $prefix, "1`."),
                    "type": "string",
                    "pattern": concat!("^", $prefix, "[1-9][0-9]*$")
                })
            }
        }
    };
}

research_id!(
    /// Id of an evidence record, e.g. `E3`.
    EvidenceId, 'E', "evidence"
);
research_id!(
    /// Id of a hypothesis, e.g. `H2`.
    HypothesisId, 'H', "hypothesis"
);
research_id!(
    /// Id of a constraint, e.g. `C3`.
    ConstraintId, 'C', "constraint"
);
research_id!(
    /// Id of a decision, e.g. `D1`.
    DecisionId, 'D', "decision"
);

/// Sequential id generator, one counter per kind.
///
/// It stores the last number handed out, so a fresh allocator yields `E1`,
/// `E2`, ... and one built with [`IdAllocator::after`] continues after the
/// highest id already in use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub struct IdAllocator {
    evidence: u32,
    hypothesis: u32,
    constraint: u32,
    decision: u32,
}

impl IdAllocator {
    /// An allocator that starts every kind at 1.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            evidence: 0,
            hypothesis: 0,
            constraint: 0,
            decision: 0,
        }
    }

    /// An allocator that continues after the given ids.
    #[must_use]
    pub fn after<'a>(
        evidence: impl IntoIterator<Item = &'a EvidenceId>,
        hypotheses: impl IntoIterator<Item = &'a HypothesisId>,
        constraints: impl IntoIterator<Item = &'a ConstraintId>,
        decisions: impl IntoIterator<Item = &'a DecisionId>,
    ) -> Self {
        Self {
            evidence: evidence.into_iter().map(|id| id.0).max().unwrap_or(0),
            hypothesis: hypotheses.into_iter().map(|id| id.0).max().unwrap_or(0),
            constraint: constraints.into_iter().map(|id| id.0).max().unwrap_or(0),
            decision: decisions.into_iter().map(|id| id.0).max().unwrap_or(0),
        }
    }

    /// Next evidence id.
    pub fn next_evidence(&mut self) -> EvidenceId {
        EvidenceId(bump(&mut self.evidence))
    }

    /// Next hypothesis id.
    pub fn next_hypothesis(&mut self) -> HypothesisId {
        HypothesisId(bump(&mut self.hypothesis))
    }

    /// Next constraint id.
    pub fn next_constraint(&mut self) -> ConstraintId {
        ConstraintId(bump(&mut self.constraint))
    }

    /// Next decision id.
    pub fn next_decision(&mut self) -> DecisionId {
        DecisionId(bump(&mut self.decision))
    }
}

/// Increments a counter and returns the new value, saturating at `u32::MAX`.
fn bump(counter: &mut u32) -> u32 {
    *counter = counter.saturating_add(1);
    *counter
}

/// One of the four phases of the X-Workflow.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// Understand the current system and gather evidence (nodes 1-6).
    Discovery,
    /// State the problem and its constraints (node 7).
    Definition,
    /// Look for and choose a solution (nodes 8-11).
    SolutionSearch,
    /// Write the Spec and the plan, and review them (nodes 12-14).
    Specification,
}

impl Phase {
    /// All phases, in workflow order.
    pub const ALL: [Self; 4] = [
        Self::Discovery,
        Self::Definition,
        Self::SolutionSearch,
        Self::Specification,
    ];

    /// Name shown to the user.
    #[must_use]
    pub const fn name_es(self) -> &'static str {
        match self {
            Self::Discovery => "Descubrimiento",
            Self::Definition => "Definición",
            Self::SolutionSearch => "Búsqueda de soluciones",
            Self::Specification => "Especificación",
        }
    }
}

/// A node of the research graph, in workflow order.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    /// 1. Restate and classify the request; ask for what is missing.
    Intake,
    /// 2. Walk the repo and draw the current architecture.
    Observation,
    /// 3. Gather evidence: history, logs, metrics, issues, docs.
    Data,
    /// 4. Contrast ideas with the data.
    Analysis,
    /// 5. One-sentence hypotheses, cause diagram and priorities.
    Hypotheses,
    /// 6. Reproducible protocols: failing tests, benchmarks.
    Experiments,
    /// 7. Measurable problem statements and the constraint table.
    Definition,
    /// 8. Prior solutions: libraries, articles, patterns in the repo.
    Research,
    /// 9. Unfiltered options.
    Brainstorm,
    /// 10. Discard what violates constraints; decision matrix.
    Modeling,
    /// 11. Optional spike in a temporary worktree.
    Prototype,
    /// 12. Write the Spec.
    Spec,
    /// 13. Split the Spec into tasks.
    Plan,
    /// 14. A clean-context subagent looks for gaps.
    CriticalReview,
}

impl Stage {
    /// All stages, in workflow order.
    pub const ALL: [Self; 14] = [
        Self::Intake,
        Self::Observation,
        Self::Data,
        Self::Analysis,
        Self::Hypotheses,
        Self::Experiments,
        Self::Definition,
        Self::Research,
        Self::Brainstorm,
        Self::Modeling,
        Self::Prototype,
        Self::Spec,
        Self::Plan,
        Self::CriticalReview,
    ];

    /// Number of stages (14).
    #[must_use]
    pub const fn total() -> u32 {
        // The array length is a small constant, so the cast is lossless.
        Self::ALL.len() as u32
    }

    /// 1-based position in the workflow, `1..=14`.
    #[must_use]
    pub const fn index(self) -> u32 {
        self as u32 + 1
    }

    /// The stage at a 1-based position, if any.
    #[must_use]
    pub fn from_index(index: u32) -> Option<Self> {
        let i = usize::try_from(index.checked_sub(1)?).ok()?;
        Self::ALL.get(i).copied()
    }

    /// The following stage, `None` after the critical review.
    #[must_use]
    pub fn next(self) -> Option<Self> {
        Self::from_index(self.index() + 1)
    }

    /// Phase the stage belongs to.
    #[must_use]
    pub const fn phase(self) -> Phase {
        match self {
            Self::Intake
            | Self::Observation
            | Self::Data
            | Self::Analysis
            | Self::Hypotheses
            | Self::Experiments => Phase::Discovery,
            Self::Definition => Phase::Definition,
            Self::Research | Self::Brainstorm | Self::Modeling | Self::Prototype => {
                Phase::SolutionSearch
            }
            Self::Spec | Self::Plan | Self::CriticalReview => Phase::Specification,
        }
    }

    /// Name shown to the user (e.g. in the `StageHeader` block).
    #[must_use]
    pub const fn name_es(self) -> &'static str {
        match self {
            Self::Intake => "Intake",
            Self::Observation => "Observación",
            Self::Data => "Datos",
            Self::Analysis => "Análisis",
            Self::Hypotheses => "Hipótesis",
            Self::Experiments => "Experimentos",
            Self::Definition => "Definición y restricciones",
            Self::Research => "Investigación",
            Self::Brainstorm => "Lluvia de ideas",
            Self::Modeling => "Modelado",
            Self::Prototype => "Prototipo",
            Self::Spec => "Spec",
            Self::Plan => "Plan",
            Self::CriticalReview => "Revisión crítica",
        }
    }

    /// Model role the stage runs with by default.
    #[must_use]
    pub const fn default_role(self) -> ModelRole {
        match self {
            Self::Intake | Self::Analysis | Self::Experiments | Self::Prototype => ModelRole::Main,
            Self::Observation | Self::Data | Self::Research => ModelRole::Explore,
            Self::Hypotheses
            | Self::Definition
            | Self::Brainstorm
            | Self::Modeling
            | Self::Spec
            | Self::Plan
            | Self::CriticalReview => ModelRole::Plan,
        }
    }

    /// Whether the stage ends in a gate that waits for the user.
    #[must_use]
    pub const fn has_gate(self) -> bool {
        matches!(
            self,
            Self::Intake
                | Self::Hypotheses
                | Self::Definition
                | Self::Modeling
                | Self::CriticalReview
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ids_display_and_parse_with_their_prefix() {
        let id: EvidenceId = "E12".parse().unwrap();
        assert_eq!(id.number(), 12);
        assert_eq!(id.to_string(), "E12");
        assert_eq!("H2".parse::<HypothesisId>().unwrap().to_string(), "H2");
        assert_eq!("C3".parse::<ConstraintId>().unwrap().number(), 3);
        assert_eq!("D1".parse::<DecisionId>().unwrap().number(), 1);
    }

    #[test]
    fn malformed_ids_are_rejected() {
        for bad in [
            "",
            "E",
            "E0",
            "E01",
            "e1",
            "H1",
            "E1a",
            "E-1",
            " E1",
            "E99999999999",
        ] {
            assert!(bad.parse::<EvidenceId>().is_err(), "{bad:?} accepted");
        }
        assert!(EvidenceId::new(0).is_err());
        let err = "X1".parse::<ConstraintId>().unwrap_err();
        assert_eq!(
            err.to_string(),
            "invalid constraint id `X1`: expected `C` followed by a number from 1"
        );
    }

    #[test]
    fn ids_serialize_as_strings() {
        let id = DecisionId::new(4).unwrap();
        assert_eq!(serde_json::to_value(id).unwrap(), json!("D4"));
        let back: DecisionId = serde_json::from_value(json!("D4")).unwrap();
        assert_eq!(back, id);
        assert!(serde_json::from_value::<DecisionId>(json!("E4")).is_err());
        assert!(serde_json::from_value::<DecisionId>(json!(4)).is_err());
    }

    #[test]
    fn allocator_is_sequential_per_kind() {
        let mut ids = IdAllocator::new();
        assert_eq!(ids.next_evidence().to_string(), "E1");
        assert_eq!(ids.next_evidence().to_string(), "E2");
        assert_eq!(ids.next_hypothesis().to_string(), "H1");
        assert_eq!(ids.next_constraint().to_string(), "C1");
        assert_eq!(ids.next_decision().to_string(), "D1");
        assert_eq!(ids.next_evidence().to_string(), "E3");
    }

    #[test]
    fn allocator_continues_after_existing_ids() {
        let evidence = [EvidenceId(2), EvidenceId(7), EvidenceId(3)];
        let none: [HypothesisId; 0] = [];
        let mut ids = IdAllocator::after(&evidence, &none, &[ConstraintId(1)], &[]);
        assert_eq!(ids.next_evidence(), EvidenceId(8));
        assert_eq!(ids.next_hypothesis(), HypothesisId(1));
        assert_eq!(ids.next_constraint(), ConstraintId(2));
        assert_eq!(ids.next_decision(), DecisionId(1));
    }

    #[test]
    fn stages_are_numbered_one_to_fourteen() {
        assert_eq!(Stage::total(), 14);
        for (i, stage) in Stage::ALL.into_iter().enumerate() {
            let index = u32::try_from(i).unwrap() + 1;
            assert_eq!(stage.index(), index);
            assert_eq!(Stage::from_index(index), Some(stage));
        }
        assert_eq!(Stage::Intake.index(), 1);
        assert_eq!(Stage::CriticalReview.index(), 14);
        assert_eq!(Stage::from_index(0), None);
        assert_eq!(Stage::from_index(15), None);
        assert_eq!(Stage::Spec.next(), Some(Stage::Plan));
        assert_eq!(Stage::CriticalReview.next(), None);
    }

    #[test]
    fn stages_map_to_their_phase_role_and_gate() {
        let count = |phase| Stage::ALL.iter().filter(|s| s.phase() == phase).count();
        assert_eq!(count(Phase::Discovery), 6);
        assert_eq!(count(Phase::Definition), 1);
        assert_eq!(count(Phase::SolutionSearch), 4);
        assert_eq!(count(Phase::Specification), 3);
        assert_eq!(Stage::Experiments.phase(), Phase::Discovery);
        assert_eq!(Stage::Prototype.phase(), Phase::SolutionSearch);
        assert_eq!(Stage::Data.default_role(), ModelRole::Explore);
        assert_eq!(Stage::CriticalReview.default_role(), ModelRole::Plan);
        assert_eq!(Stage::ALL.iter().filter(|s| s.has_gate()).count(), 5);
        assert_eq!(Stage::Definition.name_es(), "Definición y restricciones");
        assert_eq!(Phase::SolutionSearch.name_es(), "Búsqueda de soluciones");
    }

    #[test]
    fn stages_serialize_in_snake_case() {
        assert_eq!(
            serde_json::to_value(Stage::CriticalReview).unwrap(),
            json!("critical_review")
        );
        assert_eq!(
            serde_json::to_value(Phase::SolutionSearch).unwrap(),
            json!("solution_search")
        );
    }
}
