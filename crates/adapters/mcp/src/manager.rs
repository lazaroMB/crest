//! [`McpManager`]: the configured servers, their connections and the
//! [`McpPort`] over them.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::Duration;

use application::ports::mcp::{
    McpError, McpPort, McpServerState, McpServerStatus, McpTool, McpToolResult,
};
use application::ports::plugin::{McpServerSpec, McpTransport};
use async_trait::async_trait;
use rmcp::model::{
    CallToolRequest, CallToolRequestParams, CallToolResult, CancelledNotificationParam,
    ClientRequest, RequestId, ServerResult,
};
use rmcp::service::PeerRequestOptions;
use rmcp::{Peer, RoleClient, ServiceError};
use tokio::sync::{Notify, watch};
use tokio::task::AbortHandle;

use crate::connect::{Client, ConnectContext, ConnectError, StderrTail, connect};
use crate::convert;
use crate::oauth;

/// Shows the page of an OAuth login: `(server, url)`.
pub type ShowLoginUrl = Arc<dyn Fn(&str, &str) + Send + Sync>;

/// Limits and settings of an [`McpManager`].
#[derive(Clone)]
pub struct McpOptions {
    /// Limit for starting a server, the handshake and listing its tools,
    /// unless its spec says otherwise. Long enough for the first `npx -y`,
    /// which downloads the package.
    pub connect_timeout: Duration,
    /// Limit for one tool call.
    pub call_timeout: Duration,
    /// Directory of stdio servers without `cwd` (the workspace).
    pub default_cwd: Option<PathBuf>,
    /// Shows the page to open for an OAuth login; by default it is only
    /// logged.
    pub show_login_url: ShowLoginUrl,
}

impl Default for McpOptions {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(120),
            call_timeout: Duration::from_secs(120),
            default_cwd: None,
            show_login_url: Arc::new(|server, url| {
                tracing::warn!(target: "crest::mcp", server, "open {url} to log in");
            }),
        }
    }
}

impl fmt::Debug for McpOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("McpOptions")
            .field("connect_timeout", &self.connect_timeout)
            .field("call_timeout", &self.call_timeout)
            .field("default_cwd", &self.default_cwd)
            .finish_non_exhaustive()
    }
}

/// The tools of a connection, by the name offered to the model, with the
/// name the server knows each one by.
#[derive(Default)]
struct ToolSet {
    tools: Vec<McpTool>,
    originals: HashMap<String, String>,
}

impl ToolSet {
    fn from_rmcp(server: &str, listed: Vec<rmcp::model::Tool>) -> Self {
        let mut set = Self::default();
        let mut taken = Vec::new();
        for tool in listed {
            let (tool, original) = convert::tool(server, tool, &taken);
            taken.push(tool.name.clone());
            set.originals.insert(tool.name.clone(), original);
            set.tools.push(tool);
        }
        set
    }
}

/// A live session and the tools the server listed.
struct Connection {
    client: Client,
    tools: RwLock<ToolSet>,
}

impl Connection {
    fn tools(&self) -> Vec<McpTool> {
        self.tools
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .tools
            .clone()
    }

    fn original(&self, name: &str) -> Option<String> {
        self.tools
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .originals
            .get(name)
            .cloned()
    }
}

#[derive(Clone)]
enum State {
    Starting,
    Ready(Arc<Connection>),
    Failed(String),
    NeedsLogin(String),
    Disabled,
    Untrusted,
    Stopped,
}

impl State {
    fn status(&self) -> McpServerState {
        match self {
            Self::Starting => McpServerState::Starting,
            Self::Ready(conn) => McpServerState::Ready {
                tool_count: conn.tools().len(),
            },
            Self::Failed(message) => McpServerState::Failed {
                message: message.clone(),
            },
            Self::NeedsLogin(message) => McpServerState::NeedsLogin {
                message: message.clone(),
            },
            Self::Disabled => McpServerState::Disabled,
            Self::Untrusted => McpServerState::Untrusted,
            Self::Stopped => McpServerState::Stopped,
        }
    }
}

/// One configured server.
struct Server {
    spec: McpServerSpec,
    state: watch::Sender<State>,
    task: Mutex<Option<AbortHandle>>,
    stderr: StderrTail,
    tools_changed: Arc<Notify>,
    /// Whether the one automatic reconnection was used.
    reconnected: AtomicBool,
}

impl Server {
    fn abort(&self) {
        if let Some(task) = self
            .task
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            task.abort();
        }
    }
}

/// A server and whether it may start (workspace servers wait for trust).
#[derive(Debug, Clone)]
pub struct McpEntry {
    /// Its spec.
    pub spec: McpServerSpec,
    /// `false` keeps it `Untrusted` until [`McpManager::trust`].
    pub trusted: bool,
}

/// The MCP servers declared in the config, connected in the background.
///
/// [`start`](Self::start) launches every server at once; each one moves
/// from `Starting` to `Ready`, `Failed` or `NeedsLogin` on its own, and a
/// failing server never affects the others. Tools are listed when the
/// server connects and again when it sends `tools/list_changed`; a server
/// whose connection drops is reconnected once, then stays `Failed` until
/// [`restart`](McpPort::restart). Every change bumps the counter of
/// [`subscribe`](Self::subscribe).
///
/// Dropping the manager closes every session; stdio servers are killed.
pub struct McpManager {
    servers: Vec<Arc<Server>>,
    options: McpOptions,
    changes: Arc<watch::Sender<u64>>,
}

impl fmt::Debug for McpManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("McpManager")
            .field(
                "servers",
                &self
                    .servers
                    .iter()
                    .map(|s| &s.spec.name)
                    .collect::<Vec<_>>(),
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

/// How `spec` is reached, for lists.
fn describe(spec: &McpServerSpec) -> String {
    match &spec.transport {
        McpTransport::Stdio { command, args, .. } => {
            let mut line = format!("stdio: {command}");
            for arg in args {
                line.push(' ');
                line.push_str(arg);
            }
            line
        }
        McpTransport::Http { url, .. } => format!("http: {url}"),
    }
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

    /// As [`start`](Self::start), with explicit options.
    ///
    /// # Panics
    ///
    /// Outside a Tokio runtime.
    #[must_use]
    pub fn start_with(specs: Vec<McpServerSpec>, options: McpOptions) -> Self {
        let entries = specs
            .into_iter()
            .map(|spec| McpEntry {
                spec,
                trusted: true,
            })
            .collect();
        Self::start_entries(entries, options)
    }

    /// Starts every trusted, enabled server of `entries`; the others wait
    /// as `Untrusted` or `Disabled`.
    ///
    /// # Panics
    ///
    /// Outside a Tokio runtime.
    #[must_use]
    pub fn start_entries(entries: Vec<McpEntry>, options: McpOptions) -> Self {
        let (changes, _) = watch::channel(0);
        let changes = Arc::new(changes);
        let mut seen = HashSet::new();
        let mut servers = Vec::with_capacity(entries.len());
        for McpEntry { spec, trusted } in entries {
            let problem = invalid_name(&spec.name).or_else(|| {
                (!seen.insert(spec.name.clone()))
                    .then(|| format!("duplicate MCP server `{}`", spec.name))
            });
            let initial = match problem {
                Some(problem) => State::Failed(problem),
                None if !trusted => State::Untrusted,
                None if spec.disabled => State::Disabled,
                None => State::Starting,
            };
            let starting = matches!(initial, State::Starting);
            let (state, _) = watch::channel(initial);
            let server = Arc::new(Server {
                spec,
                state,
                task: Mutex::new(None),
                stderr: StderrTail::default(),
                tools_changed: Arc::new(Notify::new()),
                reconnected: AtomicBool::new(false),
            });
            if starting {
                spawn(&server, &options, &changes);
            }
            servers.push(server);
        }
        Self {
            servers,
            options,
            changes,
        }
    }

    /// A receiver whose value changes every time a server changes state or
    /// tools.
    #[must_use]
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.changes.subscribe()
    }

    fn spawn(&self, server: &Arc<Server>) {
        spawn(server, &self.options, &self.changes);
    }

    /// Waits until no server is `Starting` (each one is bounded by its
    /// connect timeout).
    pub async fn wait_ready(&self) {
        for server in &self.servers {
            let mut rx = server.state.subscribe();
            // The sender lives in `self`, so this cannot fail.
            let _ = rx.wait_for(|s| !matches!(s, State::Starting)).await;
        }
    }

    /// Starts the `Untrusted` server `name` once the user trusted it.
    ///
    /// # Errors
    ///
    /// [`McpError::UnknownServer`] for an unknown name.
    pub fn trust(&self, name: &str) -> Result<(), McpError> {
        let server = self.server(name)?;
        if matches!(*server.state.borrow(), State::Untrusted) {
            self.spawn(server);
        }
        Ok(())
    }

    /// Closes every session and marks the servers `Stopped`. Stdio servers
    /// get their stdin closed and are killed if they do not exit within a
    /// few seconds.
    pub async fn shutdown(&self) {
        for server in &self.servers {
            server.abort();
            let old = server.state.send_replace(State::Stopped);
            close(old).await;
        }
        bump(&self.changes);
    }

    fn server(&self, name: &str) -> Result<&Arc<Server>, McpError> {
        self.servers
            .iter()
            .find(|s| s.spec.name == name)
            .ok_or_else(|| McpError::UnknownServer(name.to_owned()))
    }
}

impl Drop for McpManager {
    fn drop(&mut self) {
        for server in &self.servers {
            server.abort();
        }
    }
}

/// (Re)starts the connection task of `server`.
fn spawn(server: &Arc<Server>, options: &McpOptions, changes: &Arc<watch::Sender<u64>>) {
    server.abort();
    server.state.send_replace(State::Starting);
    bump(changes);
    let task = tokio::spawn(run(
        Arc::clone(server),
        options.clone(),
        Arc::clone(changes),
    ));
    *server.task.lock().unwrap_or_else(PoisonError::into_inner) = Some(task.abort_handle());
}

fn bump(changes: &watch::Sender<u64>) {
    changes.send_modify(|n| *n = n.wrapping_add(1));
}

/// Closes the session of a `Ready` state when nothing else holds it; with
/// a call still in flight it closes when that call ends.
async fn close(state: State) {
    if let State::Ready(conn) = state
        && let Ok(mut conn) = Arc::try_unwrap(conn)
    {
        let _ = conn.client.close_with_timeout(Duration::from_secs(5)).await;
    }
}

/// Connects `server`, publishes the outcome and, once ready, reloads its
/// tools each time it says they changed.
async fn run(server: Arc<Server>, options: McpOptions, changes: Arc<watch::Sender<u64>>) {
    let spec = &server.spec;
    let ctx = ConnectContext {
        default_cwd: options.default_cwd.clone(),
        stderr: server.stderr.clone(),
        tools_changed: Arc::clone(&server.tools_changed),
    };
    let limit = spec
        .startup_timeout_ms
        .map_or(options.connect_timeout, Duration::from_millis);
    let attempt = async {
        let client = connect(spec, &ctx).await?;
        let tools = list_tools(&client, &spec.name)
            .await
            .map_err(ConnectError::Failed)?;
        Ok::<_, ConnectError>(Arc::new(Connection {
            client,
            tools: RwLock::new(tools),
        }))
    };
    let next = match tokio::time::timeout(limit, attempt).await {
        Ok(Ok(conn)) => {
            tracing::info!(target: "crest::mcp", server = %spec.name, tools = conn.tools().len(), "MCP server ready");
            State::Ready(conn)
        }
        Ok(Err(ConnectError::Failed(message))) => State::Failed(message),
        Ok(Err(ConnectError::NeedsLogin(message))) => State::NeedsLogin(message),
        Err(_) => State::Failed(server.stderr.explain(&format!(
            "did not start within {}s (set `startup_timeout` for slow servers)",
            limit.as_secs()
        ))),
    };
    if let State::Failed(message) = &next {
        tracing::warn!(target: "crest::mcp", server = %spec.name, "MCP server failed: {message}");
    }
    let conn = match &next {
        State::Ready(conn) => Some(Arc::clone(conn)),
        _ => None,
    };
    let published = server.state.send_if_modified(|current| {
        // Do not resurrect a server stopped or disabled while it connected.
        let starting = matches!(current, State::Starting);
        if starting {
            *current = next;
        }
        starting
    });
    bump(&changes);
    let (Some(conn), true) = (conn, published) else {
        return;
    };
    loop {
        server.tools_changed.notified().await;
        match list_tools(&conn.client, &spec.name).await {
            Ok(tools) => {
                *conn.tools.write().unwrap_or_else(PoisonError::into_inner) = tools;
                tracing::info!(target: "crest::mcp", server = %spec.name, "MCP tools reloaded");
                bump(&changes);
            }
            Err(message) => {
                tracing::warn!(target: "crest::mcp", server = %spec.name, "{message}");
            }
        }
    }
}

/// Every tool of `client`, when the server advertises tools.
async fn list_tools(client: &Client, server: &str) -> Result<ToolSet, String> {
    let has_tools = client
        .peer_info()
        .is_none_or(|info| info.capabilities.tools.is_some());
    if !has_tools {
        return Ok(ToolSet::default());
    }
    let listed = client
        .peer()
        .list_all_tools()
        .await
        .map_err(|err| format!("cannot list tools: {err}"))?;
    Ok(ToolSet::from_rmcp(server, listed))
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

/// The message of a server that is not ready.
fn not_ready(state: &State) -> String {
    match state {
        State::Starting => "still starting".to_owned(),
        State::Failed(message) | State::NeedsLogin(message) => message.clone(),
        State::Disabled => "disabled (/mcp enable)".to_owned(),
        State::Untrusted => "not trusted (/mcp trust)".to_owned(),
        State::Ready(_) | State::Stopped => "stopped".to_owned(),
    }
}

#[async_trait]
impl McpPort for McpManager {
    async fn list_tools(&self) -> Result<Vec<McpTool>, McpError> {
        Ok(self
            .servers
            .iter()
            .filter_map(|server| match &*server.state.borrow() {
                State::Ready(conn) => Some(conn.tools()),
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
        let State::Ready(conn) = state else {
            return Err(McpError::Unavailable {
                server: server.to_owned(),
                message: not_ready(&state),
            });
        };
        let Some(original) = conn.original(name) else {
            return Err(McpError::UnknownTool {
                server: server.to_owned(),
                name: name.to_owned(),
            });
        };
        let mut params = CallToolRequestParams::new(original);
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
                    let message = entry.stderr.explain("connection closed");
                    let same = entry.state.send_if_modified(|current| {
                        let same = matches!(current, State::Ready(c) if Arc::ptr_eq(c, &conn));
                        if same {
                            *current = State::Failed(message);
                        }
                        same
                    });
                    bump(&self.changes);
                    // One automatic reconnection per server and run.
                    if same && !entry.reconnected.swap(true, Ordering::SeqCst) {
                        tracing::warn!(target: "crest::mcp", server, "connection closed; reconnecting");
                        self.spawn(entry);
                    }
                }
                Err(convert::error(server, err))
            }
        }
    }

    async fn servers(&self) -> Vec<McpServerStatus> {
        self.servers
            .iter()
            .map(|server| McpServerStatus {
                name: server.spec.name.clone(),
                state: server.state.borrow().status(),
                transport: describe(&server.spec),
            })
            .collect()
    }

    async fn restart(&self, name: &str) -> Result<(), McpError> {
        let server = self.server(name)?;
        if matches!(*server.state.borrow(), State::Untrusted) {
            return Err(McpError::Unavailable {
                server: name.to_owned(),
                message: "not trusted: /mcp trust it first".into(),
            });
        }
        if let Some(problem) = invalid_name(name) {
            return Err(McpError::Unavailable {
                server: name.to_owned(),
                message: problem,
            });
        }
        server.abort();
        let old = server.state.send_replace(State::Starting);
        close(old).await;
        server.reconnected.store(false, Ordering::SeqCst);
        self.spawn(server);
        Ok(())
    }

    async fn set_enabled(&self, name: &str, enabled: bool) -> Result<(), McpError> {
        let server = self.server(name)?;
        let disabled = matches!(*server.state.borrow(), State::Disabled);
        if enabled {
            if disabled {
                server.state.send_replace(State::Stopped);
                return self.restart(name).await;
            }
            return Ok(());
        }
        if matches!(*server.state.borrow(), State::Untrusted) {
            return Ok(());
        }
        server.abort();
        let old = server.state.send_replace(State::Disabled);
        bump(&self.changes);
        close(old).await;
        Ok(())
    }

    async fn login(&self, name: &str) -> Result<String, McpError> {
        let server = self.server(name)?;
        let McpTransport::Http {
            url, oauth: client, ..
        } = &server.spec.transport
        else {
            return Err(McpError::Unavailable {
                server: name.to_owned(),
                message: "only HTTP servers log in; a stdio server takes its token in `env`".into(),
            });
        };
        let url = crate::connect::expand(url, "url").map_err(|message| McpError::Unavailable {
            server: name.to_owned(),
            message,
        })?;
        let show = Arc::clone(&self.options.show_login_url);
        let server_name = name.to_owned();
        let show_url = move |page: &str| show(&server_name, page);
        oauth::login(name, &url, client.as_ref(), &show_url)
            .await
            .map_err(|message| McpError::Unavailable {
                server: name.to_owned(),
                message,
            })?;
        self.restart(name).await?;
        Ok(format!("logged in to MCP server `{name}`; reconnecting"))
    }

    async fn logout(&self, name: &str) -> Result<bool, McpError> {
        let server = self.server(name)?;
        let McpTransport::Http { url, .. } = &server.spec.transport else {
            return Ok(false);
        };
        let url = crate::connect::expand(url, "url").unwrap_or_else(|_| url.clone());
        let deleted = oauth::KeyringStore::new(name, &url)
            .delete()
            .await
            .map_err(|message| McpError::Unavailable {
                server: name.to_owned(),
                message,
            })?;
        if deleted {
            self.restart(name).await?;
        }
        Ok(deleted)
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

    #[test]
    fn transports_are_described_for_lists() {
        let spec = McpServerSpec::new(
            "fs",
            McpTransport::Stdio {
                command: "npx".into(),
                args: vec![
                    "-y".into(),
                    "@modelcontextprotocol/server-filesystem".into(),
                ],
                env: std::collections::BTreeMap::new(),
                cwd: None,
            },
        );
        assert_eq!(
            describe(&spec),
            "stdio: npx -y @modelcontextprotocol/server-filesystem"
        );
    }
}
