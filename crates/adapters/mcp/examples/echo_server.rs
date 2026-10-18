//! A tiny MCP server used by the adapter's tests, and a ready example to
//! declare in `init.lua`.
//!
//! It offers two tools: `echo` (returns its text) and `add` (sums two
//! integers). It speaks MCP over stdin/stdout:
//!
//! ```text
//! cargo run -p adapter-mcp --example echo_server
//! ```

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{ServerCapabilities, ServerConfig};
use rmcp::{ServerHandler, ServiceExt, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

/// Input of `echo`.
#[derive(Debug, Deserialize, JsonSchema)]
struct EchoInput {
    /// Text to send back.
    text: String,
}

/// Input of `add`.
#[derive(Debug, Deserialize, JsonSchema)]
struct AddInput {
    /// First addend.
    a: i64,
    /// Second addend.
    b: i64,
}

#[derive(Debug, Clone)]
struct EchoServer {
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl EchoServer {
    fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }

    #[tool(description = "Returns the given text unchanged")]
    fn echo(&self, Parameters(EchoInput { text }): Parameters<EchoInput>) -> String {
        text
    }

    #[tool(description = "Adds two integers")]
    fn add(&self, Parameters(AddInput { a, b }): Parameters<AddInput>) -> String {
        a.saturating_add(b).to_string()
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for EchoServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions("Echoes text and adds numbers")
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let server = EchoServer::new().serve(rmcp::transport::stdio()).await?;
    server.waiting().await?;
    Ok(())
}
