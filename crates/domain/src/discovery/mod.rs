//! The discovery engine: a governed document whose state is an event log.
//!
//! A **discovery** is a Markdown document with a fixed set of sections
//! ([`Section`]) written one at a time, in order, at one of two
//! [`Level`]s. Its state is not "which step ran last" but *which sections
//! hold a verdict, given on which version of their inputs*: every write,
//! audit, verdict, record check and spike score is a [`DiscoveryEvent`]
//! with a sequence number, and [`replay`] folds the log into a
//! [`DiscoveryState`] that says what is established, what is open and what
//! the one legal next step is.
//!
//! Prototype of the design in `docs/research/discovery-flow.md`. This
//! module is pure: no I/O, no clock, no model. The application layer
//! appends events through a port, asks the state what is legal, and
//! renders the document and the status from the state.
//!
//! - [`section`]: sections, phases, levels, the format each section must
//!   follow and who may approve it.
//! - [`event`]: the event vocabulary and its actors.
//! - [`replay`]: the state, the legality of each event, gates over
//!   sequence numbers and the next legal step.
//! - [`contract`]: deterministic checks of a section's text (format,
//!   citations, forbidden words), shared by the writer and the audit.
//! - [`claim`]: causal claims with a verbatim span checked against the
//!   stored text of their source.
//! - [`cld`]: the causal loop diagram computed from confirmed claims.
//! - [`spike`]: a spike with its hypothesis and criteria frozen before it
//!   runs, scored against them afterwards.

pub mod claim;
pub mod cld;
pub mod contract;
pub mod event;
pub mod replay;
pub mod section;
pub mod spike;

pub use claim::{Claim, ClaimCheck, ClaimId, Direction, Sign, check_claim, check_claim_with};
pub use cld::{Cld, Loop, LoopKind};
pub use contract::{Citable, Finding, Severity, check_section};
pub use event::{
    Actor, DiscoveryEvent, Logged, RecordRef, RecordVerdict, SourceId, SourceKind, SpikeId,
    SpikeScore, Verdict,
};
pub use replay::{DiscoveryState, Illegal, NextStep, ReplayError, Status, replay};
pub use section::{Format, Level, Phase, Section, VerdictPolicy};
pub use spike::{Spike, SpikeError};
