//! Which MCP servers may start, and the store of approved workspace
//! servers.
//!
//! Servers of the global config (and its plugins) start right away: the
//! user wrote them. Servers of `<workspace>/.crest/init.lua` come with the
//! repository, and starting one runs a program or contacts a URL chosen by
//! whoever wrote that repository. They stay pending, with a warning, until
//! the user approves them once with `crest --trust-mcp <name>`.
//!
//! An approval is stored per workspace (its canonical root) together with
//! the exact transport (command, arguments, environment, URL, headers) that
//! was approved, so a later change to the declaration asks again. The store
//! is `$XDG_DATA_HOME/crest/mcp-trust.json` (by default
//! `~/.local/share/crest/`); deleting an entry revokes it. The file holds
//! the declared values, header values included, so it is written readable
//! only by its owner.

use std::collections::{BTreeMap, HashMap};
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, PoisonError, RwLock};
use std::time::Duration;

use adapter_mcp::{McpEntry, McpManager, McpOptions};
use application::ports::events::{DomainEvent, EventSink, NoticeLevel};
use application::ports::mcp::{McpError, McpPort, McpServerState, McpServerStatus, McpToolResult};
use application::ports::plugin::{ConfigOrigin, McpServerSpec, McpTransport};
use application::tools::mcp::McpTool;
use application::tools::registry::{Tool, ToolRegistry, ToolSource};
use async_trait::async_trait;
use domain::permission::ToolEffect;
use serde::{Deserialize, Serialize};

/// Name of the store file in crest's data directory.
const TRUST_FILE: &str = "mcp-trust.json";

/// Default location of the store, following `XDG_DATA_HOME`.
#[must_use]
pub fn default_trust_path() -> Option<PathBuf> {
    trust_path_from(std::env::var_os("XDG_DATA_HOME"), dirs::home_dir())
}

fn trust_path_from(xdg_data_home: Option<OsString>, home: Option<PathBuf>) -> Option<PathBuf> {
    let data_home = xdg_data_home
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| home.map(|h| h.join(".local").join("share")))?;
    Some(data_home.join("crest").join(TRUST_FILE))
}

/// Approved workspace servers, by workspace root and server name.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustStore {
    #[serde(default)]
    workspaces: BTreeMap<String, BTreeMap<String, McpTransport>>,
}

impl TrustStore {
    /// Reads the store at `path`; a missing file is an empty store.
    ///
    /// # Errors
    ///
    /// The file cannot be read or is not a valid store.
    pub fn load(path: &Path) -> io::Result<Self> {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e),
        }
    }

    /// Writes the store to `path`, creating its directory.
    ///
    /// # Errors
    ///
    /// The file cannot be written.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        let tmp = path.with_extension("json.tmp");
        write_private(&tmp, &json)?;
        std::fs::rename(&tmp, path)
    }

    /// Whether `spec`, declared in `workspace`, was approved as it is now.
    #[must_use]
    pub fn is_trusted(&self, workspace: &Path, spec: &McpServerSpec) -> bool {
        self.workspaces
            .get(&key(workspace))
            .and_then(|servers| servers.get(&spec.name))
            .is_some_and(|approved| approved == &spec.transport)
    }

    /// Approves `spec` for `workspace`, replacing an earlier approval.
    pub fn trust(&mut self, workspace: &Path, spec: &McpServerSpec) {
        self.workspaces
            .entry(key(workspace))
            .or_default()
            .insert(spec.name.clone(), spec.transport.clone());
    }
}

fn key(workspace: &Path) -> String {
    workspace.display().to_string()
}

/// Writes `bytes` to `path`, readable only by its owner.
#[cfg(unix)]
pub(crate) fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)
}

/// Writes `bytes` to `path`.
#[cfg(not(unix))]
pub(crate) fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    std::fs::write(path, bytes)
}

/// Servers split into those to start and those held back.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Admission {
    /// Servers to start.
    pub start: Vec<McpServerSpec>,
    /// Workspace servers waiting for approval.
    pub pending: Vec<McpServerSpec>,
    /// Workspace servers approved now (`--trust-mcp`), to save.
    pub newly_trusted: Vec<McpServerSpec>,
}

/// Decides which of `specs` start in `workspace`: global ones, workspace
/// ones approved before as they are, and those named in `trust_now`.
#[must_use]
pub fn admit(
    specs: Vec<McpServerSpec>,
    workspace: &Path,
    store: &TrustStore,
    trust_now: &[String],
) -> Admission {
    let mut out = Admission::default();
    for spec in specs {
        if spec.origin == ConfigOrigin::Global || store.is_trusted(workspace, &spec) {
            out.start.push(spec);
        } else if trust_now.contains(&spec.name) {
            out.newly_trusted.push(spec.clone());
            out.start.push(spec);
        } else {
            out.pending.push(spec);
        }
    }
    out
}

/// What a held-back server looks like to the user.
#[must_use]
pub fn pending_warning(spec: &McpServerSpec) -> String {
    let what = match &spec.transport {
        McpTransport::Stdio { command, args, .. } => {
            let mut line = command.clone();
            for arg in args {
                line.push(' ');
                line.push_str(arg);
            }
            format!("it would run `{line}`")
        }
        McpTransport::Http { url, .. } => format!("it would connect to {url}"),
    };
    format!(
        "workspace MCP server `{name}` not started: {what}. If you trust it, \
         start crest with `crest --trust-mcp {name}` (remembered for this workspace)",
        name = spec.name
    )
}

/// Approves the workspace server `name` of `specs` (`/mcp trust <name>`)
/// and saves it to the store at `path`, as `--trust-mcp` does. The server
/// starts the next time crest opens the workspace. Returns what to tell
/// the user.
///
/// # Errors
///
/// The workspace declares no server by that name, there is no store, or
/// the store cannot be read or written.
pub fn trust_declared(
    specs: &[McpServerSpec],
    workspace: &Path,
    path: Option<&Path>,
    name: &str,
) -> Result<String, String> {
    let Some(spec) = specs.iter().find(|s| s.name == name) else {
        return Err(format!(
            "the workspace declares no MCP server `{name}` (nor does the global config)"
        ));
    };
    if spec.origin == ConfigOrigin::Global {
        return Ok(format!(
            "the MCP server `{name}` comes from the global config: it needs no approval"
        ));
    }
    let path = path.ok_or_else(|| {
        "nowhere to save the approval (no data directory); use --trust-mcp".to_owned()
    })?;
    let mut store =
        TrustStore::load(path).map_err(|e| format!("cannot read the approved MCP servers: {e}"))?;
    if store.is_trusted(workspace, spec) {
        return Ok(format!("the MCP server `{name}` was already approved"));
    }
    store.trust(workspace, spec);
    store
        .save(path)
        .map_err(|e| format!("cannot save the approval to {}: {e}", path.display()))?;
    Ok(format!(
        "MCP server `{name}` approved for this workspace; it will start the next time you \
         open crest"
    ))
}

/// How long startup waits for the servers before the first turn; slower
/// ones are offered once they are ready.
pub const READY_TIMEOUT: Duration = Duration::from_secs(5);

/// Where the store lives and what the command line approved.
#[derive(Debug, Clone, Default)]
pub struct TrustOptions {
    /// Store file; `None` keeps no approvals (every workspace server waits
    /// unless approved on this run).
    pub store: Option<PathBuf>,
    /// `--trust-mcp` names.
    pub trust_now: Vec<String>,
}

/// The MCP servers of a session, connected.
pub struct McpServers {
    /// The manager, to shut the servers down and start trusted ones.
    pub manager: Arc<McpManager>,
    /// The port the tools and `/mcp` go through (per-server call limits).
    pub port: Arc<dyn McpPort>,
    /// Their state right after startup.
    pub statuses: Vec<McpServerStatus>,
}

impl std::fmt::Debug for McpServers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpServers")
            .field("manager", &self.manager)
            .field("statuses", &self.statuses)
            .finish_non_exhaustive()
    }
}

/// The tools of the ready MCP servers, kept up to date as servers connect,
/// reconnect, change their tools or are turned off.
pub struct McpToolSource {
    port: Arc<dyn McpPort>,
    effects: HashMap<String, ToolEffect>,
    tools: RwLock<Vec<Arc<dyn Tool>>>,
}

impl std::fmt::Debug for McpToolSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpToolSource").finish_non_exhaustive()
    }
}

impl McpToolSource {
    /// Lists the tools of the ready servers again.
    pub async fn refresh(&self) {
        let tools = match self.port.list_tools().await {
            Ok(tools) => tools,
            Err(err) => {
                eprintln!("crest: cannot list the MCP tools: {err}");
                return;
            }
        };
        let tools = tools
            .into_iter()
            .map(|spec| {
                let effect = self
                    .effects
                    .get(&spec.server)
                    .copied()
                    .unwrap_or(ToolEffect::Execute);
                Arc::new(McpTool::new(spec, Arc::clone(&self.port)).with_effect(effect))
                    as Arc<dyn Tool>
            })
            .collect();
        *self.tools.write().unwrap_or_else(PoisonError::into_inner) = tools;
    }
}

impl ToolSource for McpToolSource {
    fn tools(&self) -> Vec<Arc<dyn Tool>> {
        self.tools
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// What a state change of a server tells the user, if anything.
fn change_notice(name: &str, state: &McpServerState) -> Option<(NoticeLevel, String)> {
    match state {
        McpServerState::Ready { tool_count } => Some((
            NoticeLevel::Info,
            format!("MCP server `{name}` ready ({tool_count} tools)"),
        )),
        McpServerState::Failed { message } => Some((
            NoticeLevel::Warning,
            format!("MCP server `{name}` failed: {message}"),
        )),
        McpServerState::NeedsLogin { message } => Some((
            NoticeLevel::Warning,
            format!("MCP server `{name}`: {message}"),
        )),
        _ => None,
    }
}

/// Opens `url` in the browser and says so, with the URL in case it did
/// not open.
fn login_page(events: Option<Arc<dyn EventSink>>) -> adapter_mcp::ShowLoginUrl {
    Arc::new(move |server: &str, url: &str| {
        let opener = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        let opened = std::process::Command::new(opener)
            .arg(url)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .is_ok();
        let message = if opened {
            format!("MCP server `{server}`: log in in the browser (if it did not open: {url})")
        } else {
            format!("MCP server `{server}`: open {url} to log in")
        };
        if let Some(events) = &events {
            events.emit(DomainEvent::Notice {
                level: NoticeLevel::Info,
                message,
            });
        }
    })
}

/// Starts the servers of `specs` that may start in `workspace` (workspace
/// servers not approved wait for `/mcp trust`), waits up to
/// `ready_timeout` for them and adds their tools to `registry` as a
/// source that follows the servers for the rest of the session. State
/// changes after startup go to `events` as notices. Returns the servers
/// (none when nothing is declared) and the startup warnings: held-back,
/// failed, slow or logged-out servers, and store problems.
pub async fn start(
    specs: Vec<McpServerSpec>,
    workspace: &Path,
    trust: &TrustOptions,
    ready_timeout: Duration,
    registry: &mut ToolRegistry,
    events: Option<Arc<dyn EventSink>>,
) -> (Option<McpServers>, Vec<String>) {
    let mut warnings = Vec::new();
    let mut store = match trust.store.as_deref().map(TrustStore::load) {
        Some(Ok(store)) => store,
        Some(Err(err)) => {
            warnings.push(format!("cannot read the approved MCP servers: {err}"));
            TrustStore::default()
        }
        None => TrustStore::default(),
    };
    for name in &trust.trust_now {
        let declared = specs
            .iter()
            .any(|s| &s.name == name && s.origin == ConfigOrigin::Workspace);
        if !declared {
            warnings.push(format!(
                "--trust-mcp {name}: the workspace declares no MCP server with that name"
            ));
        }
    }
    let admission = admit(specs, workspace, &store, &trust.trust_now);
    warnings.extend(admission.pending.iter().map(pending_warning));
    if !admission.newly_trusted.is_empty() {
        for spec in &admission.newly_trusted {
            store.trust(workspace, spec);
        }
        if let Some(path) = &trust.store
            && let Err(err) = store.save(path)
        {
            warnings.push(format!(
                "cannot save the MCP server approvals to {}: {err}",
                path.display()
            ));
        }
    }
    if admission.start.is_empty() && admission.pending.is_empty() {
        return (None, warnings);
    }

    let defaults = McpOptions::default();
    let all = admission.start.iter().chain(&admission.pending);
    let timeouts: HashMap<String, Duration> = all
        .clone()
        .map(|s| {
            let limit = s
                .timeout_ms
                .map_or(defaults.call_timeout, Duration::from_millis);
            (s.name.clone(), limit)
        })
        .collect();
    let effects: HashMap<String, ToolEffect> = all.map(|s| (s.name.clone(), s.effect)).collect();
    // The manager's own limit must not cut a longer per-server timeout.
    let options = McpOptions {
        call_timeout: timeouts
            .values()
            .copied()
            .max()
            .unwrap_or(defaults.call_timeout),
        default_cwd: Some(workspace.to_path_buf()),
        show_login_url: login_page(events.clone()),
        ..defaults
    };
    let entries = admission
        .start
        .into_iter()
        .map(|spec| McpEntry {
            spec,
            trusted: true,
        })
        .chain(admission.pending.into_iter().map(|spec| McpEntry {
            spec,
            trusted: false,
        }))
        .collect();
    let manager = Arc::new(McpManager::start_entries(entries, options));
    let mut changes = manager.subscribe();
    let _ = tokio::time::timeout(ready_timeout, manager.wait_ready()).await;
    let statuses = manager.servers().await;
    for status in &statuses {
        match &status.state {
            McpServerState::Failed { message } => warnings.push(format!(
                "MCP server `{}` unavailable: {message}",
                status.name
            )),
            McpServerState::NeedsLogin { message } => {
                warnings.push(format!("MCP server `{}`: {message}", status.name));
            }
            McpServerState::Starting => warnings.push(format!(
                "MCP server `{}` is still starting; its tools are offered once it is ready",
                status.name
            )),
            McpServerState::Ready { .. }
            | McpServerState::Disabled
            | McpServerState::Untrusted
            | McpServerState::Stopped => {}
        }
    }

    let port: Arc<dyn McpPort> = Arc::new(TimedMcp {
        inner: Arc::clone(&manager) as Arc<dyn McpPort>,
        timeouts,
    });
    let source = Arc::new(McpToolSource {
        port: Arc::clone(&port),
        effects,
        tools: RwLock::new(Vec::new()),
    });
    source.refresh().await;
    changes.mark_unchanged();
    registry.add_source(source.clone());

    let watched = Arc::clone(&port);
    let mut last: HashMap<String, McpServerState> = statuses
        .iter()
        .map(|s| (s.name.clone(), s.state.clone()))
        .collect();
    tokio::spawn(async move {
        while changes.changed().await.is_ok() {
            source.refresh().await;
            for status in watched.servers().await {
                let before = last.insert(status.name.clone(), status.state.clone());
                if before.as_ref() == Some(&status.state) {
                    continue;
                }
                if let (Some(events), Some((level, message))) =
                    (&events, change_notice(&status.name, &status.state))
                {
                    events.emit(DomainEvent::Notice { level, message });
                }
            }
        }
    });
    (
        Some(McpServers {
            manager,
            port,
            statuses,
        }),
        warnings,
    )
}

/// A port with a time limit per server for each call.
struct TimedMcp {
    inner: Arc<dyn McpPort>,
    timeouts: HashMap<String, Duration>,
}

#[async_trait]
impl McpPort for TimedMcp {
    async fn list_tools(&self) -> Result<Vec<application::ports::mcp::McpTool>, McpError> {
        self.inner.list_tools().await
    }

    async fn call_tool(
        &self,
        server: &str,
        name: &str,
        input: serde_json::Value,
    ) -> Result<McpToolResult, McpError> {
        let call = self.inner.call_tool(server, name, input);
        match self.timeouts.get(server) {
            // Dropping the call on timeout cancels it on the server.
            Some(limit) => tokio::time::timeout(*limit, call)
                .await
                .unwrap_or_else(|_| {
                    Err(McpError::Timeout {
                        server: server.to_owned(),
                    })
                }),
            None => call.await,
        }
    }

    async fn servers(&self) -> Vec<McpServerStatus> {
        self.inner.servers().await
    }

    async fn restart(&self, server: &str) -> Result<(), McpError> {
        self.inner.restart(server).await
    }

    async fn set_enabled(&self, server: &str, enabled: bool) -> Result<(), McpError> {
        self.inner.set_enabled(server, enabled).await
    }

    async fn login(&self, server: &str) -> Result<String, McpError> {
        self.inner.login(server).await
    }

    async fn logout(&self, server: &str) -> Result<bool, McpError> {
        self.inner.logout(server).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stdio(name: &str, command: &str, origin: ConfigOrigin) -> McpServerSpec {
        McpServerSpec {
            origin,
            ..McpServerSpec::new(
                name,
                McpTransport::Stdio {
                    command: command.into(),
                    args: vec!["--x".into()],
                    env: BTreeMap::new(),
                    cwd: None,
                },
            )
        }
    }

    #[test]
    fn global_servers_start_and_workspace_ones_wait() {
        let ws = Path::new("/ws");
        let specs = vec![
            stdio("mine", "a", ConfigOrigin::Global),
            stdio("repo", "b", ConfigOrigin::Workspace),
        ];
        let got = admit(specs.clone(), ws, &TrustStore::default(), &[]);
        assert_eq!(got.start, [specs[0].clone()]);
        assert_eq!(got.pending, [specs[1].clone()]);
        assert!(got.newly_trusted.is_empty());
    }

    #[test]
    fn trusting_a_declared_server_saves_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trust.json");
        let ws = Path::new("/ws");
        let specs = [
            stdio("git", "mcp-git", ConfigOrigin::Workspace),
            stdio("docs", "mcp-docs", ConfigOrigin::Global),
        ];
        let said = trust_declared(&specs, ws, Some(&path), "git").unwrap();
        assert!(said.contains("next time"), "{said}");
        let store = TrustStore::load(&path).unwrap();
        assert!(store.is_trusted(ws, &specs[0]));
        let again = trust_declared(&specs, ws, Some(&path), "git").unwrap();
        assert!(again.contains("already approved"), "{again}");
        let global = trust_declared(&specs, ws, Some(&path), "docs").unwrap();
        assert!(global.contains("global"), "{global}");
        assert!(trust_declared(&specs, ws, Some(&path), "nope").is_err());
        assert!(trust_declared(&specs, ws, None, "git").is_err());
    }

    #[test]
    fn trusting_now_starts_it_and_asks_to_save() {
        let ws = Path::new("/ws");
        let repo = stdio("repo", "b", ConfigOrigin::Workspace);
        let got = admit(
            vec![repo.clone()],
            ws,
            &TrustStore::default(),
            &["repo".into()],
        );
        assert_eq!(got.start, std::slice::from_ref(&repo));
        assert_eq!(got.newly_trusted, [repo]);
    }

    #[test]
    fn an_approval_covers_only_that_workspace_and_that_transport() {
        let repo = stdio("repo", "b", ConfigOrigin::Workspace);
        let mut store = TrustStore::default();
        store.trust(Path::new("/ws"), &repo);
        assert!(store.is_trusted(Path::new("/ws"), &repo));
        assert!(!store.is_trusted(Path::new("/other"), &repo));
        let changed = stdio("repo", "evil", ConfigOrigin::Workspace);
        assert!(!store.is_trusted(Path::new("/ws"), &changed));
        let got = admit(vec![changed.clone()], Path::new("/ws"), &store, &[]);
        assert_eq!(got.pending, [changed]);
    }

    #[test]
    fn the_store_round_trips_through_its_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("deep").join(TRUST_FILE);
        assert_eq!(TrustStore::load(&path).unwrap(), TrustStore::default());
        let mut store = TrustStore::default();
        store.trust(
            Path::new("/ws"),
            &stdio("repo", "b", ConfigOrigin::Workspace),
        );
        store.save(&path).unwrap();
        assert_eq!(TrustStore::load(&path).unwrap(), store);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn a_corrupt_store_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(TRUST_FILE);
        std::fs::write(&path, "nope").unwrap();
        assert!(TrustStore::load(&path).is_err());
    }

    #[test]
    fn the_warning_says_what_would_run_and_how_to_allow_it() {
        let msg = pending_warning(&stdio("repo", "b", ConfigOrigin::Workspace));
        assert!(msg.contains("`b --x`"), "{msg}");
        assert!(msg.contains("crest --trust-mcp repo"), "{msg}");
    }

    /// A server whose `slow` tool never answers.
    struct Hanging;

    #[async_trait]
    impl McpPort for Hanging {
        async fn list_tools(&self) -> Result<Vec<application::ports::mcp::McpTool>, McpError> {
            Ok(Vec::new())
        }

        async fn call_tool(
            &self,
            _server: &str,
            name: &str,
            _input: serde_json::Value,
        ) -> Result<McpToolResult, McpError> {
            if name == "slow" {
                std::future::pending::<()>().await;
            }
            Ok(McpToolResult {
                content: Vec::new(),
                is_error: false,
            })
        }

        async fn servers(&self) -> Vec<McpServerStatus> {
            Vec::new()
        }
    }

    #[tokio::test]
    async fn each_server_has_its_own_call_timeout() {
        let port = TimedMcp {
            inner: Arc::new(Hanging),
            timeouts: HashMap::from([("short".to_owned(), Duration::from_millis(50))]),
        };
        let err = port
            .call_tool("short", "slow", serde_json::Value::Null)
            .await
            .unwrap_err();
        assert_eq!(
            err,
            McpError::Timeout {
                server: "short".into()
            }
        );
        assert!(
            port.call_tool("short", "fast", serde_json::Value::Null)
                .await
                .is_ok()
        );
    }

    #[test]
    fn the_store_follows_xdg_data_home() {
        assert_eq!(
            trust_path_from(Some("/data".into()), Some("/home/u".into())),
            Some(PathBuf::from("/data/crest/mcp-trust.json"))
        );
        assert_eq!(
            trust_path_from(Some("rel".into()), Some("/home/u".into())),
            Some(PathBuf::from("/home/u/.local/share/crest/mcp-trust.json"))
        );
        assert_eq!(trust_path_from(None, None), None);
    }
}
