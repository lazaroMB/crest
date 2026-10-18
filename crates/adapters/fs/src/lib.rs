//! FileSystemPort adapter with workspace guards.
//!
//! [`guard::WorkspaceGuard`] confines every path to the canonical workspace
//! root, resolving symlinks so that none can escape it.

pub mod guard;

pub use guard::WorkspaceGuard;
