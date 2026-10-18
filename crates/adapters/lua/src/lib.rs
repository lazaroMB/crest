//! PluginHostPort adapter with mlua.
//!
//! The Lua VM runs on its own thread ([`LuaHost`]); the app talks to it
//! through a [`LuaHandle`], which implements
//! [`PluginHostPort`](application::ports::plugin::PluginHostPort).

mod host;

pub use host::{HostOptions, LuaHandle, LuaHost};
