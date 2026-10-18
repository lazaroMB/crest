//! Opening a client session with one configured server.

use std::collections::{BTreeMap, HashMap};
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
            let (transport, stderr) = TokioChildProcess::builder(stdio_command(command, args, env))
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

/// The command of a stdio server: the inherited environment minus
/// [`SCRUBBED_ENV_VARS`], plus the spec's `env` (which may set them back).
fn stdio_command(command: &str, args: &[String], env: &BTreeMap<String, String>) -> Command {
    let mut cmd = Command::new(command);
    cmd.args(args).kill_on_drop(true);
    for key in SCRUBBED_ENV_VARS {
        cmd.env_remove(key);
    }
    cmd.envs(env);
    cmd
}

/// Forwards what a stdio server writes on stderr to the log.
async fn log_stderr(server: String, stderr: ChildStderr) {
    let mut lines = BufReader::new(stderr).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        tracing::debug!(target: "crest::mcp", server = %server, "{line}");
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use super::*;

    #[test]
    fn stdio_servers_lose_api_keys_but_get_their_env() {
        let env = BTreeMap::from([
            ("GITHUB_TOKEN".to_owned(), "placeholder".to_owned()),
            ("OPENAI_API_KEY".to_owned(), "explicit".to_owned()),
        ]);
        let cmd = stdio_command("server", &["--flag".to_owned()], &env);
        let std = cmd.as_std();
        assert_eq!(std.get_program(), "server");
        assert_eq!(std.get_args().collect::<Vec<_>>(), ["--flag"]);
        let envs: BTreeMap<_, _> = std.get_envs().collect();
        assert_eq!(envs[OsStr::new("ANTHROPIC_API_KEY")], None);
        assert_eq!(envs[OsStr::new("MISTRAL_API_KEY")], None);
        assert_eq!(
            envs[OsStr::new("GITHUB_TOKEN")],
            Some(OsStr::new("placeholder"))
        );
        assert_eq!(
            envs[OsStr::new("OPENAI_API_KEY")],
            Some(OsStr::new("explicit"))
        );
        assert!(
            !envs.contains_key(OsStr::new("PATH")),
            "the rest is inherited"
        );
    }
}
