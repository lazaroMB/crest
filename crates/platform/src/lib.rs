//! Operating-system glue shared by the adapters, the UI and the binary.
//!
//! Adapters may not depend on each other, so what several of them need to
//! run on Linux, macOS and Windows alike lives here: spawning and killing
//! child processes ([`process`]), crest's per-user directories ([`dirs`]) and
//! the Windows console of the GUI binary ([`console`]). Only `std`, `dirs`
//! and (on Windows) `windows-sys`; no internal crate.

pub mod console;
pub mod dirs;
pub mod process;
