//! A tiny MCP server used by the adapter's tests, and a ready example to
//! declare in `init.lua`.
//!
//! It offers two tools: `echo` (returns its text) and `add` (sums two
//! integers). It speaks MCP over stdin/stdout, or over streamable HTTP
//! with `--http <addr>` (it then prints its URL as the first line of
//! stdout; port `0` picks a free one):
//!
//! ```text
//! cargo run -p adapter-mcp --example echo_server
//! cargo run -p adapter-mcp --example echo_server -- --http 127.0.0.1:0
//! ```

use std::convert::Infallible;
use std::io::Write;

use hyper_util::rt::TokioIo;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{ServerCapabilities, ServerConfig};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};
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

/// Serves MCP over streamable HTTP on `addr` until killed.
async fn serve_http(addr: &str) -> Result<(), Box<dyn std::error::Error>> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let mut stdout = std::io::stdout();
    writeln!(stdout, "http://{}/mcp", listener.local_addr()?)?;
    stdout.flush()?;
    let service: StreamableHttpService<EchoServer, LocalSessionManager> =
        StreamableHttpService::new(
            || Ok(EchoServer::new()),
            Default::default(),
            StreamableHttpServerConfig::default(),
        );
    loop {
        let (stream, _) = listener.accept().await?;
        let service = service.clone();
        tokio::spawn(async move {
            let handler = hyper::service::service_fn(move |request| {
                let service = service.clone();
                async move { Ok::<_, Infallible>(service.handle(request).await) }
            });
            let _ = hyper::server::conn::http1::Builder::new()
                .serve_connection(TokioIo::new(stream), handler)
                .await;
        });
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let [flag, addr] = args.as_slice()
        && flag == "--http"
    {
        return serve_http(addr).await;
    }
    let server = EchoServer::new().serve(rmcp::transport::stdio()).await?;
    server.waiting().await?;
    Ok(())
}
