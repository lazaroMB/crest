//! [`ComputePort`] adapter: `crest-stats`, the deterministic statistics
//! engine behind the statistics cards.
//!
//! Descriptives, t-tests, rank tests, contingency tests, OLS and
//! correlation matrices, written on statrs' distributions and special
//! functions. Every call returns a [`Computed`] envelope filled in full:
//! values with their APA display, the method and why it was chosen,
//! assumption checks, misuse warnings, a BLAKE3 hash of the data read,
//! the engine and platform, reproduce lines for crest Lua, Python and R,
//! the narrative template and provenance. The request is stored in the
//! envelope, so [`ComputePort::recompute`] re-runs it and records whether
//! every value matched.
//!
//! Reference tests (`tests/reference.rs`) check the engine against NIST
//! StRD certified values and examples from R's documentation.

mod checks;
mod correlate;
mod data;
mod describe;
mod envelope;
mod linalg;
mod math;
mod ols;
mod rank;
mod reproduce;
mod tests;

use std::sync::Arc;
use std::time::SystemTime;

use application::ports::compute::{
    ComputeError, ComputePort, ComputeRequest, ComputeResult, CorrelateRequest, DescribeRequest,
    RegressRequest, TestRequest,
};
use domain::computed::Computed;

pub use envelope::{BACKEND, ENGINE_NAME};

/// The time source; injected so tests get fixed timestamps.
pub type Clock = Arc<dyn Fn() -> SystemTime + Send + Sync>;

/// The `crest-stats` engine.
#[derive(Clone)]
pub struct StatsEngine {
    clock: Clock,
}

impl Default for StatsEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for StatsEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StatsEngine").finish_non_exhaustive()
    }
}

impl StatsEngine {
    /// An engine on the system clock.
    #[must_use]
    pub fn new() -> Self {
        Self::with_clock(Arc::new(SystemTime::now))
    }

    /// An engine reading the time from `clock`.
    #[must_use]
    pub fn with_clock(clock: Clock) -> Self {
        Self { clock }
    }

    fn execute(&self, request: &ComputeRequest, id: Option<String>) -> ComputeResult {
        let started = (self.clock)();
        let id = id.unwrap_or_else(|| envelope::derive_id(request));
        let (mut computed, body) = match request {
            ComputeRequest::Describe(r) => describe::run(r, id)?,
            ComputeRequest::Test(r) => tests::run(r, id)?,
            ComputeRequest::Regress(r) => ols::run(r, id)?,
            ComputeRequest::Correlate(r) => correlate::run(r, id)?,
        };
        if computed.reproduce.lua.is_none() {
            let json = serde_json::to_value(request).unwrap_or_default();
            computed.reproduce.lua = Some(reproduce::lua(function(request), &json, request.data()));
        }
        envelope::finish(&mut computed, request, started, (self.clock)());
        Ok((computed, body))
    }
}

/// The Lua function of a request kind.
fn function(request: &ComputeRequest) -> &'static str {
    match request {
        ComputeRequest::Describe(_) => "describe",
        ComputeRequest::Test(_) => "test",
        ComputeRequest::Regress(_) => "regress",
        ComputeRequest::Correlate(_) => "correlate",
    }
}

impl ComputePort for StatsEngine {
    fn describe(&self, request: &DescribeRequest) -> ComputeResult {
        self.execute(&ComputeRequest::Describe(request.clone()), None)
    }

    fn test(&self, request: &TestRequest) -> ComputeResult {
        self.execute(&ComputeRequest::Test(request.clone()), None)
    }

    fn regress(&self, request: &RegressRequest) -> ComputeResult {
        self.execute(&ComputeRequest::Regress(request.clone()), None)
    }

    fn correlate(&self, request: &CorrelateRequest) -> ComputeResult {
        self.execute(&ComputeRequest::Correlate(request.clone()), None)
    }

    fn recompute(&self, envelope: &Computed) -> ComputeResult {
        if envelope.engine.name != ENGINE_NAME {
            return Err(ComputeError::NotRecomputable(format!(
                "envelope {} comes from {}, not {ENGINE_NAME}",
                envelope.id, envelope.engine.name
            )));
        }
        let request = ComputeRequest::from_envelope(envelope)?;
        let (mut computed, body) = self.execute(&request, Some(envelope.id.clone()))?;
        let diff = envelope.diff_values(&computed, 0.0);
        computed.verification.recomputed_at = Some(envelope::rfc3339((self.clock)()));
        computed.verification.matched = Some(diff.is_empty() && envelope.status == computed.status);
        computed.verification.diff = diff;
        Ok((computed, body))
    }
}
