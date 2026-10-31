//! The integration tests of this crate, as modules of one test binary.
//!
//! One binary instead of one per file: each test binary links the whole
//! dependency tree again, which dominated the build time of `cargo test`.
//! Add a new test file here as a module.

mod bench;
mod datasets;
mod devops;
mod diagrams;
mod engineering;
mod formats;
mod har;
mod query_plan;
mod science;
mod science_b;
mod terraform;
