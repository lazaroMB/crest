//! crest's design system: the shape tokens and the shared components the
//! views and blocks are built from (see `docs/design-system.md`).
//!
//! Colors come from [`crate::theme::token`] (or gpui-component's
//! `cx.theme()`, which the theme feeds); sizes from [`tokens`], so a
//! theme's `shape` table reaches every view built from these.

pub mod button;
pub mod chip;
pub mod header;
pub mod tokens;

pub use button::{quiet_button, tool_button};
pub use chip::{ChipKind, chip, filter_chip, readout, toggle_chip};
pub use header::{caption, header, title, toolbar};
pub use tokens::{Radius, Space, TextRole, TokenStyled};
