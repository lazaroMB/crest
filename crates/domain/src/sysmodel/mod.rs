//! Executable sketches of a proposed solution: the `crest.sysmodel/v1`
//! model, its expression language, lint, a deterministic simulator, the
//! recorded traces, the assertion checker, the board layout and a
//! renderer-agnostic 3D scene.
//!
//! ```text
//! SysModel ──lint──▶ Issues
//!    │
//!    ├──simulate(scenario, seed)──▶ Trace ──check──▶ Verdicts (+ counterexamples)
//!    │
//!    └──layout──▶ Layout ──build_scene──▶ Scene ──frame_at(trace, t)──▶ Frame
//! ```
//!
//! Everything here is pure: same model and seed, same trace.

pub mod check;
pub mod expr;
pub mod layout;
pub mod lint;
pub mod model;
pub mod scene;
pub mod sim;
pub mod trace;

#[cfg(test)]
mod tests;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub use check::{Counterexample, Origin, ScenarioRun, Verdict, check, run_all, run_scenario};
pub use lint::{Issue, Severity, lint};
pub use model::{SYSMODEL_VERSION, Scenario, SysModel, schema};
pub use sim::{SimError, simulate};
pub use trace::{EventKind, Snapshot, Trace, TraceEvent};

/// A simulated solution as the board shows it: the model and one run per
/// scenario.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Board {
    /// Spec folder, e.g. `0007-session-cache`.
    pub spec: String,
    /// Title of the Spec.
    pub title: String,
    /// The model.
    pub model: SysModel,
    /// Runs, in scenario order.
    pub runs: Vec<ScenarioRun>,
}

impl Board {
    /// Whether every scenario passed.
    #[must_use]
    pub fn passed(&self) -> bool {
        self.runs.iter().all(|r| r.passed)
    }

    /// The run of scenario `id`.
    #[must_use]
    pub fn run(&self, id: &str) -> Option<&ScenarioRun> {
        self.runs.iter().find(|r| r.scenario == id)
    }
}
