//! What the MCP servers manager shows, computed from the listing: the
//! rows of the list, the tone of each state, the summary of the sidebar
//! dock, the masking of secrets and the templates of the form. Pure, so
//! it is tested without a window.

use std::time::SystemTime;

use application_core::ports::mcp::McpServerState;
use application_core::ports::plugin::ConfigOrigin;
use application_core::usecases::mcp_servers::{McpDraft, McpKind, McpListing, McpServerInfo};
use domain::permission::ToolEffect;

/// The group a server is listed under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    /// `~/.config/crest`: every project.
    Global,
    /// `<workspace>/.crest`: this one.
    Workspace,
}

impl Group {
    /// The group's title.
    #[must_use]
    pub fn title(self) -> &'static str {
        match self {
            Self::Global => "Global",
            Self::Workspace => "This workspace",
        }
    }

    fn of(origin: ConfigOrigin) -> Self {
        match origin {
            ConfigOrigin::Global => Self::Global,
            ConfigOrigin::Workspace => Self::Workspace,
        }
    }
}

/// One row of the list: its group and the server's index in the listing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerRow {
    /// Its group.
    pub group: Group,
    /// Index into [`McpListing::servers`].
    pub index: usize,
}

/// The rows of `listing` whose name contains `query` (ignoring case),
/// global ones first, each group in config order.
#[must_use]
pub fn server_rows(listing: &McpListing, query: &str) -> Vec<ServerRow> {
    let query = query.trim().to_lowercase();
    let mut rows: Vec<ServerRow> = listing
        .servers
        .iter()
        .enumerate()
        .filter(|(_, s)| query.is_empty() || s.spec.name.to_lowercase().contains(&query))
        .map(|(index, s)| ServerRow {
            group: Group::of(s.spec.origin),
            index,
        })
        .collect();
    rows.sort_by_key(|row| (row.group == Group::Workspace, row.index));
    rows
}

/// How a state is colored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// Ready.
    Ok,
    /// Needs the user (a login, a trust).
    Warn,
    /// Failed.
    Error,
    /// Starting.
    Busy,
    /// Disabled or stopped.
    Off,
}

/// The tone of `state`.
#[must_use]
pub fn tone(state: &McpServerState) -> Tone {
    match state {
        McpServerState::Ready { .. } => Tone::Ok,
        McpServerState::NeedsLogin { .. } | McpServerState::Untrusted => Tone::Warn,
        McpServerState::Failed { .. } => Tone::Error,
        McpServerState::Starting => Tone::Busy,
        McpServerState::Disabled | McpServerState::Stopped => Tone::Off,
    }
}

/// The state as the detail's head says it (`ready · 8 tools`, `failed`).
#[must_use]
pub fn state_text(state: &McpServerState) -> String {
    match state {
        McpServerState::Ready { tool_count } => format!("ready · {tool_count} tools"),
        McpServerState::NeedsLogin { .. } => "needs login".to_owned(),
        McpServerState::Untrusted => "untrusted".to_owned(),
        other => other.label(),
    }
}

/// What the list's row says after the name: the tool count when ready,
/// the state when it needs attention, nothing otherwise.
#[must_use]
pub fn row_meta(state: &McpServerState) -> String {
    match state {
        McpServerState::Ready { tool_count } => tool_count.to_string(),
        McpServerState::Starting => "starting".to_owned(),
        McpServerState::NeedsLogin { .. } => "needs login".to_owned(),
        McpServerState::Untrusted => "untrusted".to_owned(),
        McpServerState::Failed { .. } => "failed".to_owned(),
        McpServerState::Disabled => "disabled".to_owned(),
        McpServerState::Stopped => "stopped".to_owned(),
    }
}

/// The servers in a few words, for the sidebar dock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct McpSummary {
    /// How many servers there are.
    pub total: usize,
    /// Ready ones.
    pub ready: usize,
    /// Starting ones.
    pub starting: usize,
    /// Ones that need a login or a trust.
    pub need_you: usize,
    /// Failed ones.
    pub failed: usize,
}

impl McpSummary {
    /// Counts the servers of `listing`.
    #[must_use]
    pub fn of(servers: &[McpServerInfo]) -> Self {
        let mut summary = Self {
            total: servers.len(),
            ..Self::default()
        };
        for server in servers {
            match tone(&server.state) {
                Tone::Ok => summary.ready += 1,
                Tone::Busy => summary.starting += 1,
                Tone::Warn => summary.need_you += 1,
                Tone::Error => summary.failed += 1,
                Tone::Off => {}
            }
        }
        summary
    }

    /// The worst state decides the dock's dot.
    #[must_use]
    pub fn tone(&self) -> Tone {
        if self.failed > 0 {
            Tone::Error
        } else if self.need_you > 0 {
            Tone::Warn
        } else if self.starting > 0 {
            Tone::Busy
        } else if self.ready > 0 {
            Tone::Ok
        } else {
            Tone::Off
        }
    }

    /// The parts of the status line, each with its tone: `2 ok`, `1 need
    /// you`, `1 failed`. Empty when there is nothing to say.
    #[must_use]
    pub fn parts(&self) -> Vec<(String, Tone)> {
        let mut parts = Vec::new();
        if self.ready > 0 {
            parts.push((format!("{} ok", self.ready), Tone::Ok));
        }
        if self.starting > 0 {
            parts.push((format!("{} starting", self.starting), Tone::Busy));
        }
        if self.need_you > 0 {
            parts.push((format!("{} need you", self.need_you), Tone::Warn));
        }
        if self.failed > 0 {
            parts.push((format!("{} failed", self.failed), Tone::Error));
        }
        parts
    }

    /// The status line as one text.
    #[must_use]
    pub fn text(&self) -> String {
        let parts = self.parts();
        if parts.is_empty() {
            return if self.total == 0 {
                "none yet · add one".to_owned()
            } else {
                format!("{} off", self.total)
            };
        }
        parts
            .into_iter()
            .map(|(text, _)| text)
            .collect::<Vec<_>>()
            .join(" · ")
    }
}

/// Whether the value of `key` is a secret to mask: it names a key, token,
/// secret, password or authorization and is not a `${VAR}` reference.
#[must_use]
pub fn is_secret(key: &str, value: &str) -> bool {
    if value.starts_with("${") {
        return false;
    }
    let key = key.to_lowercase();
    ["key", "token", "secret", "pass", "auth"]
        .iter()
        .any(|word| key.contains(word))
}

/// `value` hidden behind dots.
#[must_use]
pub fn mask(value: &str) -> String {
    "•".repeat(value.chars().count().clamp(8, 12))
}

/// How long ago `at` was, in a few words.
#[must_use]
pub fn ago(at: SystemTime) -> String {
    let secs = SystemTime::now()
        .duration_since(at)
        .map_or(0, |d| d.as_secs());
    if secs < 60 {
        "just now".to_owned()
    } else if secs < 3600 {
        format!("{} min ago", secs / 60)
    } else if secs < 86_400 {
        format!("{} h ago", secs / 3600)
    } else {
        format!("{} d ago", secs / 86_400)
    }
}

/// The tabs of a server's detail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum McpTab {
    /// Its config.
    #[default]
    Overview,
    /// Its tools.
    Tools,
    /// Its log.
    Log,
}

impl McpTab {
    /// Every tab, in order.
    pub const ALL: [Self; 3] = [Self::Overview, Self::Tools, Self::Log];

    /// The tab after this one, wrapping around.
    #[must_use]
    pub fn next(self) -> Self {
        let at = Self::ALL.iter().position(|t| *t == self).unwrap_or(0);
        Self::ALL[(at + 1) % Self::ALL.len()]
    }

    /// The tab before this one, wrapping around.
    #[must_use]
    pub fn prev(self) -> Self {
        let at = Self::ALL.iter().position(|t| *t == self).unwrap_or(0);
        Self::ALL[(at + Self::ALL.len() - 1) % Self::ALL.len()]
    }

    /// The tab's name.
    #[must_use]
    pub fn title(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Tools => "Tools",
            Self::Log => "Log",
        }
    }

    /// The tab's name in selectors.
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Self::Overview => "overview",
            Self::Tools => "tools",
            Self::Log => "log",
        }
    }
}

/// A server the add form can start from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Template {
    /// Its label.
    pub label: &'static str,
    name: &'static str,
    kind: McpKind,
    command: &'static str,
    args: &'static [&'static str],
    url: &'static str,
    oauth: bool,
    effect: ToolEffect,
}

impl Template {
    /// A draft from this template, keeping the scope of `draft`.
    #[must_use]
    pub fn apply(&self, draft: &McpDraft) -> McpDraft {
        let mut next = McpDraft {
            name: self.name.to_owned(),
            kind: self.kind,
            scope: draft.scope,
            command: self.command.to_owned(),
            args: self.args.iter().map(|a| (*a).to_owned()).collect(),
            url: self.url.to_owned(),
            effect: self.effect,
            ..McpDraft::new()
        };
        if self.oauth {
            next.oauth = Some(Default::default());
        }
        next
    }
}

/// The templates of the add form.
pub const TEMPLATES: &[Template] = &[
    Template {
        label: "Filesystem",
        name: "filesystem",
        kind: McpKind::Stdio,
        command: "npx",
        args: &["-y", "@modelcontextprotocol/server-filesystem", "."],
        url: "",
        oauth: false,
        effect: ToolEffect::Write,
    },
    Template {
        label: "GitHub",
        name: "github",
        kind: McpKind::Http,
        command: "",
        args: &[],
        url: "https://api.githubcopilot.com/mcp/",
        oauth: true,
        effect: ToolEffect::Network,
    },
    Template {
        label: "Fetch",
        name: "fetch",
        kind: McpKind::Stdio,
        command: "uvx",
        args: &["mcp-server-fetch"],
        url: "",
        oauth: false,
        effect: ToolEffect::Network,
    },
    Template {
        label: "Memory",
        name: "memory",
        kind: McpKind::Stdio,
        command: "npx",
        args: &["-y", "@modelcontextprotocol/server-memory"],
        url: "",
        oauth: false,
        effect: ToolEffect::Write,
    },
];

/// The effects the form offers, with a hint each.
pub const EFFECTS: [(ToolEffect, &str, &str); 4] = [
    (ToolEffect::Read, "read", "only reads"),
    (ToolEffect::Write, "write", "changes files or data"),
    (ToolEffect::Execute, "execute", "runs things (default)"),
    (ToolEffect::Network, "network", "talks to the internet"),
];

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::time::Duration;

    use application_core::ports::plugin::{McpServerSpec, McpSource, McpTransport};

    use super::*;

    fn server(name: &str, origin: ConfigOrigin, state: McpServerState) -> McpServerInfo {
        let mut spec = McpServerSpec::new(
            name,
            McpTransport::Stdio {
                command: "x".into(),
                args: Vec::new(),
                env: BTreeMap::new(),
                cwd: None,
            },
        );
        spec.origin = origin;
        spec.source = McpSource::Json;
        McpServerInfo::new(spec, state)
    }

    fn listing() -> McpListing {
        McpListing {
            servers: vec![
                server(
                    "ws-first",
                    ConfigOrigin::Workspace,
                    McpServerState::Starting,
                ),
                server(
                    "github",
                    ConfigOrigin::Global,
                    McpServerState::Ready { tool_count: 8 },
                ),
                server(
                    "linear",
                    ConfigOrigin::Global,
                    McpServerState::NeedsLogin {
                        message: "401".into(),
                    },
                ),
                server(
                    "postgres",
                    ConfigOrigin::Workspace,
                    McpServerState::Failed {
                        message: "boom".into(),
                    },
                ),
                server("browser", ConfigOrigin::Global, McpServerState::Disabled),
            ],
            can_edit: true,
            global_file: None,
            workspace_file: None,
        }
    }

    #[test]
    fn rows_group_global_then_workspace_and_filter_by_name() {
        let listing = listing();
        let rows = server_rows(&listing, "");
        let names: Vec<_> = rows
            .iter()
            .map(|r| (r.group, listing.servers[r.index].spec.name.as_str()))
            .collect();
        assert_eq!(
            names,
            [
                (Group::Global, "github"),
                (Group::Global, "linear"),
                (Group::Global, "browser"),
                (Group::Workspace, "ws-first"),
                (Group::Workspace, "postgres"),
            ]
        );
        let rows = server_rows(&listing, " GIT ");
        assert_eq!(rows.len(), 1);
        assert_eq!(listing.servers[rows[0].index].spec.name, "github");
        assert!(server_rows(&listing, "nothing").is_empty());
    }

    #[test]
    fn the_summary_picks_the_worst_state_and_words_it() {
        let listing = listing();
        let summary = McpSummary::of(&listing.servers);
        assert_eq!(
            summary,
            McpSummary {
                total: 5,
                ready: 1,
                starting: 1,
                need_you: 1,
                failed: 1,
            }
        );
        assert_eq!(summary.tone(), Tone::Error);
        assert_eq!(summary.text(), "1 ok · 1 starting · 1 need you · 1 failed");
        let calm = McpSummary {
            total: 2,
            ready: 2,
            ..McpSummary::default()
        };
        assert_eq!(calm.tone(), Tone::Ok);
        assert_eq!(calm.text(), "2 ok");
        assert_eq!(McpSummary::default().tone(), Tone::Off);
        assert_eq!(McpSummary::default().text(), "none yet · add one");
        let off = McpSummary {
            total: 1,
            ..McpSummary::default()
        };
        assert_eq!(off.text(), "1 off");
        let waiting = McpSummary {
            total: 2,
            starting: 1,
            need_you: 1,
            ..McpSummary::default()
        };
        assert_eq!(waiting.tone(), Tone::Warn);
    }

    #[test]
    fn states_have_a_tone_a_text_and_a_row_meta() {
        let ready = McpServerState::Ready { tool_count: 3 };
        assert_eq!(tone(&ready), Tone::Ok);
        assert_eq!(state_text(&ready), "ready · 3 tools");
        assert_eq!(row_meta(&ready), "3");
        assert_eq!(tone(&McpServerState::Untrusted), Tone::Warn);
        assert_eq!(state_text(&McpServerState::Untrusted), "untrusted");
        assert_eq!(row_meta(&McpServerState::Stopped), "stopped");
        assert_eq!(state_text(&McpServerState::Disabled), "disabled");
    }

    #[test]
    fn secrets_are_masked_unless_they_are_variables() {
        assert!(is_secret("API_KEY", "abc"));
        assert!(is_secret("Authorization", "Bearer x"));
        assert!(!is_secret("TOKEN", "${TOKEN}"));
        assert!(!is_secret("PGURL", "postgres://"));
        assert_eq!(mask("abc").chars().count(), 8);
        assert_eq!(mask("a".repeat(40).as_str()).chars().count(), 12);
    }

    #[test]
    fn ago_formats_seconds_minutes_hours_and_days() {
        let now = SystemTime::now();
        assert_eq!(ago(now), "just now");
        assert_eq!(ago(now - Duration::from_secs(120)), "2 min ago");
        assert_eq!(ago(now - Duration::from_secs(7200)), "2 h ago");
        assert_eq!(ago(now - Duration::from_secs(3 * 86_400)), "3 d ago");
    }

    #[test]
    fn tabs_cycle_and_wrap_and_templates_fill_a_draft() {
        assert_eq!(McpTab::Overview.next(), McpTab::Tools);
        assert_eq!(McpTab::Log.next(), McpTab::Overview);
        assert_eq!(McpTab::Overview.prev(), McpTab::Log);
        assert_eq!(McpTab::Tools.key(), "tools");
        let mut draft = McpDraft::new();
        draft.scope = ConfigOrigin::Workspace;
        let github = TEMPLATES[1].apply(&draft);
        assert_eq!(github.name, "github");
        assert_eq!(github.kind, McpKind::Http);
        assert!(github.oauth.is_some());
        assert_eq!(github.scope, ConfigOrigin::Workspace, "the scope stays");
        let fs = TEMPLATES[0].apply(&draft);
        assert_eq!(fs.command, "npx");
        assert_eq!(fs.args.len(), 3);
        assert_eq!(fs.effect, ToolEffect::Write);
        assert_eq!(EFFECTS[2].0, ToolEffect::Execute);
    }
}
