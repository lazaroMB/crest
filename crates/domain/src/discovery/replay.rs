//! The state of a discovery, replayed from its events, and the legality
//! of the next one.
//!
//! The state is a projection: nothing in it is authoritative except as a
//! fold over the log. Gates compare **sequence numbers**: a record counts
//! for a gate only if it was confirmed after the approving verdict the
//! gate names, so reopening a section closes the gates behind it without
//! deleting anything.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::event::{
    Actor, DiscoveryEvent, Logged, RecordRef, RecordVerdict, SourceId, SourceKind, SpikeId,
    SpikeScore, Verdict,
};
use super::section::{Level, Phase, Section, VerdictPolicy};

/// Whether the discovery is still being written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Sections are being written and judged.
    Open,
    /// Frozen: every section approved and accepted by a person.
    Accepted,
    /// Abandoned.
    Aborted,
}

/// An approving verdict and when it was given.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Approval {
    /// Sequence of the verdict event.
    pub seq: u64,
    /// Who approved.
    pub by: Actor,
    /// The choice (one of the approving ones).
    pub choice: Verdict,
    /// The note, if any.
    pub note: Option<String>,
}

/// The latest audit of the open section.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Audit {
    /// Sequence of the audit event.
    pub seq: u64,
    /// Blocking findings.
    pub errors: u32,
    /// Non-blocking findings.
    pub warnings: u32,
    /// Whether an isolated reviewer ran.
    pub isolated: bool,
}

/// The latest check of a record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordState {
    /// Sequence of the check.
    pub seq: u64,
    /// Its result.
    pub verdict: RecordVerdict,
    /// Who checked.
    pub by: Actor,
}

/// A stored source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceState {
    /// Sequence of the fetch.
    pub seq: u64,
    /// Hash of the stored text.
    pub hash: String,
    /// Where it was read from.
    pub location: String,
    /// What kind of source it is.
    #[serde(default)]
    pub kind: SourceKind,
}

/// A spike and how far it got.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpikeState {
    /// Worktree and branch, once started.
    pub worktree: String,
    /// Throwaway branch.
    pub branch: String,
    /// Sequence of the start.
    pub started_seq: u64,
    /// Score and its sequence, once scored.
    pub score: Option<(SpikeScore, u64)>,
}

/// The one legal next step, as `/research` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NextStep {
    /// Write (or rewrite) the open section.
    Write(Section),
    /// The open section's gate is closed; what is missing.
    OpenGate {
        /// The section.
        section: Section,
        /// What the gate still needs.
        missing: Vec<String>,
    },
    /// The latest audit found errors; fix and audit again.
    FixAudit {
        /// The section.
        section: Section,
        /// How many errors.
        errors: u32,
    },
    /// The section is written and audited; it needs its verdict.
    Verdict(Section),
    /// Every section is approved; a person may accept.
    Accept,
    /// Nothing more can happen.
    Frozen,
}

/// Why an event may not be appended now.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Illegal {
    /// The first event must be `Started`, and only the first.
    #[error("the discovery has not started")]
    NotStarted,
    /// `Started` after the first event.
    #[error("the discovery already started")]
    AlreadyStarted,
    /// The discovery is accepted or aborted.
    #[error("the discovery is {0:?}: nothing more can happen")]
    Frozen(Status),
    /// The event names a section that is not the open one.
    #[error("`{given}` is not the open section (`{open}`)")]
    NotOpen {
        /// The section the event names.
        given: Section,
        /// The open section.
        open: String,
    },
    /// The section is not part of this level.
    #[error("`{0}` is not a section of this level")]
    NotInLevel(Section),
    /// The section's gate is closed.
    #[error("the gate of `{section}` is closed: {}", missing.join("; "))]
    GateClosed {
        /// The section.
        section: Section,
        /// What is missing.
        missing: Vec<String>,
    },
    /// An audit before a write, or a verdict before an audit.
    #[error("`{section}` has not been {what} since it opened")]
    Missing {
        /// The section.
        section: Section,
        /// `written` or `audited`.
        what: &'static str,
    },
    /// The actor may not give this verdict here.
    #[error("{by} may not approve `{section}`: {why}")]
    Forbidden {
        /// The section.
        section: Section,
        /// Who tried.
        by: Actor,
        /// Why not.
        why: String,
    },
    /// Reopening a section that holds no verdict.
    #[error("`{0}` has no verdict to reopen")]
    NothingToReopen(Section),
    /// Scoring a spike that never started.
    #[error("spike {0} never started")]
    UnknownSpike(SpikeId),
    /// Accepting before every section is approved.
    #[error("`{0}` is still open")]
    StillOpen(Section),
}

/// Why a log could not be replayed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReplayError {
    /// A sequence number is not the one expected.
    #[error("event sequence {found} where {expected} was expected")]
    Sequence {
        /// What the log should have had.
        expected: u64,
        /// What it had.
        found: u64,
    },
    /// An event was illegal from the state before it.
    #[error("event {seq} is illegal: {reason}")]
    Illegal {
        /// Its sequence.
        seq: u64,
        /// Why.
        reason: Illegal,
    },
}

/// The projected state of a discovery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveryState {
    /// The request.
    pub problem: String,
    /// The level.
    pub level: Level,
    /// Open, accepted or aborted.
    pub status: Status,
    /// Approving verdicts by section.
    pub approved: BTreeMap<Section, Approval>,
    /// Hash of the open section's text since it opened, if written.
    pub written: Option<String>,
    /// The latest audit of the open section since it was last written.
    pub audit: Option<Audit>,
    /// The latest check of every record.
    pub records: BTreeMap<RecordRef, RecordState>,
    /// Every stored source.
    pub sources: BTreeMap<SourceId, SourceState>,
    /// Every spike.
    pub spikes: BTreeMap<SpikeId, SpikeState>,
    /// How many times a section was reopened.
    pub reopens: u32,
    /// How many events were refused.
    pub refusals: u32,
    /// Sequence of the last event.
    pub last_seq: u64,
}

impl DiscoveryState {
    fn new(problem: String, level: Level) -> Self {
        Self {
            problem,
            level,
            status: Status::Open,
            approved: BTreeMap::new(),
            written: None,
            audit: None,
            records: BTreeMap::new(),
            sources: BTreeMap::new(),
            spikes: BTreeMap::new(),
            reopens: 0,
            refusals: 0,
            last_seq: 0,
        }
    }

    /// The open section: the first of the level without an approval.
    /// `None` when every section is approved.
    #[must_use]
    pub fn open(&self) -> Option<Section> {
        self.level
            .sections()
            .iter()
            .copied()
            .find(|s| !self.approved.contains_key(s))
    }

    /// The phase of the open section, or `Solution` when all is approved.
    #[must_use]
    pub fn phase(&self) -> Phase {
        self.open().map_or(Phase::Solution, Section::phase)
    }

    /// Sequence of the approving verdict of `section`, if any.
    #[must_use]
    pub fn approved_at(&self, section: Section) -> Option<u64> {
        self.approved.get(&section).map(|a| a.seq)
    }

    /// Whether `record` was confirmed after `seq`.
    #[must_use]
    pub fn confirmed_after(&self, record: RecordRef, seq: u64) -> bool {
        self.records
            .get(&record)
            .is_some_and(|r| r.verdict == RecordVerdict::Confirmed && r.seq > seq)
    }

    /// Claims confirmed after `seq`.
    #[must_use]
    pub fn claims_confirmed_after(&self, seq: u64) -> usize {
        self.records
            .iter()
            .filter(|(r, s)| {
                matches!(r, RecordRef::Claim { .. })
                    && s.verdict == RecordVerdict::Confirmed
                    && s.seq > seq
            })
            .count()
    }

    /// Spikes scored satisfactory after `seq`.
    #[must_use]
    pub fn spikes_satisfactory_after(&self, seq: u64) -> usize {
        self.spikes
            .values()
            .filter(|s| matches!(s.score, Some((SpikeScore::Satisfactory, at)) if at > seq))
            .count()
    }

    /// Spikes that started and were never scored: a resume must clean
    /// their worktrees up before anything else.
    pub fn unfinished_spikes(&self) -> impl Iterator<Item = (SpikeId, &SpikeState)> {
        self.spikes
            .iter()
            .filter(|(_, s)| s.score.is_none())
            .map(|(id, s)| (*id, s))
    }

    /// What still blocks `section` from being written. Empty when the gate
    /// is open. Only the deep level has gates.
    #[must_use]
    pub fn gate_missing(&self, section: Section) -> Vec<String> {
        if self.level != Level::Deep {
            return Vec::new();
        }
        let mut missing = Vec::new();
        match section {
            Section::Findings | Section::CurrentSystem => {
                let Some(scope) = self.approved_at(Section::Scope) else {
                    return vec!["`Scope` has no approving verdict".to_owned()];
                };
                for record in [RecordRef::Structure, RecordRef::Variables, RecordRef::Cld] {
                    if !self.confirmed_after(record, scope) {
                        missing.push(format!(
                            "`{record}` confirmed after the `Scope` verdict (seq {scope})"
                        ));
                    }
                }
                if self.claims_confirmed_after(scope) == 0 {
                    missing.push(format!(
                        "at least one claim confirmed after the `Scope` verdict (seq {scope})"
                    ));
                }
            }
            Section::SolutionProposal => {
                let Some(breakdown) = self.approved_at(Section::Breakdown) else {
                    return vec!["`Breakdown` has no approving verdict".to_owned()];
                };
                let scope = self.approved_at(Section::Scope).unwrap_or(0);
                if !self.confirmed_after(RecordRef::Risks, scope) {
                    missing.push(format!(
                        "`risks` confirmed after the `Scope` verdict (seq {scope})"
                    ));
                }
                if self.spikes_satisfactory_after(breakdown) == 0 {
                    missing.push(format!(
                        "at least one spike scored satisfactory after the `Breakdown` verdict (seq {breakdown})"
                    ));
                }
            }
            _ => {}
        }
        missing
    }

    /// The one legal next step.
    #[must_use]
    pub fn next_step(&self) -> NextStep {
        if self.status != Status::Open {
            return NextStep::Frozen;
        }
        let Some(section) = self.open() else {
            return NextStep::Accept;
        };
        let missing = self.gate_missing(section);
        if !missing.is_empty() {
            return NextStep::OpenGate { section, missing };
        }
        if self.written.is_none() {
            return NextStep::Write(section);
        }
        match &self.audit {
            None => NextStep::Write(section),
            Some(audit) if audit.errors > 0 => NextStep::FixAudit {
                section,
                errors: audit.errors,
            },
            Some(_) => NextStep::Verdict(section),
        }
    }

    /// Whether `event` may be appended now.
    ///
    /// # Errors
    ///
    /// The [`Illegal`] reason when it may not.
    pub fn check(&self, event: &DiscoveryEvent) -> Result<(), Illegal> {
        if matches!(event, DiscoveryEvent::Started { .. }) {
            return Err(Illegal::AlreadyStarted);
        }
        if self.status != Status::Open {
            return Err(Illegal::Frozen(self.status));
        }
        if matches!(event, DiscoveryEvent::Refused { .. }) {
            return Ok(());
        }
        match event {
            DiscoveryEvent::SectionWritten { section, .. } => {
                let section = self.require_open(*section)?;
                let missing = self.gate_missing(section);
                if missing.is_empty() {
                    Ok(())
                } else {
                    Err(Illegal::GateClosed { section, missing })
                }
            }
            DiscoveryEvent::Audited { section, .. } => {
                let section = self.require_open(*section)?;
                if self.written.is_none() {
                    return Err(Illegal::Missing {
                        section,
                        what: "written",
                    });
                }
                Ok(())
            }
            DiscoveryEvent::Verdict {
                section,
                choice,
                by,
                ..
            } => {
                let section = self.require_open(*section)?;
                let Some(audit) = &self.audit else {
                    return Err(Illegal::Missing {
                        section,
                        what: "audited",
                    });
                };
                if !choice.approves() {
                    return Ok(());
                }
                self.check_approval(section, audit, by)
            }
            DiscoveryEvent::Reopened { section, .. } => {
                if !section.in_level(self.level) {
                    return Err(Illegal::NotInLevel(*section));
                }
                if self.approved.contains_key(section) {
                    Ok(())
                } else {
                    Err(Illegal::NothingToReopen(*section))
                }
            }
            DiscoveryEvent::SpikeScored { spike, .. } => {
                if self.spikes.contains_key(spike) {
                    Ok(())
                } else {
                    Err(Illegal::UnknownSpike(*spike))
                }
            }
            DiscoveryEvent::Accepted { by } => {
                if let Some(open) = self.open() {
                    return Err(Illegal::StillOpen(open));
                }
                if by.is_human() {
                    Ok(())
                } else {
                    Err(Illegal::Forbidden {
                        section: Section::SolutionProposal,
                        by: by.clone(),
                        why: "only a person accepts a discovery".to_owned(),
                    })
                }
            }
            DiscoveryEvent::SourceFetched { .. }
            | DiscoveryEvent::RecordChecked { .. }
            | DiscoveryEvent::SpikeStarted { .. }
            | DiscoveryEvent::Aborted => Ok(()),
            DiscoveryEvent::Started { .. } | DiscoveryEvent::Refused { .. } => Ok(()),
        }
    }

    fn require_open(&self, given: Section) -> Result<Section, Illegal> {
        if !given.in_level(self.level) {
            return Err(Illegal::NotInLevel(given));
        }
        match self.open() {
            Some(open) if open == given => Ok(open),
            Some(open) => Err(Illegal::NotOpen {
                given,
                open: open.heading().to_owned(),
            }),
            None => Err(Illegal::NotOpen {
                given,
                open: "none: every section is approved".to_owned(),
            }),
        }
    }

    fn check_approval(&self, section: Section, audit: &Audit, by: &Actor) -> Result<(), Illegal> {
        let forbidden = |why: &str| Illegal::Forbidden {
            section,
            by: by.clone(),
            why: why.to_owned(),
        };
        if by.is_human() {
            return Ok(());
        }
        if audit.errors > 0 {
            return Err(forbidden(&format!(
                "the latest audit has {} error(s)",
                audit.errors
            )));
        }
        match (section.verdict_policy(self.level), by) {
            (VerdictPolicy::Human, _) => Err(forbidden("a person must approve this section")),
            (VerdictPolicy::AuditOnly, Actor::Process(_)) => Ok(()),
            (VerdictPolicy::AuditOnly, Actor::Model(_)) => Err(forbidden(
                "this section is approved by its audit, not by a model",
            )),
            (VerdictPolicy::Evaluator, Actor::Model(_)) => {
                if audit.isolated {
                    Ok(())
                } else {
                    Err(forbidden(
                        "a model may approve only after an isolated audit",
                    ))
                }
            }
            (VerdictPolicy::Evaluator, Actor::Process(_)) => {
                Err(forbidden("this section needs an evaluator or a person"))
            }
            (_, Actor::Human(_)) => Ok(()),
        }
    }

    /// Checks `event` and applies it.
    ///
    /// # Errors
    ///
    /// As [`DiscoveryState::check`]. The state is untouched on error.
    pub fn apply(&mut self, logged: &Logged) -> Result<(), Illegal> {
        self.check(&logged.event)?;
        let seq = logged.seq;
        match &logged.event {
            DiscoveryEvent::Started { .. } => return Err(Illegal::AlreadyStarted),
            DiscoveryEvent::SourceFetched {
                source,
                hash,
                location,
                kind,
            } => {
                self.sources.insert(
                    *source,
                    SourceState {
                        seq,
                        hash: hash.clone(),
                        location: location.clone(),
                        kind: *kind,
                    },
                );
            }
            DiscoveryEvent::SectionWritten { hash, .. } => {
                self.written = Some(hash.clone());
                self.audit = None;
            }
            DiscoveryEvent::Audited {
                errors,
                warnings,
                isolated,
                ..
            } => {
                self.audit = Some(Audit {
                    seq,
                    errors: *errors,
                    warnings: *warnings,
                    isolated: *isolated,
                });
            }
            DiscoveryEvent::Verdict {
                section,
                choice,
                by,
                note,
            } => {
                if choice.approves() {
                    self.approved.insert(
                        *section,
                        Approval {
                            seq,
                            by: by.clone(),
                            choice: *choice,
                            note: note.clone(),
                        },
                    );
                }
                self.written = None;
                self.audit = None;
            }
            DiscoveryEvent::RecordChecked {
                record,
                verdict,
                by,
            } => {
                self.records.insert(
                    *record,
                    RecordState {
                        seq,
                        verdict: *verdict,
                        by: by.clone(),
                    },
                );
            }
            DiscoveryEvent::SpikeStarted {
                spike,
                worktree,
                branch,
            } => {
                self.spikes.insert(
                    *spike,
                    SpikeState {
                        worktree: worktree.clone(),
                        branch: branch.clone(),
                        started_seq: seq,
                        score: None,
                    },
                );
            }
            DiscoveryEvent::SpikeScored { spike, score } => {
                if let Some(s) = self.spikes.get_mut(spike) {
                    s.score = Some((*score, seq));
                }
            }
            DiscoveryEvent::Reopened { section, .. } => {
                self.approved.retain(|s, _| s < section);
                self.written = None;
                self.audit = None;
                self.reopens += 1;
            }
            DiscoveryEvent::Accepted { .. } => self.status = Status::Accepted,
            DiscoveryEvent::Aborted => self.status = Status::Aborted,
            DiscoveryEvent::Refused { .. } => self.refusals += 1,
        }
        self.last_seq = seq;
        Ok(())
    }
}

/// Folds `events` into a [`DiscoveryState`].
///
/// # Errors
///
/// [`ReplayError::Sequence`] when sequences are not contiguous from 1;
/// [`ReplayError::Illegal`] when an event was illegal where it stands,
/// including a first event that is not `Started`.
pub fn replay(events: &[Logged]) -> Result<DiscoveryState, ReplayError> {
    let mut iter = events.iter();
    let Some(first) = iter.next() else {
        return Err(ReplayError::Illegal {
            seq: 0,
            reason: Illegal::NotStarted,
        });
    };
    if first.seq != 1 {
        return Err(ReplayError::Sequence {
            expected: 1,
            found: first.seq,
        });
    }
    let DiscoveryEvent::Started { problem, level } = &first.event else {
        return Err(ReplayError::Illegal {
            seq: first.seq,
            reason: Illegal::NotStarted,
        });
    };
    let mut state = DiscoveryState::new(problem.clone(), *level);
    state.last_seq = 1;
    for logged in iter {
        let expected = state.last_seq + 1;
        if logged.seq != expected {
            return Err(ReplayError::Sequence {
                expected,
                found: logged.seq,
            });
        }
        state.apply(logged).map_err(|reason| ReplayError::Illegal {
            seq: logged.seq,
            reason,
        })?;
    }
    Ok(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn human() -> Actor {
        Actor::Human("alex".into())
    }

    fn model() -> Actor {
        Actor::Model("claude-opus-5-5".into())
    }

    fn audit_process() -> Actor {
        Actor::Process("audit".into())
    }

    /// Appends events, numbering them, and fails on the first illegal one.
    struct Log(Vec<Logged>);

    impl Log {
        fn start(level: Level) -> Self {
            Self(vec![Logged::new(
                1,
                1,
                DiscoveryEvent::Started {
                    problem: "slow sessions".into(),
                    level,
                },
            )])
        }

        fn push(&mut self, event: DiscoveryEvent) -> &mut Self {
            let seq = self.0.len() as u64 + 1;
            self.0.push(Logged::new(seq, seq, event));
            self
        }

        fn state(&self) -> DiscoveryState {
            replay(&self.0).unwrap()
        }

        fn error(&self) -> ReplayError {
            replay(&self.0).unwrap_err()
        }

        fn write(&mut self, section: Section) -> &mut Self {
            self.push(DiscoveryEvent::SectionWritten {
                section,
                hash: "h".into(),
            })
        }

        fn audit(&mut self, section: Section, errors: u32, isolated: bool) -> &mut Self {
            self.push(DiscoveryEvent::Audited {
                section,
                errors,
                warnings: 0,
                isolated,
                by: audit_process(),
            })
        }

        fn approve(&mut self, section: Section, by: Actor) -> &mut Self {
            self.push(DiscoveryEvent::Verdict {
                section,
                choice: Verdict::ApproveAsIs,
                by,
                note: None,
            })
        }

        fn confirm(&mut self, record: RecordRef) -> &mut Self {
            self.push(DiscoveryEvent::RecordChecked {
                record,
                verdict: RecordVerdict::Confirmed,
                by: model(),
            })
        }

        /// Write, audit clean and approve `section` by `by`.
        fn pass(&mut self, section: Section, by: Actor) -> &mut Self {
            self.write(section)
                .audit(section, 0, true)
                .approve(section, by)
        }

        fn framing(&mut self) -> &mut Self {
            for s in [
                Section::Goal,
                Section::Background,
                Section::Constraints,
                Section::Scope,
            ] {
                self.pass(s, human());
            }
            self
        }

        fn method_records(&mut self) -> &mut Self {
            self.confirm(RecordRef::Structure)
                .confirm(RecordRef::Variables)
                .confirm(RecordRef::Claim { id: 1 })
                .confirm(RecordRef::Cld)
        }
    }

    #[test]
    fn a_new_discovery_opens_at_goal_and_asks_to_write_it() {
        let st = Log::start(Level::Research).state();
        assert_eq!(st.open(), Some(Section::Goal));
        assert_eq!(st.phase(), Phase::Framing);
        assert_eq!(st.next_step(), NextStep::Write(Section::Goal));
    }

    #[test]
    fn the_first_event_must_be_started_at_seq_1() {
        assert_eq!(
            replay(&[]).unwrap_err(),
            ReplayError::Illegal {
                seq: 0,
                reason: Illegal::NotStarted
            }
        );
        let wrong = Logged::new(1, 1, DiscoveryEvent::Aborted);
        assert!(matches!(
            replay(&[wrong]).unwrap_err(),
            ReplayError::Illegal {
                reason: Illegal::NotStarted,
                ..
            }
        ));
        let mut log = Log::start(Level::Deep);
        log.0.push(Logged::new(5, 5, DiscoveryEvent::Aborted));
        assert_eq!(
            log.error(),
            ReplayError::Sequence {
                expected: 2,
                found: 5
            }
        );
    }

    #[test]
    fn write_audit_verdict_in_order() {
        let mut log = Log::start(Level::Deep);
        assert!(matches!(
            log.audit(Section::Goal, 0, false).error(),
            ReplayError::Illegal {
                seq: 2,
                reason: Illegal::Missing {
                    what: "written",
                    ..
                }
            }
        ));
        let mut log = Log::start(Level::Deep);
        log.write(Section::Goal);
        assert_eq!(log.state().next_step(), NextStep::Write(Section::Goal));
        assert!(matches!(
            log.approve(Section::Goal, human()).error(),
            ReplayError::Illegal {
                reason: Illegal::Missing {
                    what: "audited",
                    ..
                },
                ..
            }
        ));
        let mut log = Log::start(Level::Deep);
        log.write(Section::Goal).audit(Section::Goal, 2, false);
        assert_eq!(
            log.state().next_step(),
            NextStep::FixAudit {
                section: Section::Goal,
                errors: 2
            }
        );
        log.write(Section::Goal).audit(Section::Goal, 0, false);
        assert_eq!(log.state().next_step(), NextStep::Verdict(Section::Goal));
        log.approve(Section::Goal, human());
        let st = log.state();
        assert_eq!(st.open(), Some(Section::Background));
        assert_eq!(st.approved_at(Section::Goal), Some(6));
    }

    #[test]
    fn a_rewrite_clears_the_audit() {
        let mut log = Log::start(Level::Research);
        log.write(Section::Goal)
            .audit(Section::Goal, 0, false)
            .write(Section::Goal);
        assert_eq!(log.state().next_step(), NextStep::Write(Section::Goal));
    }

    #[test]
    fn only_the_open_section_can_be_written() {
        let mut log = Log::start(Level::Research);
        assert!(matches!(
            log.write(Section::Scope).error(),
            ReplayError::Illegal {
                reason: Illegal::NotOpen {
                    given: Section::Scope,
                    ..
                },
                ..
            }
        ));
        let mut log = Log::start(Level::Research);
        assert!(matches!(
            log.write(Section::CurrentSystem).error(),
            ReplayError::Illegal {
                reason: Illegal::NotInLevel(Section::CurrentSystem),
                ..
            }
        ));
    }

    #[test]
    fn a_reject_keeps_the_section_open_and_needs_a_new_audit() {
        let mut log = Log::start(Level::Research);
        log.write(Section::Goal).audit(Section::Goal, 0, false);
        log.push(DiscoveryEvent::Verdict {
            section: Section::Goal,
            choice: Verdict::RejectAndRevise,
            by: human(),
            note: Some("name the user".into()),
        });
        let st = log.state();
        assert_eq!(st.open(), Some(Section::Goal));
        assert_eq!(st.next_step(), NextStep::Write(Section::Goal));
    }

    #[test]
    fn framing_is_approved_by_people_only() {
        let mut log = Log::start(Level::Deep);
        log.write(Section::Goal).audit(Section::Goal, 0, true);
        assert!(matches!(
            log.approve(Section::Goal, model()).error(),
            ReplayError::Illegal {
                reason: Illegal::Forbidden { .. },
                ..
            }
        ));
    }

    #[test]
    fn research_constraints_are_approved_by_the_audit_itself() {
        let mut log = Log::start(Level::Research);
        log.pass(Section::Goal, human())
            .pass(Section::Background, human())
            .write(Section::Constraints)
            .audit(Section::Constraints, 0, false)
            .approve(Section::Constraints, audit_process());
        assert_eq!(log.state().open(), Some(Section::Scope));

        let mut log = Log::start(Level::Research);
        log.pass(Section::Goal, human())
            .pass(Section::Background, human())
            .write(Section::Constraints)
            .audit(Section::Constraints, 1, false);
        assert!(matches!(
            log.approve(Section::Constraints, audit_process()).error(),
            ReplayError::Illegal {
                reason: Illegal::Forbidden { .. },
                ..
            }
        ));
    }

    #[test]
    fn a_model_approves_deep_sections_only_after_a_clean_isolated_audit() {
        let mut log = Log::start(Level::Deep);
        log.framing().method_records();
        log.write(Section::Findings)
            .audit(Section::Findings, 0, false);
        assert!(matches!(
            log.approve(Section::Findings, model()).error(),
            ReplayError::Illegal {
                reason: Illegal::Forbidden { .. },
                ..
            }
        ));
        let mut log = Log::start(Level::Deep);
        log.framing().method_records();
        log.write(Section::Findings)
            .audit(Section::Findings, 0, true)
            .approve(Section::Findings, model());
        assert_eq!(log.state().open(), Some(Section::CurrentSystem));
    }

    #[test]
    fn the_findings_gate_needs_records_confirmed_after_scope() {
        let mut log = Log::start(Level::Deep);
        // Records confirmed before Scope is approved do not count.
        log.method_records().framing();
        let st = log.state();
        let missing = st.gate_missing(Section::Findings);
        assert_eq!(missing.len(), 4, "{missing:?}");
        assert!(matches!(
            st.next_step(),
            NextStep::OpenGate {
                section: Section::Findings,
                ..
            }
        ));
        assert!(matches!(
            log.write(Section::Findings).error(),
            ReplayError::Illegal {
                reason: Illegal::GateClosed { .. },
                ..
            }
        ));

        let mut log = Log::start(Level::Deep);
        log.framing().method_records();
        let st = log.state();
        assert!(st.gate_missing(Section::Findings).is_empty());
        assert_eq!(st.next_step(), NextStep::Write(Section::Findings));
    }

    #[test]
    fn the_research_level_has_no_gates() {
        let mut log = Log::start(Level::Research);
        log.framing();
        let st = log.state();
        assert!(st.gate_missing(Section::Findings).is_empty());
        assert_eq!(st.next_step(), NextStep::Write(Section::Findings));
    }

    #[test]
    fn reopening_scope_drops_later_verdicts_and_closes_the_gate_again() {
        let mut log = Log::start(Level::Deep);
        log.framing()
            .method_records()
            .pass(Section::Findings, model())
            .pass(Section::CurrentSystem, model());
        log.push(DiscoveryEvent::Reopened {
            section: Section::Scope,
            reason: "narrow the scope to the cache".into(),
        });
        let st = log.state();
        assert_eq!(st.open(), Some(Section::Scope));
        assert!(st.approved.contains_key(&Section::Constraints));
        assert!(!st.approved.contains_key(&Section::Findings));
        assert_eq!(st.reopens, 1);
        // The records are still there, but once Scope is reapproved they
        // are older than its verdict and no longer count.
        assert_eq!(st.records.len(), 4);
        log.pass(Section::Scope, human());
        let st = log.state();
        assert_eq!(st.gate_missing(Section::Findings).len(), 4);
        log.method_records();
        assert!(log.state().gate_missing(Section::Findings).is_empty());
    }

    #[test]
    fn reopening_needs_a_verdict_to_drop() {
        let mut log = Log::start(Level::Research);
        assert!(matches!(
            log.push(DiscoveryEvent::Reopened {
                section: Section::Scope,
                reason: String::new()
            })
            .error(),
            ReplayError::Illegal {
                reason: Illegal::NothingToReopen(Section::Scope),
                ..
            }
        ));
    }

    #[test]
    fn the_solution_gate_needs_risks_and_a_satisfactory_spike_after_breakdown() {
        let mut log = Log::start(Level::Deep);
        log.framing()
            .method_records()
            .pass(Section::Findings, model())
            .pass(Section::CurrentSystem, model())
            .pass(Section::DesiredState, model())
            .pass(Section::Gap, model());
        // A spike scored before Breakdown does not count.
        log.push(DiscoveryEvent::SpikeStarted {
            spike: SpikeId(1),
            worktree: ".crest/spikes/sp1".into(),
            branch: "crest/spike-sp1".into(),
        })
        .push(DiscoveryEvent::SpikeScored {
            spike: SpikeId(1),
            score: SpikeScore::Satisfactory,
        })
        .pass(Section::Breakdown, model());
        let st = log.state();
        let missing = st.gate_missing(Section::SolutionProposal);
        assert_eq!(missing.len(), 2, "{missing:?}");

        log.confirm(RecordRef::Risks)
            .push(DiscoveryEvent::SpikeStarted {
                spike: SpikeId(2),
                worktree: ".crest/spikes/sp2".into(),
                branch: "crest/spike-sp2".into(),
            });
        let st = log.state();
        assert_eq!(st.unfinished_spikes().count(), 1);
        assert_eq!(st.gate_missing(Section::SolutionProposal).len(), 1);
        log.push(DiscoveryEvent::SpikeScored {
            spike: SpikeId(2),
            score: SpikeScore::Satisfactory,
        });
        let st = log.state();
        assert!(st.gate_missing(Section::SolutionProposal).is_empty());
        assert_eq!(st.unfinished_spikes().count(), 0);
    }

    #[test]
    fn scoring_an_unknown_spike_is_illegal() {
        let mut log = Log::start(Level::Deep);
        assert!(matches!(
            log.push(DiscoveryEvent::SpikeScored {
                spike: SpikeId(9),
                score: SpikeScore::NotSatisfactory
            })
            .error(),
            ReplayError::Illegal {
                reason: Illegal::UnknownSpike(SpikeId(9)),
                ..
            }
        ));
    }

    #[test]
    fn accept_needs_every_section_approved_and_a_person() {
        let mut log = Log::start(Level::Research);
        log.framing();
        assert!(matches!(
            log.push(DiscoveryEvent::Accepted { by: human() }).error(),
            ReplayError::Illegal {
                reason: Illegal::StillOpen(Section::Findings),
                ..
            }
        ));
        let mut log = Log::start(Level::Research);
        log.framing()
            .write(Section::Findings)
            .audit(Section::Findings, 0, false)
            .approve(Section::Findings, audit_process())
            .pass(Section::Breakdown, human())
            .pass(Section::SolutionProposal, human());
        let st = log.state();
        assert_eq!(st.open(), None);
        assert_eq!(st.next_step(), NextStep::Accept);
        assert!(matches!(
            log.push(DiscoveryEvent::Accepted { by: model() }).error(),
            ReplayError::Illegal {
                reason: Illegal::Forbidden { .. },
                ..
            }
        ));
        let mut log = Log::start(Level::Research);
        log.framing()
            .write(Section::Findings)
            .audit(Section::Findings, 0, false)
            .approve(Section::Findings, audit_process())
            .pass(Section::Breakdown, human())
            .pass(Section::SolutionProposal, human())
            .push(DiscoveryEvent::Accepted { by: human() });
        let st = log.state();
        assert_eq!(st.status, Status::Accepted);
        assert_eq!(st.next_step(), NextStep::Frozen);
        assert!(matches!(
            log.write(Section::Goal).error(),
            ReplayError::Illegal {
                reason: Illegal::Frozen(Status::Accepted),
                ..
            }
        ));
    }

    #[test]
    fn refusals_are_recorded_while_open_and_never_once_frozen() {
        let mut log = Log::start(Level::Research);
        log.push(DiscoveryEvent::Refused {
            what: "write Findings".into(),
            why: "not the open section".into(),
        });
        assert_eq!(log.state().refusals, 1);
        log.push(DiscoveryEvent::Aborted);
        assert!(matches!(
            log.push(DiscoveryEvent::Refused {
                what: "write Goal".into(),
                why: "aborted".into(),
            })
            .error(),
            ReplayError::Illegal {
                reason: Illegal::Frozen(Status::Aborted),
                ..
            }
        ));
        log.0.pop();
        let st = log.state();
        assert_eq!(st.status, Status::Aborted);
        assert_eq!(st.refusals, 1);
    }

    #[test]
    fn the_state_serializes() {
        let mut log = Log::start(Level::Deep);
        log.framing();
        let st = log.state();
        let json = serde_json::to_string(&st).unwrap();
        let back: DiscoveryState = serde_json::from_str(&json).unwrap();
        assert_eq!(back, st);
    }

    /// A tiny deterministic generator (LCG) for the property tests below.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            self.0 >> 33
        }

        fn below(&mut self, n: u64) -> u64 {
            self.next() % n.max(1)
        }

        fn pick<T: Copy>(&mut self, items: &[T]) -> T {
            items[self.below(items.len() as u64) as usize]
        }
    }

    /// Any event the vocabulary allows, on a random section and actor.
    fn random_event(rng: &mut Rng, level: Level) -> DiscoveryEvent {
        let sections = level.sections();
        let section = rng.pick(sections);
        let actor = match rng.below(3) {
            0 => human(),
            1 => model(),
            _ => audit_process(),
        };
        match rng.below(11) {
            0 | 1 => DiscoveryEvent::SectionWritten {
                section,
                hash: "h".into(),
            },
            2 | 3 => DiscoveryEvent::Audited {
                section,
                errors: u32::try_from(rng.below(2)).unwrap_or(0),
                warnings: 0,
                isolated: rng.below(2) == 0,
                by: actor,
            },
            4 | 5 => DiscoveryEvent::Verdict {
                section,
                choice: rng.pick(&[
                    Verdict::ApproveAsIs,
                    Verdict::ApproveWithNotes,
                    Verdict::RejectAndRevise,
                ]),
                by: actor,
                note: None,
            },
            6 => DiscoveryEvent::RecordChecked {
                record: rng.pick(&[
                    RecordRef::Structure,
                    RecordRef::Variables,
                    RecordRef::Cld,
                    RecordRef::Risks,
                    RecordRef::Claim { id: 1 },
                    RecordRef::Evidence { id: 1 },
                ]),
                verdict: RecordVerdict::Confirmed,
                by: actor,
            },
            7 => DiscoveryEvent::SpikeStarted {
                spike: SpikeId(1),
                worktree: "w".into(),
                branch: "b".into(),
            },
            8 => DiscoveryEvent::SpikeScored {
                spike: SpikeId(1),
                score: SpikeScore::Satisfactory,
            },
            9 => DiscoveryEvent::Reopened {
                section,
                reason: "r".into(),
            },
            _ => DiscoveryEvent::Accepted { by: actor },
        }
    }

    /// Builds a random legal log of up to `steps` events at `level`,
    /// checking two invariants after every append:
    ///
    /// - an event that is not a `Reopened` never changes the sequence of
    ///   an earlier approval, and never removes one;
    /// - a `Reopened` of `s` leaves no approval of `s` or any later
    ///   section visible.
    fn legal_log_keeps_the_invariants(seed: u64, level: Level, steps: usize) -> usize {
        let mut rng = Rng(seed);
        let mut log = Log::start(level);
        let mut appended = 0;
        for _ in 0..steps {
            let event = random_event(&mut rng, level);
            let before = log.state();
            if before.check(&event).is_err() {
                continue;
            }
            log.push(event.clone());
            appended += 1;
            let after = log.state();
            match &event {
                DiscoveryEvent::Reopened { section, .. } => {
                    assert!(
                        after.approved.keys().all(|s| s < section),
                        "seed {seed}: approvals at or after {section} survived a reopen: {:?}",
                        after.approved.keys().collect::<Vec<_>>()
                    );
                }
                _ => {
                    for (section, approval) in &before.approved {
                        let kept = after.approved.get(section).unwrap_or_else(|| {
                            panic!("seed {seed}: {section} lost its approval on {event:?}")
                        });
                        assert_eq!(
                            kept.seq, approval.seq,
                            "seed {seed}: the approval of {section} changed its seq on {event:?}"
                        );
                    }
                }
            }
            if after.status != Status::Open {
                break;
            }
        }
        appended
    }

    #[test]
    fn random_legal_logs_never_make_an_earlier_verdict_newer_and_reopen_hides_later_ones() {
        let mut appended = 0;
        for seed in 0..200 {
            appended += legal_log_keeps_the_invariants(seed, Level::Research, 60);
            appended += legal_log_keeps_the_invariants(seed, Level::Deep, 80);
        }
        // The generator reaches real logs, not only refusals.
        assert!(appended > 2_000, "only {appended} legal events in 400 runs");
    }

    #[test]
    fn evidence_records_are_cited_as_e_and_do_not_open_gates() {
        let mut log = Log::start(Level::Deep);
        log.framing().confirm(RecordRef::Evidence { id: 1 });
        let st = log.state();
        assert_eq!(RecordRef::Evidence { id: 1 }.to_string(), "E1");
        assert!(st.records.contains_key(&RecordRef::Evidence { id: 1 }));
        let missing = st.gate_missing(Section::Findings);
        assert_eq!(missing.len(), 4, "{missing:?}");
    }
}
