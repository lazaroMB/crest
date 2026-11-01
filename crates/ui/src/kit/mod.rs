//! crest's design system: the shape tokens and the shared components the
//! views and blocks are built from.
//!
//! Colors come from [`crate::theme::token`]; sizes (spacing, radii, text)
//! from [`tokens`], so a theme's `shape` table reaches every view that
//! uses them.

pub mod tokens;

pub use tokens::{Radius, Space, TextRole, TokenStyled};
