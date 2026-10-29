//! The discovery engine: `/research` on an append-only event log.
//!
//! A discovery is a document of fixed sections written one at a time at
//! one of two levels ([`Level`](domain::discovery::Level)). Its state is
//! the replay of its log ([`domain::discovery`]); this module is the
//! application layer around it:
//!
//! - [`engine`]: [`DiscoveryEngine`](engine::DiscoveryEngine), the use
//!   case: `start`, `step`, `run`, `answer`, `reopen`, `abort`, `accept`,
//!   `status`. Every command loads the log, asks the state for the one
//!   legal next step and appends one event through the
//!   [`DiscoveryLogPort`](crate::ports::discovery::DiscoveryLogPort).
//! - [`packet`]: what a writer, reviewer or evaluator may see for one
//!   section: the request, the approved sections, the confirmed records
//!   with excerpts of their sources. Never a whole source.
//! - [`writers`]: one prompt per section quoting its format contract,
//!   retried with the deterministic findings.
//! - [`audit`]: the contract plus the isolated reviewer; [`evaluator`]:
//!   the isolated verdict and the reading of a person's answer.
//! - [`explore`], [`records`], [`spikes`], [`gates`]: the records of a
//!   deep discovery (sources, evidence, claims, structure, variables, the
//!   causal diagram, risks) and the spikes with frozen criteria that open
//!   its gates.
//! - [`lock`]: the folder of an open discovery is closed to the tools.
//! - [`render`]: `discovery.md`; [`derive`]: `spec.md`, `plan.json` and
//!   the rest of the Spec folder on accept, so `/implement` and
//!   `/simulate` read what they read today.
//! - [`session`]: [`DiscoverySession`](session::DiscoverySession), the
//!   engine as the chat and headless mode drive it.

pub mod audit;
pub mod derive;
pub mod engine;
pub mod error;
pub mod evaluator;
pub mod explore;
pub mod gates;
pub mod lock;
pub mod model;
pub mod packet;
pub mod records;
pub mod render;
pub mod session;
pub mod spikes;
pub mod writers;

#[cfg(test)]
mod tests;
