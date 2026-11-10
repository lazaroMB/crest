//! What `/mcp` and the MCP servers manager of the window do: list the
//! servers with everything the window shows, act on one (test, restart,
//! log in or out, trust, turn on or off), and add, edit or remove the
//! servers of `mcp.json` while crest runs.

use std::path::PathBuf;
use std::sync::Arc;

use adapter_mcp::{McpEntry, McpManager, McpOptions, probe};
use application::ports::events::NoticeLevel;
use application::ports::mcp::{McpPort, McpServerState};
use application::ports::plugin::{ConfigOrigin, McpServerSpec, McpSource, McpTransport};
use application::usecases::commands::{MCP_USAGE, McpCommand, McpVerb, Reply};
use application::usecases::mcp_servers::{
    McpAction, McpDraft, McpListing, McpServerInfo, McpToolInfo, where_saved,
};
use domain::block::{Block, BlockKind, Cell, Column, TableSpec};
use futures::stream::{self, BoxStream, StreamExt as _};

use crate::mcp_config::{self, McpConfigPaths};

/// What `/mcp` and the manager need: the declared servers, the running
/// ones, and the files approvals and saved servers go to.
pub struct McpControl {
    /// The servers declared at startup, for when no manager runs.
    specs: Vec<McpServerSpec>,
    workspace: PathBuf,
    /// The store of approved workspace servers (`mcp-trust.json`).
    trust_path: Option<PathBuf>,
    /// The two `mcp.json`.
    config: McpConfigPaths,
    /// The running servers: the manager and the port over it.
    servers: Option<(Arc<McpManager>, Arc<dyn McpPort>)>,
}

impl std::fmt::Debug for McpControl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpControl")
            .field("workspace", &self.workspace)
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl McpControl {
    /// The control of `workspace`'s servers.
    #[must_use]
    pub fn new(
        specs: Vec<McpServerSpec>,
        workspace: PathBuf,
        trust_path: Option<PathBuf>,
        config: McpConfigPaths,
        servers: Option<(Arc<McpManager>, Arc<dyn McpPort>)>,
    ) -> Self {
        Self {
            specs,
            workspace,
            trust_path,
            config,
            servers,
        }
    }

    /// The port over the running servers, if they run.
    #[must_use]
    pub fn port(&self) -> Option<&Arc<dyn McpPort>> {
        self.servers.as_ref().map(|(_, port)| port)
    }

    fn manager(&self) -> Option<&Arc<McpManager>> {
        self.servers.as_ref().map(|(manager, _)| manager)
    }

    /// The servers as they are now: the manager's when it runs (servers
    /// are added and edited while crest runs), else the declared ones.
    #[must_use]
    pub fn specs(&self) -> Vec<McpServerSpec> {
        self.manager()
            .map_or_else(|| self.specs.clone(), |manager| manager.specs())
    }

    fn options(&self) -> McpOptions {
        McpOptions {
            default_cwd: Some(self.workspace.clone()),
            ..McpOptions::default()
        }
    }

    /// Yields once per state or tool change of any server; the window
    /// reloads the listing on each.
    pub fn changes(&self) -> BoxStream<'static, ()> {
        match self.manager() {
            Some(manager) => {
                let rx = manager.subscribe();
                stream::unfold(rx, |mut rx| async move {
                    rx.changed().await.ok().map(|()| ((), rx))
                })
                .boxed()
            }
            None => stream::pending().boxed(),
        }
    }

    // ------------------------------------------------------------ /mcp

    /// `/mcp <args>`, as the terminal answers it (the window opens the
    /// manager for most forms instead).
    pub async fn run(self: Arc<Self>, command: McpCommand) -> Reply {
        match command {
            McpCommand::List => self.list(None).await,
            McpCommand::Open(name) => self.list(Some(&name)).await,
            McpCommand::Add => Reply::Notice(
                NoticeLevel::Info,
                format!(
                    "servers are added from the window (/mcp) or in {}",
                    where_saved(ConfigOrigin::Global, McpSource::Json)
                ),
            ),
            McpCommand::Tools(name) => self.tools(&name).await,
            McpCommand::Trust(name) => {
                self.notice(self.act(&name, McpAction::Trust).await, "trust")
            }
            McpCommand::Manage {
                verb: McpVerb::Remove,
                name,
            } => self.notice(self.remove(&name).await, "remove"),
            McpCommand::Manage { verb, name } => {
                let action = match verb {
                    McpVerb::Restart => McpAction::Restart,
                    McpVerb::Enable => McpAction::Enable,
                    McpVerb::Disable => McpAction::Disable,
                    McpVerb::Login => McpAction::Login,
                    McpVerb::Logout => McpAction::Logout,
                    McpVerb::Test | McpVerb::Remove => McpAction::Test,
                };
                self.notice(self.act(&name, action).await, verb.as_str())
            }
            McpCommand::Invalid => Reply::Notice(NoticeLevel::Warning, MCP_USAGE.into()),
        }
    }

    #[allow(clippy::unused_self)]
    fn notice(&self, outcome: Result<String, String>, verb: &str) -> Reply {
        match outcome {
            Ok(message) => Reply::Notice(NoticeLevel::Info, message),
            Err(err) => Reply::Notice(NoticeLevel::Warning, format!("/mcp {verb}: {err}")),
        }
    }

    fn no_servers() -> Reply {
        Reply::Notice(
            NoticeLevel::Info,
            "no MCP server is declared (app.mcp in init.lua, or /mcp add in the window)".into(),
        )
    }

    /// Every server (or `only` that one) with its state, transport, origin
    /// and what to do next.
    async fn list(&self, only: Option<&str>) -> Reply {
        let Some(port) = self.port() else {
            return Self::no_servers();
        };
        let specs = self.specs();
        let statuses = port.servers().await;
        if statuses.is_empty() {
            return Self::no_servers();
        }
        if let Some(name) = only
            && !statuses.iter().any(|s| s.name == name)
        {
            return Reply::Notice(
                NoticeLevel::Warning,
                format!("/mcp: no MCP server `{name}`"),
            );
        }
        let origin = |name: &str| {
            specs
                .iter()
                .find(|s| s.name == name)
                .map_or("", |s| match s.origin {
                    ConfigOrigin::Global => "global",
                    ConfigOrigin::Workspace => "workspace",
                })
        };
        let rows = statuses
            .iter()
            .filter(|status| only.is_none_or(|name| status.name == name))
            .map(|status| {
                let detail = match &status.state {
                    McpServerState::Untrusted => format!("/mcp trust {}", status.name),
                    McpServerState::Disabled => format!("/mcp enable {}", status.name),
                    McpServerState::NeedsLogin { .. } => format!("/mcp login {}", status.name),
                    McpServerState::Ready { .. } => format!("/mcp tools {}", status.name),
                    state => state.message().unwrap_or_default().to_owned(),
                };
                vec![
                    Cell::Text(status.name.clone()),
                    Cell::Text(status.state.label()),
                    Cell::Text(status.transport.clone()),
                    Cell::Text(origin(&status.name).to_owned()),
                    Cell::Text(detail),
                ]
            })
            .collect();
        Reply::block(Block::new(BlockKind::Table(TableSpec {
            columns: ["MCP server", "State", "Transport", "Config", "Detail"]
                .map(Column::new)
                .to_vec(),
            rows,
        })))
    }

    /// The tools of `name`, as the model sees them.
    async fn tools(&self, name: &str) -> Reply {
        let Some(port) = self.port() else {
            return Self::no_servers();
        };
        let Some(status) = port.servers().await.into_iter().find(|s| s.name == name) else {
            return Reply::Notice(
                NoticeLevel::Warning,
                format!("/mcp: no MCP server `{name}`"),
            );
        };
        if !matches!(status.state, McpServerState::Ready { .. }) {
            let why = status.state.message().unwrap_or_default();
            return Reply::Notice(
                NoticeLevel::Warning,
                format!(
                    "MCP server `{name}` is {}{}",
                    status.state.label(),
                    if why.is_empty() {
                        String::new()
                    } else {
                        format!(": {why}")
                    }
                ),
            );
        }
        let tools = port.list_tools().await.unwrap_or_default();
        let rows: Vec<Vec<Cell>> = tools
            .into_iter()
            .filter(|t| t.server == name)
            .map(|tool| {
                let summary = tool.description.lines().next().unwrap_or_default();
                let summary: String = summary.chars().take(160).collect();
                vec![Cell::Text(tool.qualified_name()), Cell::Text(summary)]
            })
            .collect();
        if rows.is_empty() {
            return Reply::Notice(
                NoticeLevel::Info,
                format!("MCP server `{name}` offers no tools"),
            );
        }
        Reply::block(Block::new(BlockKind::Table(TableSpec {
            columns: vec![Column::new("Tool"), Column::new("Description")],
            rows,
        })))
    }

    // ------------------------------------------------------ the manager

    /// Every server with its state, config, tools and log.
    pub async fn listing(&self) -> McpListing {
        let files = |origin| {
            self.config
                .of(origin)
                .map(|path| path.display().to_string())
        };
        let mut listing = McpListing {
            servers: Vec::new(),
            can_edit: self.servers.is_some(),
            global_file: files(ConfigOrigin::Global),
            workspace_file: files(ConfigOrigin::Workspace),
        };
        let Some((manager, port)) = &self.servers else {
            listing.servers = self
                .specs
                .iter()
                .map(|spec| McpServerInfo::new(spec.clone(), McpServerState::Stopped))
                .collect();
            return listing;
        };
        let statuses = port.servers().await;
        let tools = port.list_tools().await.unwrap_or_default();
        for spec in manager.specs() {
            let state = statuses
                .iter()
                .find(|s| s.name == spec.name)
                .map_or(McpServerState::Stopped, |s| s.state.clone());
            let name = spec.name.clone();
            let http = matches!(spec.transport, McpTransport::Http { .. });
            let mut info = McpServerInfo::new(spec, state);
            info.tools = tools
                .iter()
                .filter(|t| t.server == name)
                .map(McpToolInfo::from)
                .collect();
            info.logged_in = http.then(|| manager.logged_in(&name).unwrap_or(false));
            info.stderr = manager.stderr(&name).unwrap_or_default();
            info.log = manager.log(&name).unwrap_or_default();
            listing.servers.push(info);
        }
        listing
    }

    /// Does `action` to server `name`; `Ok` is what to tell the user.
    pub async fn act(&self, name: &str, action: McpAction) -> Result<String, String> {
        if action == McpAction::Trust {
            return self.trust(name).await;
        }
        let Some((manager, port)) = &self.servers else {
            return Err("this session has no MCP servers".to_owned());
        };
        let err = |e: application::ports::mcp::McpError| e.to_string();
        match action {
            McpAction::Test => {
                let spec = manager
                    .spec(name)
                    .ok_or_else(|| format!("no MCP server `{name}`"))?;
                let outcome = probe(&spec, &self.options()).await;
                let (level, text) = match &outcome {
                    Ok(report) => (
                        NoticeLevel::Info,
                        format!(
                            "connected in {} ms · {} tools",
                            report.elapsed.as_millis(),
                            report.tool_count
                        ),
                    ),
                    Err(problem) => (NoticeLevel::Error, first_line(problem)),
                };
                manager.note(name, level, format!("test: {text}"));
                outcome.map(|_| text)
            }
            McpAction::Restart => port
                .restart(name)
                .await
                .map(|()| format!("MCP server `{name}` restarting"))
                .map_err(err),
            McpAction::Enable | McpAction::Disable => {
                let enabled = action == McpAction::Enable;
                port.set_enabled(name, enabled).await.map_err(err)?;
                let spec = manager.spec(name);
                let persisted = match &spec {
                    Some(spec) if spec.source == McpSource::Json => {
                        self.persist_disabled(spec, !enabled).await?;
                        true
                    }
                    _ => false,
                };
                Ok(match (enabled, persisted) {
                    (true, _) => format!("MCP server `{name}` enabled"),
                    (false, true) => format!("MCP server `{name}` disabled"),
                    (false, false) => format!(
                        "MCP server `{name}` disabled for this session (disabled = true in init.lua keeps it off)"
                    ),
                })
            }
            McpAction::Login => port.login(name).await.map_err(err),
            McpAction::CancelLogin => {
                if manager.cancel_login(name) {
                    Ok("login cancelled".to_owned())
                } else {
                    Err("no login is waiting".to_owned())
                }
            }
            McpAction::Logout => port.logout(name).await.map_err(err).map(|deleted| {
                if deleted {
                    format!("MCP server `{name}`: login tokens deleted")
                } else {
                    format!("MCP server `{name}` had no stored login")
                }
            }),
            McpAction::Trust => unreachable!("handled above"),
        }
    }

    /// Approves the workspace server `name` and starts it.
    async fn trust(&self, name: &str) -> Result<String, String> {
        // Reads and writes the store file: off the async workers.
        let specs = self.specs();
        let (workspace, store, owned) = (
            self.workspace.clone(),
            self.trust_path.clone(),
            name.to_owned(),
        );
        let saved = tokio::task::spawn_blocking(move || {
            crate::mcp::trust_declared(&specs, &workspace, store.as_deref(), &owned)
        })
        .await
        .unwrap_or_else(|err| Err(err.to_string()))?;
        if let Some(manager) = self.manager() {
            let _ = manager.trust(name);
        }
        Ok(saved)
    }

    /// Writes `disabled` for the `mcp.json` server `spec`.
    async fn persist_disabled(&self, spec: &McpServerSpec, disabled: bool) -> Result<(), String> {
        let path = self
            .config
            .of(spec.origin)
            .ok_or_else(|| format!("no mcp.json for {}", where_saved(spec.origin, spec.source)))?
            .to_path_buf();
        let name = spec.name.clone();
        tokio::task::spawn_blocking(move || {
            mcp_config::update(&path, |file| {
                if let Some(def) = file.servers.get_mut(&name) {
                    def.disabled = disabled;
                }
            })
        })
        .await
        .unwrap_or_else(|err| Err(err.to_string()))
    }

    /// Saves `draft` to the `mcp.json` of its scope (replacing `previous`
    /// when editing) and starts it. A workspace server saved here is
    /// approved at once: the user typed it.
    pub async fn save(&self, previous: Option<String>, draft: McpDraft) -> Result<String, String> {
        let Some((manager, _)) = &self.servers else {
            return Err("this session has no MCP servers".to_owned());
        };
        let old = match &previous {
            Some(name) => {
                let old = manager
                    .spec(name)
                    .ok_or_else(|| format!("no MCP server `{name}`"))?;
                if old.source == McpSource::Lua {
                    return Err(format!(
                        "`{name}` is declared in {}; edit it there",
                        where_saved(old.origin, old.source)
                    ));
                }
                Some(old)
            }
            None => None,
        };
        let taken: Vec<String> = manager
            .specs()
            .into_iter()
            .map(|s| s.name)
            .filter(|n| previous.as_deref() != Some(n.as_str()))
            .collect();
        let spec = draft.validate(&taken)?;
        let path = self
            .config
            .of(spec.origin)
            .ok_or_else(|| format!("no mcp.json for {}", where_saved(spec.origin, spec.source)))?
            .to_path_buf();
        let old_file = old
            .as_ref()
            .filter(|old| old.origin != spec.origin || old.name != spec.name)
            .and_then(|old| {
                self.config
                    .of(old.origin)
                    .map(|p| (p.to_path_buf(), old.name.clone()))
            });
        let (workspace, trust_path, written) = (
            self.workspace.clone(),
            self.trust_path.clone(),
            spec.clone(),
        );
        tokio::task::spawn_blocking(move || -> Result<(), String> {
            if let Some((path, name)) = old_file {
                mcp_config::update(&path, |file| {
                    file.remove(&name);
                })?;
            }
            mcp_config::update(&path, |file| file.upsert(&written))?;
            if written.origin == ConfigOrigin::Workspace
                && let Some(trust_path) = trust_path
            {
                let mut store = crate::mcp::TrustStore::load(&trust_path)
                    .map_err(|e| format!("cannot read the approved MCP servers: {e}"))?;
                store.trust(&workspace, &written);
                store
                    .save(&trust_path)
                    .map_err(|e| format!("cannot save the approval: {e}"))?;
            }
            Ok(())
        })
        .await
        .unwrap_or_else(|err| Err(err.to_string()))?;
        let saved_in = where_saved(spec.origin, spec.source);
        let name = spec.name.clone();
        let entry = McpEntry {
            spec,
            trusted: true,
        };
        match old {
            Some(old) if old.name == name => manager.replace(entry).await,
            Some(old) => {
                manager.remove(&old.name).await.map_err(|e| e.to_string())?;
                manager.add(entry)
            }
            None => manager.add(entry),
        }
        .map_err(|e| e.to_string())?;
        Ok(format!("MCP server `{name}` saved to {saved_in}; starting"))
    }

    /// Removes server `name` from its `mcp.json` and stops it.
    pub async fn remove(&self, name: &str) -> Result<String, String> {
        let Some((manager, _)) = &self.servers else {
            return Err("this session has no MCP servers".to_owned());
        };
        let spec = manager
            .spec(name)
            .ok_or_else(|| format!("no MCP server `{name}`"))?;
        let saved_in = where_saved(spec.origin, spec.source);
        if spec.source == McpSource::Lua {
            return Err(format!(
                "`{name}` is declared in {saved_in}; remove it there"
            ));
        }
        let path = self
            .config
            .of(spec.origin)
            .ok_or_else(|| format!("no mcp.json for {saved_in}"))?
            .to_path_buf();
        let owned = name.to_owned();
        tokio::task::spawn_blocking(move || {
            mcp_config::update(&path, |file| {
                file.remove(&owned);
            })
        })
        .await
        .unwrap_or_else(|err| Err(err.to_string()))?;
        manager.remove(name).await.map_err(|e| e.to_string())?;
        Ok(format!("MCP server `{name}` removed from {saved_in}"))
    }

    /// Connects `draft` once without saving it.
    pub async fn test(&self, draft: McpDraft) -> Result<String, String> {
        let spec = draft.validate(&[])?;
        let report = probe(&spec, &self.options()).await?;
        Ok(format!(
            "connected in {} ms · {} tools",
            report.elapsed.as_millis(),
            report.tool_count
        ))
    }
}

fn first_line(message: &str) -> String {
    message.lines().next().unwrap_or_default().to_owned()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::Path;

    use application::usecases::mcp_servers::McpKind;

    use super::*;

    fn echo_server_path() -> PathBuf {
        let exe = std::env::current_exe().unwrap();
        let profile_dir = exe.parent().and_then(Path::parent).unwrap();
        let path = profile_dir
            .join("examples")
            .join(format!("echo_server{}", std::env::consts::EXE_SUFFIX));
        if !path.exists() {
            let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
            let status = std::process::Command::new(cargo)
                .args(["build", "-p", "adapter-mcp", "--example", "echo_server"])
                .status()
                .unwrap();
            assert!(status.success(), "cannot build the echo_server example");
        }
        path
    }

    fn lua_spec(name: &str, origin: ConfigOrigin) -> McpServerSpec {
        McpServerSpec {
            origin,
            ..McpServerSpec::new(
                name,
                McpTransport::Stdio {
                    command: "mcp-git".into(),
                    args: Vec::new(),
                    env: BTreeMap::new(),
                    cwd: None,
                },
            )
        }
    }

    struct Bench {
        dir: tempfile::TempDir,
        control: Arc<McpControl>,
    }

    impl Bench {
        fn config(dir: &tempfile::TempDir) -> McpConfigPaths {
            McpConfigPaths {
                global: Some(dir.path().join("config").join("mcp.json")),
                workspace: Some(dir.path().join("ws").join(".crest").join("mcp.json")),
            }
        }

        /// A control over a manager running `specs` (none when `None`).
        fn new(specs: Vec<McpServerSpec>, running: bool) -> Self {
            let dir = tempfile::tempdir().unwrap();
            // The workspace is the working directory of stdio servers.
            std::fs::create_dir_all(dir.path().join("ws")).unwrap();
            let servers = running.then(|| {
                let manager = Arc::new(McpManager::start(specs.clone()));
                let port: Arc<dyn McpPort> = Arc::clone(&manager) as Arc<dyn McpPort>;
                (manager, port)
            });
            let control = Arc::new(McpControl::new(
                specs,
                dir.path().join("ws"),
                Some(dir.path().join("data").join("mcp-trust.json")),
                Self::config(&dir),
                servers,
            ));
            Self { dir, control }
        }
    }

    #[tokio::test]
    async fn mcp_trust_saves_the_approval_or_explains_the_usage() {
        let bench = Bench::new(vec![lua_spec("git", ConfigOrigin::Workspace)], false);
        let run = |args: &str| Arc::clone(&bench.control).run(McpCommand::parse(args));
        assert!(matches!(
            run("trust git").await,
            Reply::Notice(NoticeLevel::Info, _)
        ));
        assert!(
            bench
                .dir
                .path()
                .join("data")
                .join("mcp-trust.json")
                .exists()
        );
        let Reply::Notice(NoticeLevel::Warning, usage) = run("frobnicate git").await else {
            panic!("a usage warning");
        };
        assert!(usage.contains("usage: /mcp"), "{usage}");
        let Reply::Notice(NoticeLevel::Info, none) = run("list").await else {
            panic!("a notice without servers");
        };
        assert!(none.contains("no MCP server"), "{none}");
        let Reply::Notice(NoticeLevel::Warning, nope) = run("remove git").await else {
            panic!("a warning without a manager");
        };
        assert!(nope.contains("no MCP servers"), "{nope}");
        let listing = bench.control.listing().await;
        assert!(!listing.can_edit);
        assert_eq!(listing.servers.len(), 1);
        assert_eq!(listing.servers[0].state, McpServerState::Stopped);
    }

    #[tokio::test]
    async fn save_writes_mcp_json_and_starts_the_server() {
        let bench = Bench::new(Vec::new(), true);
        let draft = McpDraft {
            name: "echo".into(),
            kind: McpKind::Stdio,
            scope: ConfigOrigin::Workspace,
            command: echo_server_path().display().to_string(),
            ..McpDraft::new()
        };
        let said = bench.control.save(None, draft.clone()).await.unwrap();
        assert!(said.contains("saved to .crest/mcp.json"), "{said}");
        let file = bench.dir.path().join("ws").join(".crest").join("mcp.json");
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains("\"echo\""), "{text}");
        let trust =
            std::fs::read_to_string(bench.dir.path().join("data").join("mcp-trust.json")).unwrap();
        assert!(
            trust.contains("echo_server"),
            "the workspace server is approved: {trust}"
        );
        let manager = bench.control.manager().unwrap();
        manager.wait_ready().await;
        let listing = bench.control.listing().await;
        assert!(listing.can_edit);
        assert_eq!(listing.servers.len(), 1);
        let echo = &listing.servers[0];
        assert_eq!(echo.state, McpServerState::Ready { tool_count: 2 });
        assert!(echo.editable());
        assert_eq!(echo.tools.len(), 2);
        assert!(echo.log.iter().any(|e| e.text.starts_with("connected")));
        assert_eq!(echo.saved_in, ".crest/mcp.json");
        assert!(matches!(
            bench.control.save(None, draft.clone()).await,
            Err(e) if e.contains("already")
        ));

        // Edited into the global file: it leaves the workspace one.
        let moved = McpDraft {
            scope: ConfigOrigin::Global,
            args: vec!["--delay-ms".into(), "100".into()],
            ..draft
        };
        bench
            .control
            .save(Some("echo".into()), moved)
            .await
            .unwrap();
        assert!(!std::fs::read_to_string(&file).unwrap().contains("\"echo\""));
        let global =
            std::fs::read_to_string(bench.dir.path().join("config").join("mcp.json")).unwrap();
        assert!(global.contains("--delay-ms"), "{global}");
        manager.wait_ready().await;
        assert_eq!(
            bench.control.listing().await.servers[0].spec.origin,
            ConfigOrigin::Global
        );

        let said = bench.control.act("echo", McpAction::Disable).await.unwrap();
        assert_eq!(said, "MCP server `echo` disabled");
        let global =
            std::fs::read_to_string(bench.dir.path().join("config").join("mcp.json")).unwrap();
        assert!(global.contains("\"disabled\": true"), "{global}");
        bench.control.act("echo", McpAction::Enable).await.unwrap();
        let global =
            std::fs::read_to_string(bench.dir.path().join("config").join("mcp.json")).unwrap();
        assert!(!global.contains("disabled"), "{global}");
        manager.wait_ready().await;

        let said = bench.control.act("echo", McpAction::Test).await.unwrap();
        assert!(said.starts_with("connected in"), "{said}");
        assert!(
            bench
                .control
                .act("echo", McpAction::CancelLogin)
                .await
                .is_err()
        );

        let said = bench.control.remove("echo").await.unwrap();
        assert!(
            said.contains("removed from ~/.config/crest/mcp.json"),
            "{said}"
        );
        assert!(bench.control.listing().await.servers.is_empty());
        assert!(
            !std::fs::read_to_string(bench.dir.path().join("config").join("mcp.json"))
                .unwrap()
                .contains("echo")
        );
    }

    #[tokio::test]
    async fn lua_servers_are_read_only_and_test_connects_a_draft() {
        let bench = Bench::new(vec![lua_spec("git", ConfigOrigin::Global)], true);
        let err = bench.control.remove("git").await.unwrap_err();
        assert!(err.contains("init.lua"), "{err}");
        let err = bench
            .control
            .save(
                Some("git".into()),
                McpDraft::from_spec(&lua_spec("git", ConfigOrigin::Global)),
            )
            .await
            .unwrap_err();
        assert!(err.contains("init.lua"), "{err}");
        let said = bench.control.act("git", McpAction::Disable).await.unwrap();
        assert!(said.contains("for this session"), "{said}");
        let draft = McpDraft {
            name: "probe".into(),
            command: echo_server_path().display().to_string(),
            ..McpDraft::new()
        };
        let said = bench.control.test(draft).await.unwrap();
        assert!(said.starts_with("connected in"), "{said}");
        assert!(bench.control.test(McpDraft::new()).await.is_err());
        let Reply::Notice(NoticeLevel::Warning, nope) = Arc::clone(&bench.control)
            .run(McpCommand::parse("nope"))
            .await
        else {
            panic!("a warning for an unknown name");
        };
        assert!(nope.contains("no MCP server `nope`"), "{nope}");
    }
}
