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

use crate::block::SourceRef;
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

/// A piece of evidence and where it came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Evidence {
    /// Id cited by the Spec, e.g. `E3`.
    pub id: EvidenceId,
    /// What the evidence shows, in one sentence.
    pub claim: String,
    /// File lines, commit, URL or command output backing the claim.
    pub source: SourceRef,
    /// When it was collected, in Unix milliseconds.
    pub collected_at: u64,
}

/// Where a hypothesis stands after the experiments.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HypothesisStatus {
    /// Stated, experiments pending.
    #[default]
    Open,
    /// The experiments back it.
    Supported,
    /// The experiments contradict it.
    Refuted,
    /// Deliberately left without an experiment.
    Untested,
}

/// A one-sentence hypothesis with its causes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Hypothesis {
    /// Id, e.g. `H2`.
    pub id: HypothesisId,
    /// The hypothesis, in one sentence.
    pub statement: String,
    /// Candidate causes (the branches of the Ishikawa diagram).
    #[serde(default)]
    pub causes: Vec<String>,
    /// Priority in the hypothesis table; 1 is the highest.
    pub priority: u8,
    /// Result of the experiments so far.
    #[serde(default)]
    pub status: HypothesisStatus,
}

/// Category of a constraint, as in the constraint table of the Spec.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConstraintKind {
    /// Operating systems, architectures, toolchains.
    Platform,
    /// Latency, throughput, memory.
    Performance,
    /// APIs, formats or versions that must keep working.
    Compatibility,
    /// Secrets, permissions, sandboxing.
    Security,
    /// Anything else.
    Other,
}

/// How a test or check is run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TestKind {
    /// A test of the project's suite; `ref` is the command that runs it.
    Test,
    /// Any other command (benchmark, script); `ref` is the command line.
    Command,
}

/// A runnable check, serialized as `{"kind": "test", "ref": "..."}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TestRef {
    /// How to run it.
    pub kind: TestKind,
    /// What to run, e.g. `cargo test -p store offline`.
    #[serde(rename = "ref")]
    pub reference: String,
}

/// A constraint the solution must respect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Constraint {
    /// Id, e.g. `C3`.
    pub id: ConstraintId,
    /// Category.
    pub kind: ConstraintKind,
    /// The constraint, measurable when possible.
    pub statement: String,
    /// Check that proves it holds, once known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test: Option<TestRef>,
}

/// A choice between options, backed by evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Decision {
    /// Id, e.g. `D1`.
    pub id: DecisionId,
    /// The option chosen.
    pub chosen: String,
    /// Options considered and discarded.
    #[serde(default)]
    pub discarded: Vec<String>,
    /// Why the chosen option wins.
    pub rationale: String,
    /// Evidence the rationale relies on.
    #[serde(default)]
    pub evidence_ids: Vec<EvidenceId>,
}

/// An entry of the research journal, written by every node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct JournalEntry {
    /// When it was written, in Unix milliseconds.
    pub at: u64,
    /// Node that wrote it.
    pub stage: Stage,
    /// Markdown text.
    pub text: String,
}

/// Everything the research graph has learned so far.
///
/// New records get the next free id of their kind, so ids stay
/// sequential even after the state is restored from a checkpoint.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub struct ResearchState {
    /// Collected evidence, in collection order.
    #[serde(default)]
    pub evidence: Vec<Evidence>,
    /// Hypotheses, in creation order.
    #[serde(default)]
    pub hypotheses: Vec<Hypothesis>,
    /// Constraints of the problem.
    #[serde(default)]
    pub constraints: Vec<Constraint>,
    /// Decisions taken while modeling.
    #[serde(default)]
    pub decisions: Vec<Decision>,
    /// Research journal, one or more entries per node.
    #[serde(default)]
    pub journal: Vec<JournalEntry>,
    /// Measurable one-sentence problem statements; one Spec each.
    #[serde(default)]
    pub problem_statements: Vec<String>,
}

impl ResearchState {
    /// An allocator that continues after every id in the state.
    #[must_use]
    pub fn allocator(&self) -> IdAllocator {
        IdAllocator::after(
            self.evidence.iter().map(|e| &e.id),
            self.hypotheses.iter().map(|h| &h.id),
            self.constraints.iter().map(|c| &c.id),
            self.decisions.iter().map(|d| &d.id),
        )
    }

    /// Records a piece of evidence and returns its new id.
    pub fn add_evidence(
        &mut self,
        claim: impl Into<String>,
        source: SourceRef,
        collected_at: u64,
    ) -> EvidenceId {
        let id = self.allocator().next_evidence();
        self.evidence.push(Evidence {
            id,
            claim: claim.into(),
            source,
            collected_at,
        });
        id
    }

    /// Records an open hypothesis and returns its new id.
    pub fn add_hypothesis(
        &mut self,
        statement: impl Into<String>,
        causes: Vec<String>,
        priority: u8,
    ) -> HypothesisId {
        let id = self.allocator().next_hypothesis();
        self.hypotheses.push(Hypothesis {
            id,
            statement: statement.into(),
            causes,
            priority,
            status: HypothesisStatus::Open,
        });
        id
    }

    /// Records a constraint and returns its new id.
    pub fn add_constraint(
        &mut self,
        kind: ConstraintKind,
        statement: impl Into<String>,
        test: Option<TestRef>,
    ) -> ConstraintId {
        let id = self.allocator().next_constraint();
        self.constraints.push(Constraint {
            id,
            kind,
            statement: statement.into(),
            test,
        });
        id
    }

    /// Records a decision and returns its new id.
    pub fn add_decision(
        &mut self,
        chosen: impl Into<String>,
        discarded: Vec<String>,
        rationale: impl Into<String>,
        evidence_ids: Vec<EvidenceId>,
    ) -> DecisionId {
        let id = self.allocator().next_decision();
        self.decisions.push(Decision {
            id,
            chosen: chosen.into(),
            discarded,
            rationale: rationale.into(),
            evidence_ids,
        });
        id
    }

    /// Appends an entry to the research journal.
    pub fn log(&mut self, at: u64, stage: Stage, text: impl Into<String>) {
        self.journal.push(JournalEntry {
            at,
            stage,
            text: text.into(),
        });
    }

    /// The evidence with the given id.
    #[must_use]
    pub fn evidence(&self, id: EvidenceId) -> Option<&Evidence> {
        self.evidence.iter().find(|e| e.id == id)
    }

    /// Checks that every cited id names collected evidence.
    ///
    /// The critical review uses it to reject claims without a source.
    ///
    /// # Errors
    ///
    /// Returns the unknown ids, without repeats, in the order they were
    /// first cited.
    pub fn cites_only_known_evidence(&self, ids: &[EvidenceId]) -> Result<(), Vec<EvidenceId>> {
        let mut unknown: Vec<EvidenceId> = Vec::new();
        for &id in ids {
            if self.evidence(id).is_none() && !unknown.contains(&id) {
                unknown.push(id);
            }
        }
        if unknown.is_empty() {
            Ok(())
        } else {
            Err(unknown)
        }
    }
}

/// Evidence ids cited in a markdown text, as `[E3]` or `[E1, E4]`.
///
/// Brackets count only when everything inside them is a comma-separated
/// list of evidence ids, so links like `[docs](...)` are ignored. Fenced
/// code blocks and inline code spans are skipped. Ids come back without
/// repeats, in the order they are first cited.
#[must_use]
pub fn extract_citations(markdown: &str) -> Vec<EvidenceId> {
    let mut ids = Vec::new();
    let mut fence: Option<(char, usize)> = None;
    for line in markdown.lines() {
        if let Some(marker) = fence_marker(line) {
            match fence {
                None => fence = Some(marker),
                Some((ch, len)) if marker.0 == ch && marker.1 >= len => fence = None,
                Some(_) => {}
            }
            continue;
        }
        if fence.is_none() {
            collect_bracketed_ids(&strip_code_spans(line), &mut ids);
        }
    }
    ids
}

/// The fence character and length when `line` opens or closes a code
/// fence (up to three spaces of indentation, then 3+ backticks or tildes).
fn fence_marker(line: &str) -> Option<(char, usize)> {
    let trimmed = line.trim_start_matches(' ');
    if line.len() - trimmed.len() > 3 {
        return None;
    }
    let ch = trimmed.chars().next().filter(|c| *c == '`' || *c == '~')?;
    let len = trimmed.chars().take_while(|c| *c == ch).count();
    (len >= 3).then_some((ch, len))
}

/// The line with every closed inline code span removed.
fn strip_code_spans(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(start) = rest.find('`') {
        out.push_str(&rest[..start]);
        let after = &rest[start..];
        let ticks = after.len() - after.trim_start_matches('`').len();
        let body = &after[ticks..];
        let closing = find_tick_run(body, ticks);
        match closing {
            Some(end) => rest = &body[end + ticks..],
            None => {
                out.push_str(&after[..ticks]);
                rest = body;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Byte offset of the first run of exactly `n` backticks in `s`.
fn find_tick_run(s: &str, n: usize) -> Option<usize> {
    let mut offset = 0;
    while let Some(start) = s[offset..].find('`') {
        let at = offset + start;
        let run = s[at..].len() - s[at..].trim_start_matches('`').len();
        if run == n {
            return Some(at);
        }
        offset = at + run;
    }
    None
}

/// Appends the ids of every `[E1, E2]` group of `text` not seen yet.
fn collect_bracketed_ids(text: &str, ids: &mut Vec<EvidenceId>) {
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        let inner = &rest[open + 1..];
        let Some(close) = inner.find(['[', ']']) else {
            return;
        };
        if inner.as_bytes()[close] == b']' {
            let group: Result<Vec<EvidenceId>, _> = inner[..close]
                .split(',')
                .map(|p| p.trim().parse())
                .collect();
            for id in group.into_iter().flatten() {
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
        }
        rest = &inner[close..];
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

    #[test]
    fn records_have_a_stable_wire_shape() {
        let evidence = Evidence {
            id: EvidenceId(3),
            claim: "p95 sube a 500 ms con 500 rps".into(),
            source: SourceRef::Command {
                command: "make bench".into(),
                output_excerpt: "p95=500ms".into(),
            },
            collected_at: 1_700_000_000_000,
        };
        assert_eq!(
            serde_json::to_value(&evidence).unwrap(),
            json!({
                "id": "E3",
                "claim": "p95 sube a 500 ms con 500 rps",
                "source": {"type": "command", "command": "make bench", "output_excerpt": "p95=500ms"},
                "collected_at": 1_700_000_000_000_u64
            })
        );
        let constraint = Constraint {
            id: ConstraintId(2),
            kind: ConstraintKind::Compatibility,
            statement: "funciona sin red".into(),
            test: Some(TestRef {
                kind: TestKind::Test,
                reference: "cargo test -p store offline".into(),
            }),
        };
        assert_eq!(
            serde_json::to_value(&constraint).unwrap(),
            json!({
                "id": "C2",
                "kind": "compatibility",
                "statement": "funciona sin red",
                "test": {"kind": "test", "ref": "cargo test -p store offline"}
            })
        );
    }

    #[test]
    fn optional_record_fields_default() {
        let hypothesis: Hypothesis =
            serde_json::from_value(json!({"id": "H1", "statement": "s", "priority": 1})).unwrap();
        assert_eq!(hypothesis.status, HypothesisStatus::Open);
        assert!(hypothesis.causes.is_empty());
        let decision: Decision =
            serde_json::from_value(json!({"id": "D1", "chosen": "a", "rationale": "r"})).unwrap();
        assert!(decision.discarded.is_empty() && decision.evidence_ids.is_empty());
        let entry = JournalEntry {
            at: 5,
            stage: Stage::Data,
            text: "t".into(),
        };
        let back: JournalEntry =
            serde_json::from_value(json!({"at": 5, "stage": "data", "text": "t"})).unwrap();
        assert_eq!(back, entry);
    }

    fn url(u: &str) -> SourceRef {
        SourceRef::Url {
            url: u.into(),
            title: None,
        }
    }

    #[test]
    fn state_allocates_sequential_ids_per_kind() {
        let mut state = ResearchState::default();
        assert_eq!(state.add_evidence("a", url("https://a"), 1), EvidenceId(1));
        assert_eq!(state.add_evidence("b", url("https://b"), 2), EvidenceId(2));
        assert_eq!(state.add_hypothesis("h", vec![], 1), HypothesisId(1));
        assert_eq!(
            state.add_constraint(ConstraintKind::Security, "c", None),
            ConstraintId(1)
        );
        let decision = state.add_decision("x", vec!["y".into()], "r", vec![EvidenceId(2)]);
        assert_eq!(decision, DecisionId(1));
        state.log(3, Stage::Modeling, "eligió x");
        assert_eq!(state.journal.len(), 1);
        assert_eq!(state.evidence(EvidenceId(2)).unwrap().claim, "b");
    }

    #[test]
    fn restored_state_continues_after_its_highest_id() {
        let mut state: ResearchState = serde_json::from_value(json!({
            "evidence": [
                {"id": "E4", "claim": "a", "source": {"type": "url", "url": "https://a"}, "collected_at": 1}
            ]
        }))
        .unwrap();
        assert_eq!(
            state.add_evidence("b", url("https://b"), 2).to_string(),
            "E5"
        );
        assert_eq!(state.add_hypothesis("h", vec![], 1).to_string(), "H1");
    }

    #[test]
    fn citations_of_unknown_evidence_are_reported_once() {
        let mut state = ResearchState::default();
        let e1 = state.add_evidence("a", url("https://a"), 1);
        assert_eq!(state.cites_only_known_evidence(&[]), Ok(()));
        assert_eq!(state.cites_only_known_evidence(&[e1, e1]), Ok(()));
        let (e7, e3) = (EvidenceId(7), EvidenceId(3));
        assert_eq!(
            state.cites_only_known_evidence(&[e7, e1, e3, e7]),
            Err(vec![e7, e3])
        );
    }

    #[test]
    fn research_state_schema() {
        insta::assert_json_snapshot!(
            "research_state_schema",
            schemars::schema_for!(ResearchState)
        );
    }

    fn ids(text: &str) -> Vec<String> {
        extract_citations(text)
            .iter()
            .map(ToString::to_string)
            .collect()
    }

    #[test]
    fn citations_are_extracted_in_first_cited_order() {
        let text = "La latencia sube [E3] con la carga [E1, E4].\n\nOtra vez [E3] y [ E2 ].";
        assert_eq!(ids(text), ["E3", "E1", "E4", "E2"]);
    }

    #[test]
    fn non_citation_brackets_are_ignored() {
        let text = "[docs](https://x) [E1 y E2] [E0] [H1] [E1, docs] [] [[E5]] - [x] tarea [E6";
        assert_eq!(ids(text), ["E5"]);
    }

    #[test]
    fn citations_inside_code_are_ignored() {
        let text = "Antes [E1] y `arr[E2]` y ``x `[E3]` y``.\n\
                    ```rust\nlet a = v[E4];\n```\n\
                    ~~~~\n```\n[E5]\n~~~~\n\
                    Después [E6] y `sin cerrar [E7]";
        assert_eq!(ids(text), ["E1", "E6", "E7"]);
    }

    #[test]
    fn citations_feed_the_known_evidence_check() {
        let mut state = ResearchState::default();
        state.add_evidence("a", url("https://a"), 1);
        let cited = extract_citations("Sí [E1]; no [E2].");
        assert_eq!(
            state.cites_only_known_evidence(&cited),
            Err(vec![EvidenceId(2)])
        );
    }
}
