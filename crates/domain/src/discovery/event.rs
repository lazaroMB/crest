//! The event vocabulary of a discovery.
//!
//! Events are appended, never edited. [`Logged`] carries the sequence
//! number and the time the storage gave an event; [`replay`](super::replay)
//! requires sequences to be contiguous from 1.

use std::fmt;

use serde::{Deserialize, Serialize};

use super::section::{Level, Section};

/// Who did something.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "name", rename_all = "snake_case")]
pub enum Actor {
    /// A person, by the name the chat knows (the git user, usually).
    Human(String),
    /// A model, by its id.
    Model(String),
    /// The engine itself, for a named process (`audit`, `user-text`).
    Process(String),
}

impl Actor {
    /// Whether this is a person.
    #[must_use]
    pub fn is_human(&self) -> bool {
        matches!(self, Self::Human(_))
    }
}

impl fmt::Display for Actor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Human(n) => write!(f, "human:{n}"),
            Self::Model(n) => write!(f, "model:{n}"),
            Self::Process(n) => write!(f, "process:{n}"),
        }
    }
}

/// The three answers a verdict may be.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Verdict {
    /// Approved, the next section opens.
    ApproveAsIs,
    /// Approved; a note is recorded, nothing is rewritten.
    ApproveWithNotes,
    /// The section is rewritten with the note as a correction.
    RejectAndRevise,
}

impl Verdict {
    /// Whether the verdict approves the section.
    #[must_use]
    pub const fn approves(self) -> bool {
        matches!(self, Self::ApproveAsIs | Self::ApproveWithNotes)
    }
}

/// Id of a stored source (`S7`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SourceId(pub u32);

impl fmt::Display for SourceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "S{}", self.0)
    }
}

/// Id of a spike (`SP1`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SpikeId(pub u32);

impl fmt::Display for SpikeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SP{}", self.0)
    }
}

/// A record the sections cite and the gates count.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RecordRef {
    /// The structural map of the system inside the scope.
    Structure,
    /// The feedback sentence and the directional variables.
    Variables,
    /// The computed causal loop diagram.
    Cld,
    /// The systemic risks, one per loop.
    Risks,
    /// A causal claim.
    Claim {
        /// Its id.
        id: u32,
    },
    /// A piece of evidence: a statement with a verbatim span of a source.
    Evidence {
        /// Its id.
        id: u32,
    },
}

impl fmt::Display for RecordRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Structure => f.write_str("structure"),
            Self::Variables => f.write_str("variables"),
            Self::Cld => f.write_str("cld"),
            Self::Risks => f.write_str("risks"),
            Self::Claim { id } => write!(f, "CL{id}"),
            Self::Evidence { id } => write!(f, "E{id}"),
        }
    }
}

/// What kind of source a text is. The audit may require a claim resting
/// on a vendor's own documentation to be marked as such.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    /// A peer-reviewed paper, a preprint or a study report.
    Study,
    /// A book or a chapter.
    Book,
    /// A standard, an RFC or a specification.
    Standard,
    /// An encyclopedia or a reference page.
    Encyclopedia,
    /// Documentation written by the vendor of the thing described.
    Vendor,
    /// Independent documentation or an article.
    Article,
    /// A file of a repository, this one or another.
    Repo,
    /// Not classified.
    #[default]
    Other,
}

impl SourceKind {
    /// Whether statements from this kind of source are the vendor's own
    /// claims rather than independent findings.
    #[must_use]
    pub const fn is_vendor(self) -> bool {
        matches!(self, Self::Vendor)
    }

    /// The name used in the document and the log.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Study => "study",
            Self::Book => "book",
            Self::Standard => "standard",
            Self::Encyclopedia => "encyclopedia",
            Self::Vendor => "vendor",
            Self::Article => "article",
            Self::Repo => "repo",
            Self::Other => "other",
        }
    }
}

impl fmt::Display for SourceKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How a record check ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordVerdict {
    /// The record stands on its sources.
    Confirmed,
    /// The sources contradict it.
    Refuted,
    /// The sources neither support nor contradict it.
    Inconclusive,
    /// The sources could not be read.
    Unverifiable,
}

/// How a spike was scored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpikeScore {
    /// The frozen criteria were met.
    Satisfactory,
    /// They were not; a new hypothesis is needed, not a weaker criterion.
    NotSatisfactory,
}

/// What can happen to a discovery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum DiscoveryEvent {
    /// The first event: the request and the level.
    Started {
        /// The user's request.
        problem: String,
        /// The level.
        level: Level,
    },
    /// The text of a source was fetched and stored.
    SourceFetched {
        /// Its id.
        source: SourceId,
        /// BLAKE3 or any stable hash of the stored text, hex.
        hash: String,
        /// Final URL or path after redirects.
        location: String,
        /// What kind of source it is.
        #[serde(default)]
        kind: SourceKind,
    },
    /// The open section was written (or rewritten).
    SectionWritten {
        /// The section.
        section: Section,
        /// Hash of the text, hex.
        hash: String,
    },
    /// The open section was audited.
    Audited {
        /// The section.
        section: Section,
        /// Findings that block a verdict.
        errors: u32,
        /// Findings that do not.
        warnings: u32,
        /// Whether an isolated reviewer ran, besides the deterministic
        /// checks.
        isolated: bool,
        /// Who audited (the engine, or the reviewer model).
        by: Actor,
    },
    /// A verdict on the open section.
    Verdict {
        /// The section.
        section: Section,
        /// The answer.
        choice: Verdict,
        /// Who answered.
        by: Actor,
        /// The note, if any.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    /// A record was checked against its sources.
    RecordChecked {
        /// The record.
        record: RecordRef,
        /// The result.
        verdict: RecordVerdict,
        /// Who checked.
        by: Actor,
    },
    /// A spike's worktree is about to be created. Appended *before* the
    /// `git worktree add`, so a resume can clean up first.
    SpikeStarted {
        /// Its id.
        spike: SpikeId,
        /// Worktree path, relative to the workspace.
        worktree: String,
        /// Throwaway branch.
        branch: String,
    },
    /// A spike was scored against its frozen criteria.
    SpikeScored {
        /// Its id.
        spike: SpikeId,
        /// The score.
        score: SpikeScore,
    },
    /// A section was reopened: it and everything after it lose their
    /// verdicts; records and spikes produced since no longer count.
    Reopened {
        /// The section.
        section: Section,
        /// Why.
        reason: String,
    },
    /// The discovery is accepted and frozen.
    Accepted {
        /// Who accepted (a person).
        by: Actor,
    },
    /// The discovery was abandoned.
    Aborted,
    /// Something was refused. Recorded so the trail shows every attempt
    /// made while the discovery was open; a frozen log takes nothing.
    Refused {
        /// What was attempted.
        what: String,
        /// Why it was illegal.
        why: String,
    },
}

/// An event with the sequence number and time its storage gave it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Logged {
    /// 1-based, contiguous.
    pub seq: u64,
    /// Unix milliseconds.
    pub at: u64,
    /// The event.
    #[serde(flatten)]
    pub event: DiscoveryEvent,
}

impl Logged {
    /// `event` at `seq`, with time `at`.
    #[must_use]
    pub fn new(seq: u64, at: u64, event: DiscoveryEvent) -> Self {
        Self { seq, at, event }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_round_trip_through_json_with_seq_flattened() {
        let logged = Logged::new(
            3,
            1_700_000_000_000,
            DiscoveryEvent::Verdict {
                section: Section::Scope,
                choice: Verdict::ApproveWithNotes,
                by: Actor::Human("alex".into()),
                note: Some("tighten Out".into()),
            },
        );
        let json = serde_json::to_value(&logged).unwrap();
        assert_eq!(json["seq"], 3);
        assert_eq!(json["event"], "verdict");
        assert_eq!(json["choice"], "approve-with-notes");
        assert_eq!(json["by"]["kind"], "human");
        let back: Logged = serde_json::from_value(json).unwrap();
        assert_eq!(back, logged);
    }

    #[test]
    fn actors_and_ids_display_their_prefixes() {
        assert_eq!(Actor::Human("a".into()).to_string(), "human:a");
        assert_eq!(Actor::Model("m".into()).to_string(), "model:m");
        assert_eq!(Actor::Process("audit".into()).to_string(), "process:audit");
        assert_eq!(SourceId(7).to_string(), "S7");
        assert_eq!(SpikeId(1).to_string(), "SP1");
        assert_eq!(RecordRef::Claim { id: 3 }.to_string(), "CL3");
    }
}
