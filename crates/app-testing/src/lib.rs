//! In-memory fakes of every port of `application-core`, for tests without
//! network, `rg` or UI.
//!
//! The fakes are written against `crate::ports`, so `application-core` and
//! `application` mount the same files (`src/fakes/`) as their own `testing`
//! module for their unit tests. Here `crate::ports` resolves to
//! `application_core::ports` through the import below. Keeping them out of
//! the application crates behind a feature avoids a second build of them and
//! of every crate above them when tests run.
//!
//! Use it only under `[dev-dependencies]` (`scripts/check-arch.sh` checks it).

use application_core::ports;

mod fakes;

pub use fakes::*;
