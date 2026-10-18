//! [`McpManager`]: the configured servers, their connections and the
//! [`McpPort`] over them.

use std::collections::HashSet;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use application::ports::mcp::{
    McpError, McpPort, McpServerState, McpServerStatus, McpTool, McpToolResult,
};
use application::ports::plugin::McpServerSpec;
use async_trait::async_trait;
use rmcp::model::{
    CallToolRequest, CallToolRequestParams, CallToolResult, CancelledNotificationParam,
    ClientRequest, RequestId, ServerResult,
};
use rmcp::service::PeerRequestOptions;
use rmcp::{Peer, RoleClient, ServiceError};
use tokio::sync::watch;
use tokio::task::AbortHandle;

use crate::connect::{Client, connect};
use crate::convert;

/// Time limits of an [`McpManager`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct McpOptions {
    /// Limit for starting a server, the handshake and listing its tools.
    pub connect_timeout: Duration,
    /// Limit for one tool call.
    pub call_timeout: Duration,
}

impl Default for McpOptions {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(30),
            call_timeout: Duration::from_secs(120),
        }
    }
}

/// A live session and the tools the server listed when it connected.
struct Connection {
    client: Client,
    tools: Vec<McpTool>,
}

#[derive(Clone)]
enum State {
    Starting,
    Ready(Arc<Connection>),
    Failed(String),
    Stopped,
}

impl State {
    fn status(&self) -> McpServerState {
        match self {
            Self::Starting => McpServerState::Starting,
            Self::Ready(conn) => McpServerState::Ready {
                tool_count: conn.tools.len(),
            },
            Self::Failed(message) => McpServerState::Failed {
                message: message.clone(),
            },
            Self::Stopped => McpServerState::Stopped,
        }
    }
}

struct Server {
    name: String,
    state: watch::Sender<State>,
}

/// The MCP servers declared in the config, connected in the background.
///
/// [`start`](Self::start) launches every server at once; each one moves
/// from `Starting` to `Ready` or `Failed` on its own, and a failing server
/// never affects the others. Tools are listed once, when the server
/// connects: `tools/list_changed` notifications are not followed, and a
/// server whose connection drops stays `Failed` until crest restarts (no
/// reconnection).
///
/// Dropping the manager closes every session; stdio servers are killed.
pub struct McpManager {
    servers: Vec<Server>,
    options: McpOptions,
    tasks: Vec<AbortHandle>,
}

impl fmt::Debug for McpManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("McpManager")
            .field(
                "servers",
                &self.servers.iter().map(|s| &s.name).collect::<Vec<_>>(),
            )
            .field("options", &self.options)
            .finish_non_exhaustive()
    }
}

/// Why `name` cannot be used to qualify tool names, if it cannot.
fn invalid_name(name: &str) -> Option<String> {
    let valid_chars = name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    (name.is_empty() || !valid_chars || name.contains("__"))
        .then(|| format!("invalid server name `{name}` (use letters, digits, `-` and single `_`)"))
}

impl McpManager {
    /// Starts connecting to every server in `specs` with the default
    /// [`McpOptions`]. Returns at once; see [`wait_ready`](Self::wait_ready).
    ///
    /// # Panics
    ///
    /// Outside a Tokio runtime.
    #[must_use]
    pub fn start(specs: Vec<McpServerSpec>) -> Self {
        Self::start_with(specs, McpOptions::default())
    }

    /// As [`start`](Self::start), with explicit time limits.
    ///
    /// # Panics
    ///
    /// Outside a Tokio runtime.
    #[must_use]
    pub fn start_with(specs: Vec<McpServerSpec>, options: McpOptions) -> Self {
        let mut seen = HashSet::new();
        let mut servers = Vec::with_capacity(specs.len());
        let mut tasks = Vec::new();
        for spec in specs {
            let problem = invalid_name(&spec.name).or_else(|| {
                (!seen.insert(spec.name.clone()))
                    .then(|| format!("duplicate MCP server `{}`", spec.name))
            });
            let initial = problem.map_or(State::Starting, State::Failed);
            let starting = matches!(initial, State::Starting);
            let (state, _) = watch::channel(initial);
            if starting {
                let task = tokio::spawn(run_connect(spec.clone(), state.clone(), options));
                tasks.push(task.abort_handle());
            }
            servers.push(Server {
                name: spec.name,
                state,
            });
        }
        Self {
            servers,
            options,
            tasks,
        }
    }

    /// Waits until no server is `Starting` (each one is bounded by the
    /// connect timeout).
    pub async fn wait_ready(&self) {
        for server in &self.servers {
            let mut rx = server.state.subscribe();
            // The sender lives in `self`, so this cannot fail.
            let _ = rx.wait_for(|s| !matches!(s, State::Starting)).await;
        }
    }

    /// Closes every session and marks the servers `Stopped`. Stdio servers
    /// get their stdin closed and are killed if they do not exit within a
    /// few seconds.
    pub async fn shutdown(&self) {
        for task in &self.tasks {
            task.abort();
        }
        for server in &self.servers {
            let old = server.state.send_replace(State::Stopped);
            // With a call still in flight `try_unwrap` fails; the session
            // then closes when that call ends and drops the last reference.
            if let State::Ready(conn) = old
                && let Ok(mut conn) = Arc::try_unwrap(conn)
            {
                let _ = conn.client.close_with_timeout(Duration::from_secs(5)).await;
            }
        }
    }

    fn server(&self, name: &str) -> Result<&Server, McpError> {
        self.servers
            .iter()
            .find(|s| s.name == name)
            .ok_or_else(|| McpError::UnknownServer(name.to_owned()))
    }
}

impl Drop for McpManager {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

/// Connects `spec` and publishes the outcome in `state`.
async fn run_connect(spec: McpServerSpec, state: watch::Sender<State>, options: McpOptions) {
    let attempt = async {
        let client = connect(&spec).await?;
        let has_tools = client
            .peer_info()
            .is_none_or(|info| info.capabilities.tools.is_some());
        let tools = if has_tools {
            client
                .peer()
                .list_all_tools()
                .await
                .map_err(|err| format!("cannot list tools: {err}"))?
        } else {
            Vec::new()
        };
        let tools = tools
            .into_iter()
            .map(|tool| convert::tool(&spec.name, tool))
            .collect();
        Ok::<_, String>(Connection { client, tools })
    };
    let next = match tokio::time::timeout(options.connect_timeout, attempt).await {
        Ok(Ok(conn)) => {
            tracing::info!(target: "crest::mcp", server = %spec.name, tools = conn.tools.len(), "MCP server ready");
            State::Ready(Arc::new(conn))
        }
        Ok(Err(message)) => State::Failed(message),
        Err(_) => State::Failed(format!(
            "did not start within {}s",
            options.connect_timeout.as_secs()
        )),
    };
    if let State::Failed(message) = &next {
        tracing::warn!(target: "crest::mcp", server = %spec.name, "MCP server failed: {message}");
    }
    state.send_if_modified(|current| {
        // Do not resurrect a server stopped while it was connecting.
        let starting = matches!(current, State::Starting);
        if starting {
            *current = next;
        }
        starting
    });
}

/// Sends `cancelled` for a request whose caller went away before the
/// answer, so the server can stop working on it.
struct CancelOnDrop {
    peer: Peer<RoleClient>,
    id: Option<RequestId>,
}

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if let Some(id) = self.id.take() {
            let peer = self.peer.clone();
            let params =
                CancelledNotificationParam::new(Some(id), Some("cancelled by user".into()));
            tokio::spawn(async move {
                let _ = peer.notify_cancelled(params).await;
            });
        }
    }
}

/// `tools/call` limited to `timeout` (rmcp then cancels it on the server)
/// and cancelled on the server too when the returned future is dropped.
async fn call_tool(
    peer: &Peer<RoleClient>,
    params: CallToolRequestParams,
    timeout: Duration,
) -> Result<CallToolResult, ServiceError> {
    let request = ClientRequest::CallToolRequest(CallToolRequest::new(params));
    let handle = peer
        .send_cancellable_request(request, PeerRequestOptions::with_timeout(timeout))
        .await?;
    let mut guard = CancelOnDrop {
        peer: peer.clone(),
        id: Some(handle.id.clone()),
    };
    let response = handle.await_response().await;
    guard.id = None;
    match response? {
        ServerResult::CallToolResult(result) => Ok(result),
        _ => Err(ServiceError::UnexpectedResponse),
    }
}

#[async_trait]
impl McpPort for McpManager {
    async fn list_tools(&self) -> Result<Vec<McpTool>, McpError> {
        Ok(self
            .servers
            .iter()
            .filter_map(|server| match &*server.state.borrow() {
                State::Ready(conn) => Some(conn.tools.clone()),
                _ => None,
            })
            .flatten()
            .collect())
    }

    async fn call_tool(
        &self,
        server: &str,
        name: &str,
        input: serde_json::Value,
    ) -> Result<McpToolResult, McpError> {
        let entry = self.server(server)?;
        let state = entry.state.borrow().clone();
        let conn = match state {
            State::Ready(conn) => conn,
            other => {
                let message = match other {
                    State::Starting => "still starting".to_owned(),
                    State::Failed(message) => message,
                    _ => "stopped".to_owned(),
                };
                return Err(McpError::Unavailable {
                    server: server.to_owned(),
                    message,
                });
            }
        };
        if !conn.tools.iter().any(|tool| tool.name == name) {
            return Err(McpError::UnknownTool {
                server: server.to_owned(),
                name: name.to_owned(),
            });
        }
        let mut params = CallToolRequestParams::new(name.to_owned());
        match input {
            serde_json::Value::Object(arguments) => params = params.with_arguments(arguments),
            serde_json::Value::Null => {}
            _ => {
                return Err(McpError::InvalidInput {
                    name: name.to_owned(),
                    message: "the input must be a JSON object".into(),
                });
            }
        }
        match call_tool(conn.client.peer(), params, self.options.call_timeout).await {
            Ok(result) => Ok(convert::result(result)),
            Err(err) => {
                if convert::is_disconnect(&err) {
                    entry.state.send_if_modified(|current| {
                        let same = matches!(current, State::Ready(c) if Arc::ptr_eq(c, &conn));
                        if same {
                            *current = State::Failed("connection closed".into());
                        }
                        same
                    });
                }
                Err(convert::error(server, err))
            }
        }
    }

    async fn servers(&self) -> Vec<McpServerStatus> {
        self.servers
            .iter()
            .map(|server| McpServerStatus {
                name: server.name.clone(),
                state: server.state.borrow().status(),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_names_must_qualify_tool_names() {
        assert_eq!(invalid_name("github"), None);
        assert_eq!(invalid_name("my-server_2"), None);
        assert!(invalid_name("").is_some());
        assert!(invalid_name("a__b").is_some());
        assert!(invalid_name("con espacio").is_some());
    }
}
