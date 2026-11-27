//! Use cases: the agent loop and the flows built on it.
//!
//! All but `simulate` live in `application-core` and are re-exported here;
//! `simulate` stays because it builds on the research flow's spec writer.

pub use application_core::usecases::*;

pub mod simulate;

pub mod background;
