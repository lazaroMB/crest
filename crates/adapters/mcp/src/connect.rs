//! Opening a client session with one configured server.

use std::collections::HashMap;
use std::process::Stdio;

use application::ports::plugin::{McpServerSpec, McpTransport};
use reqwest::header::{HeaderName, HeaderValue};
use rmcp::model::{ClientCapabilities, ClientConfig, Implementation};
use rmcp::service::RunningService;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::{StreamableHttpClientTransport, TokioChildProcess};
use rmcp::{RoleClient, ServiceExt};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{ChildStderr, Command};

/// A connected client session.
pub(crate) type Client = RunningService<RoleClient, ClientConfig>;

/// Environment variables removed from what a stdio server inherits: API
/// keys and tokens of LLM providers. A server that needs one gets it
/// explicitly through the `env` of its spec.
pub const SCRUBBED_ENV_VARS: [&str; 6] = [
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "OPENAI_API_KEY",
    "GEMINI_API_KEY",
    "GOOGLE_API_KEY",
    "MISTRAL_API_KEY",
];

/// How crest introduces itself to servers.
fn client_config() -> ClientConfig {
    ClientConfig::new(
        ClientCapabilities::default(),
        Implementation::new("crest", env!("CARGO_PKG_VERSION")),
    )
}

/// Starts or reaches the server of `spec` and runs the MCP handshake.
///
/// # Errors
///
/// A message for the user when the process cannot start, a header is
/// invalid or the handshake fails.
pub(crate) async fn connect(spec: &McpServerSpec) -> Result<Client, String> {
    match &spec.transport {
        McpTransport::Stdio { command, args, env } => {
            let mut cmd = Command::new(command);
            cmd.args(args).kill_on_drop(true);
            for key in SCRUBBED_ENV_VARS {
                cmd.env_remove(key);
            }
            cmd.envs(env);
            let (transport, stderr) = TokioChildProcess::builder(cmd)
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|err| format!("cannot start `{command}`: {err}"))?;
            if let Some(stderr) = stderr {
                tokio::spawn(log_stderr(spec.name.clone(), stderr));
            }
            client_config()
                .serve(transport)
                .await
                .map_err(|err| format!("handshake failed: {err}"))
        }
        McpTransport::Http { url, headers } => {
            let mut custom = HashMap::new();
            for (name, value) in headers {
                let name = HeaderName::from_bytes(name.as_bytes())
                    .map_err(|err| format!("invalid header name `{name}`: {err}"))?;
                let value = HeaderValue::from_str(value)
                    .map_err(|err| format!("invalid value for header `{name}`: {err}"))?;
                custom.insert(name, value);
            }
            let config =
                StreamableHttpClientTransportConfig::with_uri(url.as_str()).custom_headers(custom);
            let transport = StreamableHttpClientTransport::<reqwest::Client>::from_config(config);
            client_config()
                .serve(transport)
                .await
                .map_err(|err| format!("handshake with {url} failed: {err}"))
        }
    }
}

/// Forwards what a stdio server writes on stderr to the log.
async fn log_stderr(server: String, stderr: ChildStderr) {
    let mut lines = BufReader::new(stderr).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        tracing::debug!(target: "crest::mcp", server = %server, "{line}");
    }
}
