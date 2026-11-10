//! [`McpManager`]: the configured servers, their connections and the
//! [`McpPort`] over them.

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::{Duration, Instant};

use application_core::ports::events::NoticeLevel;
use application_core::ports::mcp::{
    McpError, McpLogEntry, McpPort, McpServerState, McpServerStatus, McpTool, McpToolResult,
    invalid_server_name,
};
use application_core::ports::plugin::{McpServerSpec, McpTransport};
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

/// Entries kept in a server's log.
const LOG_LINES: usize = 30;

/// Limits and settings of an [`McpManager`].
#[derive(Clone)]
pub struct McpOptions {
    /// Limit for starting a server, the handshake and listing its tools,
    /// unless its spec says otherwise. Long enough for the first `npx -y`,
    /// which downloads the package.
    pub connect_timeout: Duration,
    /// Limit for one tool call, unless the server's spec says otherwise.
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
    /// What happened to it, newest first.
    log: Mutex<VecDeque<McpLogEntry>>,
    /// Whether an OAuth token is stored for it (http servers).
    logged_in: AtomicBool,
    /// The login waiting for the browser, if one does.
    login: Mutex<Option<AbortHandle>>,
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

    /// Records `text` at `level`, dropping the oldest entry past the cap.
    fn note(&self, level: NoticeLevel, text: impl Into<String>) {
        let mut log = self.log.lock().unwrap_or_else(PoisonError::into_inner);
        if log.len() == LOG_LINES {
            log.pop_back();
        }
        log.push_front(McpLogEntry::now(level, text));
    }

    fn entries(&self) -> Vec<McpLogEntry> {
        self.log
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .cloned()
            .collect()
    }

    /// Gives up a login that waits for the browser; whether one did.
    fn cancel_login(&self) -> bool {
        match self
            .login
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            Some(login) => {
                login.abort();
                true
            }
            None => false,
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

/// What [`probe`] found out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProbeReport {
    /// How long the start, the handshake and the tool list took.
    pub elapsed: Duration,
    /// How many tools the server advertises.
    pub tool_count: usize,
}

/// The MCP servers declared in the config, connected in the background.
///
/// [`start`](Self::start) launches every server at once; each one moves
/// from `Starting` to `Ready`, `Failed` or `NeedsLogin` on its own, and a
/// failing server never affects the others. Tools are listed when the
/// server connects and again when it sends `tools/list_changed`; a server
/// whose connection drops is reconnected once, then stays `Failed` until
/// [`restart`](McpPort::restart). Servers can be [added](Self::add),
/// [replaced](Self::replace) and [removed](Self::remove) while the rest
/// run. Every change bumps the counter of [`subscribe`](Self::subscribe).
///
/// Dropping the manager closes every session; stdio servers are killed.
pub struct McpManager {
    servers: RwLock<Vec<Arc<Server>>>,
    options: McpOptions,
    changes: Arc<watch::Sender<u64>>,
}

impl fmt::Debug for McpManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("McpManager")
            .field(
                "servers",
                &self
                    .snapshot()
                    .iter()
                    .map(|s| s.spec.name.clone())
                    .collect::<Vec<_>>(),
            )
            .field("options", &self.options)
            .finish_non_exhaustive()
    }
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

/// A server of `spec` in its initial state, and whether it starts now.
fn build(spec: McpServerSpec, trusted: bool, duplicate: bool) -> (Arc<Server>, bool) {
    let problem = invalid_server_name(&spec.name)
        .or_else(|| duplicate.then(|| format!("duplicate MCP server `{}`", spec.name)));
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
        log: Mutex::new(VecDeque::new()),
        logged_in: AtomicBool::new(false),
        login: Mutex::new(None),
    });
    (server, starting)
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
        let mut servers: Vec<Arc<Server>> = Vec::with_capacity(entries.len());
        for McpEntry { spec, trusted } in entries {
            let duplicate = servers.iter().any(|s| s.spec.name == spec.name);
            let (server, starting) = build(spec, trusted, duplicate);
            if starting {
                spawn(&server, &options, &changes);
            }
            servers.push(server);
        }
        Self {
            servers: RwLock::new(servers),
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

    fn snapshot(&self) -> Vec<Arc<Server>> {
        self.servers
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Waits until no server is `Starting` (each one is bounded by its
    /// connect timeout).
    pub async fn wait_ready(&self) {
        for server in self.snapshot() {
            let mut rx = server.state.subscribe();
            // The sender lives in the server, which we hold.
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
            server.note(NoticeLevel::Info, "trusted for this workspace");
            self.spawn(&server);
        }
        Ok(())
    }

    /// Closes every session and marks the servers `Stopped`. Stdio servers
    /// get their stdin closed and are killed if they do not exit within a
    /// few seconds.
    pub async fn shutdown(&self) {
        for server in self.snapshot() {
            stop(&server).await;
            server.note(NoticeLevel::Info, "stopped");
        }
        bump(&self.changes);
    }

    /// Adds a server while the rest run; it starts at once when trusted
    /// and enabled.
    ///
    /// # Errors
    ///
    /// [`McpError::Unavailable`] when a server of that name exists or the
    /// name cannot qualify tool names.
    pub fn add(&self, entry: McpEntry) -> Result<(), McpError> {
        let McpEntry { spec, trusted } = entry;
        if let Some(problem) = invalid_server_name(&spec.name) {
            return Err(McpError::Unavailable {
                server: spec.name,
                message: problem,
            });
        }
        let mut servers = self.servers.write().unwrap_or_else(PoisonError::into_inner);
        if servers.iter().any(|s| s.spec.name == spec.name) {
            return Err(McpError::Unavailable {
                server: spec.name.clone(),
                message: format!("there is already an MCP server `{}`", spec.name),
            });
        }
        let (server, starting) = build(spec, trusted, false);
        server.note(NoticeLevel::Info, "added");
        if starting {
            spawn(&server, &self.options, &self.changes);
        }
        servers.push(server);
        drop(servers);
        bump(&self.changes);
        Ok(())
    }

    /// Stops and forgets server `name`; its tools are gone at once.
    ///
    /// # Errors
    ///
    /// [`McpError::UnknownServer`] for an unknown name.
    pub async fn remove(&self, name: &str) -> Result<(), McpError> {
        let server = self.server(name)?;
        stop(&server).await;
        self.servers
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|s| !Arc::ptr_eq(s, &server));
        bump(&self.changes);
        Ok(())
    }

    /// Stops the server named like `entry` and starts `entry` in its
    /// place, keeping its position in the list.
    ///
    /// # Errors
    ///
    /// [`McpError::UnknownServer`] when no server has that name.
    pub async fn replace(&self, entry: McpEntry) -> Result<(), McpError> {
        let old = self.server(&entry.spec.name)?;
        stop(&old).await;
        let (server, starting) = build(entry.spec, entry.trusted, false);
        server.note(NoticeLevel::Info, "config saved · restarting");
        {
            let mut servers = self.servers.write().unwrap_or_else(PoisonError::into_inner);
            match servers.iter().position(|s| Arc::ptr_eq(s, &old)) {
                Some(at) => servers[at] = Arc::clone(&server),
                None => servers.push(Arc::clone(&server)),
            }
        }
        if starting {
            self.spawn(&server);
        }
        bump(&self.changes);
        Ok(())
    }

    /// The specs of every server, in list order.
    #[must_use]
    pub fn specs(&self) -> Vec<McpServerSpec> {
        self.snapshot().iter().map(|s| s.spec.clone()).collect()
    }

    /// The spec of server `name`.
    #[must_use]
    pub fn spec(&self, name: &str) -> Option<McpServerSpec> {
        self.server(name).ok().map(|s| s.spec.clone())
    }

    /// What happened to server `name`, newest first.
    ///
    /// # Errors
    ///
    /// [`McpError::UnknownServer`] for an unknown name.
    pub fn log(&self, name: &str) -> Result<Vec<McpLogEntry>, McpError> {
        Ok(self.server(name)?.entries())
    }

    /// The last lines server `name` wrote on stderr (stdio servers).
    ///
    /// # Errors
    ///
    /// [`McpError::UnknownServer`] for an unknown name.
    pub fn stderr(&self, name: &str) -> Result<Vec<String>, McpError> {
        Ok(self.server(name)?.stderr.lines())
    }

    /// Adds `text` to the log of server `name` (what the window did to
    /// it, such as a test).
    pub fn note(&self, name: &str, level: NoticeLevel, text: impl Into<String>) {
        if let Ok(server) = self.server(name) {
            server.note(level, text);
            bump(&self.changes);
        }
    }

    /// Whether an OAuth token is stored for server `name`.
    ///
    /// # Errors
    ///
    /// [`McpError::UnknownServer`] for an unknown name.
    pub fn logged_in(&self, name: &str) -> Result<bool, McpError> {
        Ok(self.server(name)?.logged_in.load(Ordering::SeqCst))
    }

    /// Gives up the login of server `name` that waits for the browser;
    /// whether one did. Its [`login`](McpPort::login) then fails with
    /// "login cancelled".
    #[must_use]
    pub fn cancel_login(&self, name: &str) -> bool {
        self.server(name).is_ok_and(|server| server.cancel_login())
    }

    fn server(&self, name: &str) -> Result<Arc<Server>, McpError> {
        self.servers
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .find(|s| s.spec.name == name)
            .cloned()
            .ok_or_else(|| McpError::UnknownServer(name.to_owned()))
    }
}

impl Drop for McpManager {
    fn drop(&mut self) {
        for server in self.snapshot() {
            server.abort();
            server.cancel_login();
        }
    }
}

/// Connects `spec` once, lists its tools and closes the session, without
/// registering it anywhere: the "Test connection" of the manager's form.
///
/// # Errors
///
/// What went wrong, with the stderr tail of a stdio server; a server that
/// asks for a login counts as reachable but failed.
pub async fn probe(spec: &McpServerSpec, options: &McpOptions) -> Result<ProbeReport, String> {
    if let Some(problem) = invalid_server_name(&spec.name) {
        return Err(problem);
    }
    let ctx = ConnectContext {
        default_cwd: options.default_cwd.clone(),
        stderr: StderrTail::default(),
        tools_changed: Arc::new(Notify::new()),
    };
    let limit = spec
        .startup_timeout_ms
        .map_or(options.connect_timeout, Duration::from_millis);
    let started = Instant::now();
    let attempt = async {
        let client = connect(spec, &ctx).await?;
        let tools = list_tools(&client, &spec.name)
            .await
            .map_err(ConnectError::Failed)?;
        Ok::<_, ConnectError>((client, tools.tools.len()))
    };
    match tokio::time::timeout(limit, attempt).await {
        Ok(Ok((mut client, tool_count))) => {
            let elapsed = started.elapsed();
            let _ = client.close_with_timeout(Duration::from_secs(5)).await;
            Ok(ProbeReport {
                elapsed,
                tool_count,
            })
        }
        Ok(Err(ConnectError::Failed(message))) => Err(message),
        Ok(Err(ConnectError::NeedsLogin(message))) => {
            Err(format!("reachable, but it needs a login: {message}"))
        }
        Err(_) => Err(ctx.stderr.explain(&format!(
            "did not start within {}s (set `startup_timeout` for slow servers)",
            limit.as_secs()
        ))),
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

/// Aborts whatever `server` does and closes its session; it ends `Stopped`.
async fn stop(server: &Arc<Server>) {
    server.abort();
    server.cancel_login();
    let old = server.state.send_replace(State::Stopped);
    close(old).await;
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
    if let McpTransport::Http { url, .. } = &spec.transport {
        let url = crate::connect::expand(url, "url").unwrap_or_else(|_| url.clone());
        let stored = oauth::KeyringStore::new(&spec.name, &url)
            .has_tokens()
            .await;
        server.logged_in.store(stored, Ordering::SeqCst);
    }
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
            server.note(
                NoticeLevel::Info,
                format!("connected · {} tools", conn.tools().len()),
            );
            State::Ready(conn)
        }
        Ok(Err(ConnectError::Failed(message))) => State::Failed(message),
        Ok(Err(ConnectError::NeedsLogin(message))) => State::NeedsLogin(message),
        Err(_) => State::Failed(server.stderr.explain(&format!(
            "did not start within {}s (set `startup_timeout` for slow servers)",
            limit.as_secs()
        ))),
    };
    match &next {
        State::Failed(message) => {
            tracing::warn!(target: "crest::mcp", server = %spec.name, "MCP server failed: {message}");
            server.note(NoticeLevel::Error, first_line(message));
        }
        State::NeedsLogin(message) => server.note(NoticeLevel::Warning, first_line(message)),
        _ => {}
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
                let count = tools.tools.len();
                *conn.tools.write().unwrap_or_else(PoisonError::into_inner) = tools;
                tracing::info!(target: "crest::mcp", server = %spec.name, "MCP tools reloaded");
                server.note(NoticeLevel::Info, format!("tools reloaded · {count} tools"));
                bump(&changes);
            }
            Err(message) => {
                tracing::warn!(target: "crest::mcp", server = %spec.name, "{message}");
                server.note(NoticeLevel::Warning, message);
                bump(&changes);
            }
        }
    }
}

/// The first line of a failure message (the stderr tail goes to the
/// Overview, not the log).
fn first_line(message: &str) -> String {
    message.lines().next().unwrap_or_default().to_owned()
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
            .snapshot()
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
        let limit = entry
            .spec
            .timeout_ms
            .map_or(self.options.call_timeout, Duration::from_millis);
        match call_tool(conn.client.peer(), params, limit).await {
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
                        entry.note(NoticeLevel::Warning, "connection closed · reconnecting");
                        self.spawn(&entry);
                    } else if same {
                        entry.note(NoticeLevel::Error, "connection closed");
                    }
                }
                Err(convert::error(server, err))
            }
        }
    }

    async fn servers(&self) -> Vec<McpServerStatus> {
        self.snapshot()
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
        if let Some(problem) = invalid_server_name(name) {
            return Err(McpError::Unavailable {
                server: name.to_owned(),
                message: problem,
            });
        }
        server.abort();
        let old = server.state.send_replace(State::Starting);
        close(old).await;
        server.reconnected.store(false, Ordering::SeqCst);
        server.note(NoticeLevel::Info, "restarting");
        self.spawn(&server);
        Ok(())
    }

    async fn set_enabled(&self, name: &str, enabled: bool) -> Result<(), McpError> {
        let server = self.server(name)?;
        let disabled = matches!(*server.state.borrow(), State::Disabled);
        if enabled {
            if disabled {
                server.state.send_replace(State::Stopped);
                server.note(NoticeLevel::Info, "enabled");
                return self.restart(name).await;
            }
            return Ok(());
        }
        if matches!(*server.state.borrow(), State::Untrusted) {
            return Ok(());
        }
        server.abort();
        let old = server.state.send_replace(State::Disabled);
        server.note(NoticeLevel::Info, "disabled");
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
        let client = client.clone();
        let server_name = name.to_owned();
        let login = tokio::spawn(async move {
            let shown = server_name.clone();
            let show_url = move |page: &str| show(&shown, page);
            oauth::login(&server_name, &url, client.as_ref(), &show_url).await
        });
        // One login per server: a new one replaces one still waiting.
        if let Some(previous) = server
            .login
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .replace(login.abort_handle())
        {
            previous.abort();
        }
        let outcome = login.await;
        server
            .login
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let unavailable = |message: String| McpError::Unavailable {
            server: name.to_owned(),
            message,
        };
        match outcome {
            Ok(Ok(())) => {}
            Ok(Err(message)) => {
                server.note(NoticeLevel::Warning, format!("login failed: {message}"));
                return Err(unavailable(message));
            }
            Err(join) if join.is_cancelled() => {
                server.note(NoticeLevel::Info, "login cancelled");
                return Err(unavailable("login cancelled".to_owned()));
            }
            Err(join) => return Err(unavailable(join.to_string())),
        }
        server.logged_in.store(true, Ordering::SeqCst);
        server.note(NoticeLevel::Info, "logged in (oauth)");
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
        server.logged_in.store(false, Ordering::SeqCst);
        if deleted {
            server.note(
                NoticeLevel::Info,
                "logged out · token deleted from the keyring",
            );
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
        assert_eq!(invalid_server_name("github"), None);
        assert_eq!(invalid_server_name("my-server_2"), None);
        assert!(invalid_server_name("").is_some());
        assert!(invalid_server_name("a__b").is_some());
        assert!(invalid_server_name("con espacio").is_some());
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

    #[test]
    fn the_log_keeps_the_newest_entries_first_and_caps_them() {
        let (server, _) = build(
            McpServerSpec::new(
                "x",
                McpTransport::Stdio {
                    command: "true".into(),
                    args: Vec::new(),
                    env: std::collections::BTreeMap::new(),
                    cwd: None,
                },
            ),
            true,
            false,
        );
        for n in 0..(LOG_LINES + 5) {
            server.note(NoticeLevel::Info, format!("entry {n}"));
        }
        let entries = server.entries();
        assert_eq!(entries.len(), LOG_LINES);
        assert_eq!(entries[0].text, format!("entry {}", LOG_LINES + 4));
        assert_eq!(entries[LOG_LINES - 1].text, "entry 5");
        assert_eq!(first_line("boom\nstderr:\nline"), "boom");
    }
}
