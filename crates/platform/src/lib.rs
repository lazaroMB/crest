//! Operating-system glue shared by the adapters, the UI and the binary.
//!
//! Adapters may not depend on each other, so what several of them need to
//! run on Linux, macOS and Windows alike lives here: spawning and killing
//! child processes ([`process`]), crest's per-user directories ([`dirs`]),
//! the Windows console of the GUI binary ([`console`]) and the login shell's
//! environment for crest.app on macOS ([`login_shell`]), and the keyring
//! service of crest's secrets ([`KEYRING_SERVICE`]). Only `std`, `dirs`
//! and (on Windows) `windows-sys`; no internal crate.

pub mod console;
pub mod dirs;
pub mod login_shell;
pub mod process;

/// The system keyring service under which crest keeps every secret (API
/// keys, plugin secrets, MCP tokens), one entry per secret name.
pub const KEYRING_SERVICE: &str = "crest";
