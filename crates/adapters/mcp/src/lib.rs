//! [`McpPort`](application::ports::mcp::McpPort) adapter with rmcp.
//!
//! [`McpManager`] connects to the servers declared in the config, over
//! stdio (a child process) or streamable HTTP, lists their tools and routes
//! calls to them. Approvals are not its business: the agent loop applies
//! them to MCP tools like to any other tool.

mod connect;
mod convert;
mod manager;
mod oauth;

pub use connect::SCRUBBED_ENV_VARS;
pub use manager::{McpEntry, McpManager, McpOptions, ShowLoginUrl};
