//! In-memory fakes of every port of `application`, for tests without
//! network, `rg` or UI.
//!
//! The fakes are written against `crate::ports`, so `application` mounts the
//! same files (`src/fakes/`) as its own `testing` module for its unit tests.
//! Here `crate::ports` resolves to `application::ports` through the import
//! below. Keeping them out of `application` behind a feature avoids a second
//! build of `application` and of every crate above it when tests run.
//!
//! Use it only under `[dev-dependencies]` (`scripts/check-arch.sh` checks it).

use application::ports;

mod fakes;

pub use fakes::*;
