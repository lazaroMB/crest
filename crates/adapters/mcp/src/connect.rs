//! Opening a client session with one configured server.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, PoisonError};

use application::ports::plugin::{McpServerSpec, McpTransport};
use reqwest::header::{HeaderName, HeaderValue};
use rmcp::handler::client::ClientHandler;
use rmcp::model::{ClientCapabilities, ClientConfig, Implementation, ProtocolVersion};
use rmcp::service::{ClientInitializeError, NotificationContext, RunningService};
use rmcp::transport::auth::AuthClient;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::{StreamableHttpClientTransport, TokioChildProcess};
use rmcp::{RoleClient, ServiceExt};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{ChildStderr, Command};
use tokio::sync::Notify;

use crate::oauth;

/// A connected client session.
pub(crate) type Client = RunningService<RoleClient, CrestClient>;

/// Environment variables removed from what a stdio server inherits: API
/// keys and tokens of LLM providers and the Brave search key. A server
/// that needs one gets it
/// explicitly through the `env` of its spec.
pub const SCRUBBED_ENV_VARS: [&str; 8] = [
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "OPENAI_API_KEY",
    "OPENCODE_API_KEY",
    "GEMINI_API_KEY",
    "GOOGLE_API_KEY",
    "MISTRAL_API_KEY",
    "BRAVE_API_KEY",
];

/// Lines of a stdio server's stderr kept to explain a failure.
const STDERR_LINES: usize = 20;

/// The last lines a stdio server wrote on stderr.
#[derive(Debug, Clone, Default)]
pub(crate) struct StderrTail(Arc<Mutex<VecDeque<String>>>);

impl StderrTail {
    fn push(&self, line: String) {
        let mut lines = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if lines.len() == STDERR_LINES {
            lines.pop_front();
        }
        lines.push_back(line);
    }

    /// `message` followed by the kept lines, if any.
    pub(crate) fn explain(&self, message: &str) -> String {
        let lines = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if lines.is_empty() {
            return message.to_owned();
        }
        let tail: Vec<&str> = lines.iter().map(String::as_str).collect();
        format!("{message}\nstderr:\n{}", tail.join("\n"))
    }

    pub(crate) fn clear(&self) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
    }
}

/// How crest introduces itself, and where `tools/list_changed` goes.
#[derive(Clone)]
pub(crate) struct CrestClient {
    tools_changed: Arc<Notify>,
}

impl ClientHandler for CrestClient {
    fn get_info(&self) -> ClientConfig {
        let mut info = ClientConfig::new(
            ClientCapabilities::default(),
            Implementation::new("crest", env!("CARGO_PKG_VERSION")),
        );
        // The widely deployed revision; servers answer with theirs.
        info.protocol_version = ProtocolVersion::V_2025_11_25;
        info
    }

    fn on_tool_list_changed(
        &self,
        _context: NotificationContext<RoleClient>,
    ) -> impl Future<Output = ()> + Send + '_ {
        self.tools_changed.notify_one();
        std::future::ready(())
    }
}

/// Why a server could not be reached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ConnectError {
    /// It cannot start or answer; the message says why.
    Failed(String),
    /// It asks for an OAuth login.
    NeedsLogin(String),
}

/// What a connection needs besides the spec.
#[derive(Clone)]
pub(crate) struct ConnectContext {
    /// Directory of stdio servers without a `cwd`.
    pub default_cwd: Option<PathBuf>,
    /// Where the stderr of a stdio server goes.
    pub stderr: StderrTail,
    /// Woken when the server says its tools changed.
    pub tools_changed: Arc<Notify>,
}

/// Starts or reaches the server of `spec` and runs the MCP handshake.
///
/// # Errors
///
/// [`ConnectError::NeedsLogin`] when an HTTP server asks for OAuth and
/// there are no stored tokens, otherwise a message for the user when the
/// process cannot start, a value cannot be expanded or the handshake
/// fails.
pub(crate) async fn connect(
    spec: &McpServerSpec,
    ctx: &ConnectContext,
) -> Result<Client, ConnectError> {
    let handler = CrestClient {
        tools_changed: Arc::clone(&ctx.tools_changed),
    };
    let failed = ConnectError::Failed;
    match &spec.transport {
        McpTransport::Stdio {
            command,
            args,
            env,
            cwd,
        } => {
            let args = args
                .iter()
                .map(|arg| expand(arg, "args"))
                .collect::<Result<Vec<_>, _>>()
                .map_err(failed)?;
            let env = env
                .iter()
                .map(|(k, v)| Ok((k.clone(), expand(v, &format!("env.{k}"))?)))
                .collect::<Result<BTreeMap<_, _>, String>>()
                .map_err(failed)?;
            let dir = cwd
                .as_deref()
                .map(PathBuf::from)
                .or_else(|| ctx.default_cwd.clone());
            let cmd = stdio_command(command, &args, &env, dir.as_deref());
            let (transport, stderr) = TokioChildProcess::builder(cmd)
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|err| ConnectError::Failed(spawn_error(command, &err)))?;
            ctx.stderr.clear();
            if let Some(stderr) = stderr {
                tokio::spawn(log_stderr(spec.name.clone(), stderr, ctx.stderr.clone()));
            }
            handler.serve(transport).await.map_err(|err| {
                ConnectError::Failed(ctx.stderr.explain(&format!("handshake failed: {err}")))
            })
        }
        McpTransport::Http {
            url,
            headers,
            oauth: client,
            ..
        } => {
            let url = expand(url, "url").map_err(failed)?;
            let mut custom = HashMap::new();
            for (name, value) in headers {
                let value = expand(value, &format!("headers.{name}")).map_err(failed)?;
                let header = HeaderName::from_bytes(name.as_bytes()).map_err(|err| {
                    ConnectError::Failed(format!("invalid header name `{name}`: {err}"))
                })?;
                let value = HeaderValue::from_str(&value).map_err(|err| {
                    ConnectError::Failed(format!("invalid value for header `{name}`: {err}"))
                })?;
                custom.insert(header, value);
            }
            let mut config =
                StreamableHttpClientTransportConfig::with_uri(url.as_str()).custom_headers(custom);
            config.reinit_on_expired_session = true;
            // No request timeout on this client: it would cut SSE streams.
            let http = reqwest::Client::new();
            let outcome = match oauth::stored_manager(&spec.name, &url, client.as_ref()).await {
                Some(manager) => {
                    let transport = StreamableHttpClientTransport::with_client(
                        AuthClient::new(http, manager),
                        config,
                    );
                    handler.serve(transport).await
                }
                None => {
                    let transport = StreamableHttpClientTransport::with_client(http, config);
                    handler.serve(transport).await
                }
            };
            outcome.map_err(|err| http_error(&spec.name, &url, &err))
        }
    }
}

/// The error of a failed HTTP handshake: a login when the server asks for
/// one, else what went wrong with a hint.
fn http_error(name: &str, url: &str, err: &ClientInitializeError) -> ConnectError {
    if err.auth_challenge().is_some() || mentions(err, &["401", "Unauthorized", "AuthRequired"]) {
        return ConnectError::NeedsLogin(format!(
            "{url} asks for a login: run /mcp login {name} (or set an Authorization header)"
        ));
    }
    let hint = if mentions(err, &["403", "Forbidden"]) {
        "; the server refused the credentials (403)"
    } else if mentions(err, &["404", "Not Found"]) {
        "; is this the server's MCP endpoint (often …/mcp)?"
    } else if mentions(err, &["405", "Method Not Allowed"]) {
        "; the server may only speak the old HTTP+SSE transport (…/sse), which crest does not"
    } else {
        ""
    };
    ConnectError::Failed(format!("handshake with {url} failed: {err}{hint}"))
}

fn mentions(err: &ClientInitializeError, needles: &[&str]) -> bool {
    let text = format!("{err} {err:?}");
    needles.iter().any(|needle| text.contains(needle))
}

/// The message of a server process that cannot start.
fn spawn_error(command: &str, err: &std::io::Error) -> String {
    if err.kind() == std::io::ErrorKind::NotFound && command.contains(std::path::MAIN_SEPARATOR) {
        return format!("cannot start `{command}`: it does not exist");
    }
    if err.kind() == std::io::ErrorKind::NotFound {
        return format!(
            "`{command}` was not found on PATH; if crest was opened from the desktop, use \
             the absolute path or add its directory to PATH in ~/.profile or ~/.zprofile"
        );
    }
    format!("cannot start `{command}`: {err}")
}

/// `value` with every `${VAR}` and `${VAR:-default}` replaced from the
/// environment; `field` names it in the error.
///
/// # Errors
///
/// When a variable without default is not set, or a `${` is not closed.
pub(crate) fn expand(value: &str, field: &str) -> Result<String, String> {
    expand_with(value, field, |name| std::env::var(name).ok())
}

fn expand_with(
    value: &str,
    field: &str,
    lookup: impl Fn(&str) -> Option<String>,
) -> Result<String, String> {
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let end = after
            .find('}')
            .ok_or_else(|| format!("{field}: `${{` without a closing `}}`"))?;
        let expr = &after[..end];
        let (name, default) = match expr.split_once(":-") {
            Some((name, default)) => (name, Some(default)),
            None => (expr, None),
        };
        match lookup(name).filter(|v| !v.is_empty()) {
            Some(found) => out.push_str(&found),
            None => match default {
                Some(default) => out.push_str(default),
                None => {
                    return Err(format!(
                        "{field} uses ${{{name}}}, which is not set in crest's environment"
                    ));
                }
            },
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// The command of a stdio server: the inherited environment minus
/// [`SCRUBBED_ENV_VARS`], plus the spec's `env` (which may set them back),
/// run in `cwd` when given.
fn stdio_command(
    command: &str,
    args: &[String],
    env: &BTreeMap<String, String>,
    cwd: Option<&Path>,
) -> Command {
    let mut cmd = Command::new(command);
    cmd.args(args).kill_on_drop(true);
    for key in SCRUBBED_ENV_VARS {
        cmd.env_remove(key);
    }
    cmd.envs(env);
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }
    cmd
}

/// Keeps the last lines a stdio server writes on stderr and forwards them
/// to the log.
async fn log_stderr(server: String, stderr: ChildStderr, tail: StderrTail) {
    let mut lines = BufReader::new(stderr).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        tracing::debug!(target: "crest::mcp", server = %server, "{line}");
        tail.push(line);
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use super::*;

    #[test]
    fn stdio_servers_lose_api_keys_but_get_their_env_and_dir() {
        let env = BTreeMap::from([
            ("GITHUB_TOKEN".to_owned(), "placeholder".to_owned()),
            ("OPENAI_API_KEY".to_owned(), "explicit".to_owned()),
        ]);
        let cmd = stdio_command(
            "server",
            &["--flag".to_owned()],
            &env,
            Some(Path::new("/ws")),
        );
        let std = cmd.as_std();
        assert_eq!(std.get_program(), "server");
        assert_eq!(std.get_args().collect::<Vec<_>>(), ["--flag"]);
        assert_eq!(std.get_current_dir(), Some(Path::new("/ws")));
        let envs: BTreeMap<_, _> = std.get_envs().collect();
        assert_eq!(envs[OsStr::new("ANTHROPIC_API_KEY")], None);
        assert_eq!(envs[OsStr::new("OPENCODE_API_KEY")], None);
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

    #[test]
    fn variables_are_expanded_from_the_environment() {
        let lookup = |name: &str| (name == "TOKEN").then(|| "abc".to_owned());
        assert_eq!(
            expand_with("Bearer ${TOKEN}", "headers.Authorization", lookup).unwrap(),
            "Bearer abc"
        );
        assert_eq!(
            expand_with("${MISSING:-x}/${TOKEN}", "url", lookup).unwrap(),
            "x/abc"
        );
        assert_eq!(
            expand_with("plain $HOME", "args", lookup).unwrap(),
            "plain $HOME"
        );
        let err =
            expand_with("Bearer ${GITHUB_TOKEN}", "headers.Authorization", lookup).unwrap_err();
        assert!(
            err.contains("headers.Authorization uses ${GITHUB_TOKEN}"),
            "{err}"
        );
        assert!(expand_with("${OPEN", "url", lookup).is_err());
    }

    #[test]
    fn stderr_tail_explains_failures() {
        let tail = StderrTail::default();
        assert_eq!(tail.explain("boom"), "boom");
        for n in 0..30 {
            tail.push(format!("line {n}"));
        }
        let text = tail.explain("handshake failed");
        assert!(
            text.starts_with("handshake failed\nstderr:\nline 10"),
            "{text}"
        );
        assert!(text.ends_with("line 29"), "{text}");
    }

    #[test]
    fn a_missing_command_says_how_to_fix_the_path() {
        let err = std::io::Error::from(std::io::ErrorKind::NotFound);
        let message = spawn_error("npx", &err);
        assert!(message.contains("`npx` was not found on PATH"), "{message}");
    }
}
