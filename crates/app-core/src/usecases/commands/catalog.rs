//! Every built-in `/` command, with its aliases, its usage and what it does:
//! the source of the help's Commands section and of the hint for an
//! unknown command. A command can have several rows (one per form of its
//! arguments); the first row of a name holds its aliases.

/// Who runs a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handler {
    /// The session in the window (`/model`, `/clear`, `/q`...).
    Ui,
    /// The session's controller, through [`super::dispatch`] (`/research`,
    /// `/implement`, `/!cmd`...).
    Controller,
}

/// The named group a built-in command belongs to in the palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandGroup {
    /// This conversation (`/clear`, `/open`, `!`, `/aside`).
    Chat,
    /// Research, implement, simulate.
    Flows,
    /// Steer the running flow.
    Stages,
    /// How the agent runs.
    ModelAndPermissions,
    /// Projects, MCP, plugins.
    Workspace,
    /// Help, theme, account, quit.
    App,
}

impl CommandGroup {
    /// Every group, in the palette's order.
    pub const ALL: [Self; 6] = [
        Self::Chat,
        Self::Flows,
        Self::Stages,
        Self::ModelAndPermissions,
        Self::Workspace,
        Self::App,
    ];

    /// The name the palette shows.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Chat => "Chat",
            Self::Flows => "Flows",
            Self::Stages => "Stages",
            Self::ModelAndPermissions => "Model & permissions",
            Self::Workspace => "Workspace",
            Self::App => "App",
        }
    }
}

/// A row of the help for a `/` command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandInfo {
    /// Name typed after the slash (`model`).
    pub name: &'static str,
    /// Other names that run it (`h` for `help`).
    pub aliases: &'static [&'static str],
    /// How to type it (`/model [alias]`).
    pub usage: &'static str,
    /// What it does.
    pub description: &'static str,
    /// Who runs it.
    pub handler: Handler,
    /// The palette group it is listed under.
    pub group: CommandGroup,
}

/// The built-in commands: the session's first, then the controller's.
pub const CATALOG: &[CommandInfo] = &[
    CommandInfo {
        name: "help",
        aliases: &["h"],
        usage: "/help [section|search]",
        description: "Open this help (also /h), on a section (commands, keys, tools, plugins, mcp, models, \
                      cli, files) or filtered by a search",
        handler: Handler::Ui,
        group: CommandGroup::App,
    },
    CommandInfo {
        name: "model",
        aliases: &[],
        usage: "/model [alias|set <id>|clear]",
        description: "Pick one of the models your providers let you use from the popup (Enter saves it, Tab \
                      completes); `/model <alias>` sent as typed sets the next turn's model, `set <id>` saves one, \
                      `clear` drops the next turn's",
        handler: Handler::Ui,
        group: CommandGroup::ModelAndPermissions,
    },
    CommandInfo {
        name: "theme",
        aliases: &[],
        usage: "/theme [set] [name]",
        description: "Pick a color theme from the popup: moving previews it, Enter saves it, Esc goes back; \
                      `/theme <name>` saves one directly",
        handler: Handler::Ui,
        group: CommandGroup::App,
    },
    CommandInfo {
        name: "mode",
        aliases: &[],
        usage: "/mode <ask|auto-edit|plan>",
        description: "Change the permission mode",
        handler: Handler::Ui,
        group: CommandGroup::ModelAndPermissions,
    },
    CommandInfo {
        name: "permissions",
        aliases: &[],
        usage: "/permissions",
        description: "List the permission rules of this session and project: d removes one, p moves a \
                      session rule to the project file",
        handler: Handler::Ui,
        group: CommandGroup::ModelAndPermissions,
    },
    CommandInfo {
        name: "clear",
        aliases: &[],
        usage: "/clear",
        description: "Start a new, empty session (not while a turn runs)",
        handler: Handler::Ui,
        group: CommandGroup::Chat,
    },
    CommandInfo {
        name: "project",
        aliases: &[],
        usage: "/project",
        description: "Close this project and choose another one",
        handler: Handler::Ui,
        group: CommandGroup::Workspace,
    },
    CommandInfo {
        name: "login",
        aliases: &[],
        usage: "/login [anthropic|opencode]",
        description: "Store an API key (Anthropic or OpenCode) in the system keyring and restart the agent",
        handler: Handler::Ui,
        group: CommandGroup::App,
    },
    CommandInfo {
        name: "logout",
        aliases: &[],
        usage: "/logout [anthropic|opencode]",
        description: "Delete the API key of one account (or of every account) from the system keyring",
        handler: Handler::Ui,
        group: CommandGroup::App,
    },
    CommandInfo {
        name: "picker.files",
        aliases: &[],
        usage: "/picker.files",
        description: "Find a file by name, with a preview; Enter inserts @path in the prompt",
        handler: Handler::Ui,
        group: CommandGroup::Workspace,
    },
    CommandInfo {
        name: "picker.grep",
        aliases: &[],
        usage: "/picker.grep",
        description: "Search text in the workspace (ripgrep), with a preview; Enter inserts @path:line \
                      in the prompt",
        handler: Handler::Ui,
        group: CommandGroup::Workspace,
    },
    CommandInfo {
        name: "modal",
        aliases: &[],
        usage: "/modal close",
        description: "Close the front modal window",
        handler: Handler::Ui,
        group: CommandGroup::App,
    },
    CommandInfo {
        name: "q",
        aliases: &["quit", "qa"],
        usage: "/q",
        description: "Quit crest (also /quit and /qa)",
        handler: Handler::Ui,
        group: CommandGroup::App,
    },
    CommandInfo {
        name: "!",
        aliases: &[],
        usage: "! <command>",
        description: "Run a shell command in the workspace; its output is shown and goes to \
                      the model with the next prompt (also /!cmd)",
        handler: Handler::Controller,
        group: CommandGroup::Chat,
    },
    CommandInfo {
        name: "implement",
        aliases: &[],
        usage: "/implement <spec>",
        description: "Run the plan of a Spec task by task, one commit per task, with gates you \
                      answer from the input",
        handler: Handler::Controller,
        group: CommandGroup::Flows,
    },
    CommandInfo {
        name: "implement",
        aliases: &[],
        usage: "/implement resume",
        description: "Continue the last implementation where it stopped (also after an error)",
        handler: Handler::Controller,
        group: CommandGroup::Flows,
    },
    CommandInfo {
        name: "implement",
        aliases: &[],
        usage: "/implement cancel",
        description: "Stop the active or pending implementation for good (its journal is kept)",
        handler: Handler::Controller,
        group: CommandGroup::Flows,
    },
    CommandInfo {
        name: "research",
        aliases: &[],
        usage: "/research <problem>",
        description: "Start the research flow on a problem (5 stages, depth picked by the \
                      triage); it ends in a Spec",
        handler: Handler::Controller,
        group: CommandGroup::Flows,
    },
    CommandInfo {
        name: "research",
        aliases: &[],
        usage: "/research --quick <problem>",
        description: "The 5-stage flow at quick depth: fewer explorers and a cheaper model \
                      (also -q)",
        handler: Handler::Controller,
        group: CommandGroup::Flows,
    },
    CommandInfo {
        name: "research",
        aliases: &[],
        usage: "/research --deep <problem>",
        description: "The full 14-stage flow, with experiments and a prototype (also -d)",
        handler: Handler::Controller,
        group: CommandGroup::Flows,
    },
    CommandInfo {
        name: "research",
        aliases: &[],
        usage: "/research",
        description: "Where the research flow is and what it cost so far",
        handler: Handler::Controller,
        group: CommandGroup::Flows,
    },
    CommandInfo {
        name: "research",
        aliases: &[],
        usage: "/research resume [n]",
        description: "Continue the last research flow (or number n of /research list), \
                      running a failed or interrupted stage again",
        handler: Handler::Controller,
        group: CommandGroup::Flows,
    },
    CommandInfo {
        name: "research",
        aliases: &[],
        usage: "/research list [all]",
        description: "The open research flows of this workspace, newest first, with their \
                      pipeline and cost (all: also the finished and cancelled ones)",
        handler: Handler::Controller,
        group: CommandGroup::Flows,
    },
    CommandInfo {
        name: "research",
        aliases: &[],
        usage: "/research cancel [n|all]",
        description: "Drop the last research flow (or number n of /research list, or every \
                      open one) for good; it is no longer pending",
        handler: Handler::Controller,
        group: CommandGroup::Flows,
    },
    CommandInfo {
        name: "research",
        aliases: &[],
        usage: "/research accept",
        description: "Discovery engine: freeze the discovery once every section is approved \
                      and derive spec.md and plan.json",
        handler: Handler::Controller,
        group: CommandGroup::Flows,
    },
    CommandInfo {
        name: "reopen",
        aliases: &[],
        usage: "/reopen <section> [reason]",
        description: "Discovery engine: reopen a section; it and every later one lose their \
                      verdicts and are written again",
        handler: Handler::Controller,
        group: CommandGroup::Stages,
    },
    CommandInfo {
        name: "retry",
        aliases: &[],
        usage: "/retry",
        description: "Run the failed stage of the research flow again",
        handler: Handler::Controller,
        group: CommandGroup::Stages,
    },
    CommandInfo {
        name: "stage",
        aliases: &[],
        usage: "/stage",
        description: "Where the active flow is (research or implementation)",
        handler: Handler::Controller,
        group: CommandGroup::Stages,
    },
    CommandInfo {
        name: "back",
        aliases: &[],
        usage: "/back <stage>",
        description: "Take the research flow back to a stage, by number or name",
        handler: Handler::Controller,
        group: CommandGroup::Stages,
    },
    CommandInfo {
        name: "skip",
        aliases: &[],
        usage: "/skip",
        description: "Skip an optional stage of the research flow",
        handler: Handler::Controller,
        group: CommandGroup::Stages,
    },
    CommandInfo {
        name: "abort",
        aliases: &[],
        usage: "/abort",
        description: "Stop the active flow for good (its journal is kept)",
        handler: Handler::Controller,
        group: CommandGroup::Stages,
    },
    CommandInfo {
        name: "aside",
        aliases: &["aparte"],
        usage: "/aside [question]",
        description: "Chat aside while a flow waits at a gate: lines go to the chat, with the \
                      gate as context, until /flow (also /aparte)",
        handler: Handler::Controller,
        group: CommandGroup::Chat,
    },
    CommandInfo {
        name: "flow",
        aliases: &["volver"],
        usage: "/flow [answer]",
        description: "Back to the waiting flow: answers it, or proposes the answer from the \
                      side chat for you to confirm (also /volver)",
        handler: Handler::Controller,
        group: CommandGroup::Stages,
    },
    CommandInfo {
        name: "open",
        aliases: &[],
        usage: "/open @<path>",
        description: "Show a workspace file in a modal, 3D models included (type @ to complete the path)",
        handler: Handler::Controller,
        group: CommandGroup::Chat,
    },
    CommandInfo {
        name: "simulate",
        aliases: &[],
        usage: "/simulate <spec>",
        description: "Simulate the solution of a Spec and open the 3D board with every scenario",
        handler: Handler::Controller,
        group: CommandGroup::Flows,
    },
    CommandInfo {
        name: "mcp",
        aliases: &[],
        usage: "/mcp [tools|restart|enable|disable|login|logout|trust <name>]",
        description: "List the MCP servers with their state, or show a server's tools, \
                      restart, turn on or off, log in or out (OAuth) or approve one of the \
                      workspace config",
        handler: Handler::Controller,
        group: CommandGroup::Workspace,
    },
    CommandInfo {
        name: "plugin",
        aliases: &[],
        usage: "/plugin list",
        description: "List the plugins that declare capabilities and whether they are approved",
        handler: Handler::Controller,
        group: CommandGroup::Workspace,
    },
    CommandInfo {
        name: "plugin",
        aliases: &[],
        usage: "/plugin trust <name>",
        description: "Approve the capabilities (network, shell) a plugin declares",
        handler: Handler::Controller,
        group: CommandGroup::Workspace,
    },
];

/// The rows of the commands `handler` runs, in catalog order.
pub fn commands_of(handler: Handler) -> impl Iterator<Item = &'static CommandInfo> {
    CATALOG.iter().filter(move |info| info.handler == handler)
}

/// The status-bar hint for `/command`, which nothing runs: it names the
/// session's own commands.
#[must_use]
pub fn unknown_command_hint(command: &str) -> String {
    let names: Vec<String> = commands_of(Handler::Ui)
        .map(|info| format!("/{}", info.name))
        .collect();
    format!(
        "unknown command /{command} (use {}; /help lists every command)",
        names.join(", ")
    )
}
