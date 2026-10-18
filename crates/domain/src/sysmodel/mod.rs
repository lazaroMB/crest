//! Executable sketches of a proposed solution: the `crest.sysmodel/v1`
//! model, its expression language and lint.

pub mod expr;
pub mod lint;
pub mod model;

pub use lint::{Issue, Severity, lint};
pub use model::{SYSMODEL_VERSION, Scenario, SysModel, schema};
