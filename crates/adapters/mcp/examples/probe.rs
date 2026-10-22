//! Connects to one MCP server the way crest does and prints its state and
//! tools: a quick check of a server before declaring it in `init.lua`.
//!
//! ```text
//! cargo run -p adapter-mcp --example probe -- https://mcp.deepwiki.com/mcp
//! cargo run -p adapter-mcp --example probe -- npx -y @modelcontextprotocol/server-filesystem .
//! ```

use std::collections::BTreeMap;

use adapter_mcp::McpManager;
use application::ports::mcp::McpPort;
use application::ports::plugin::{McpServerSpec, McpTransport};

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(first) = args.first() else {
        eprintln!("usage: probe <url> | probe <command> [args...]");
        std::process::exit(2);
    };
    let transport = if first.starts_with("http://") || first.starts_with("https://") {
        McpTransport::Http {
            url: first.clone(),
            headers: BTreeMap::new(),
            oauth: None,
        }
    } else {
        McpTransport::Stdio {
            command: first.clone(),
            args: args[1..].to_vec(),
            env: BTreeMap::new(),
            cwd: None,
        }
    };
    let manager = McpManager::start(vec![McpServerSpec::new("probe", transport)]);
    manager.wait_ready().await;
    for status in manager.servers().await {
        println!("{}: {}", status.transport, status.state.label());
        if let Some(message) = status.state.message() {
            println!("  {message}");
        }
    }
    for tool in manager.list_tools().await.unwrap_or_default() {
        let summary = tool.description.lines().next().unwrap_or_default();
        println!("  {} — {summary}", tool.qualified_name());
    }
    manager.shutdown().await;
}
