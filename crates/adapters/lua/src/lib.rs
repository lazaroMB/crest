//! PluginHostPort adapter with mlua.
//!
//! The Lua VM runs on its own thread ([`LuaHost`]); the app talks to it
//! through a [`LuaHandle`], which implements
//! [`PluginHostPort`](application::ports::plugin::PluginHostPort).

mod api;
mod callbacks;
mod caps;
mod config;
mod convert;
mod host;
mod sandbox;
mod themes;
mod vm;

pub use config::default_sources;
pub use host::{HostOptions, LuaHandle, LuaHost};
pub use themes::{LuaThemeSource, THEMES_DIR};
