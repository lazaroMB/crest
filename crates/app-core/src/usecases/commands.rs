//! The `/` commands: how a line reads ([`parse`], [`Command`]), which ones
//! there are ([`catalog::CATALOG`]) and where each goes ([`dispatch`] over a
//! [`CommandHandler`]).
//!
//! The window's session parses every line here and runs its own commands
//! (`/model`, `/clear`, `/q`...); the rest goes to its controller, which
//! hands it to [`dispatch`]. The headless research loop reads `/aside` and
//! `/flow` with the same parse. Adding a command means a [`Command`]
//! variant, its row in the catalog and, for a controller command, a
//! [`CommandHandler`] method.

pub mod catalog;
pub mod research;

use domain::block::Block;
use domain::permission::{ParsePermissionModeError, PermissionMode};
use futures::FutureExt as _;
use futures::future::BoxFuture;

use crate::ports::events::NoticeLevel;
use crate::ports::plugin::PluginAction;

pub use catalog::{CATALOG, CommandGroup, CommandInfo, Handler, commands_of, unknown_command_hint};
pub use research::{ResearchCommand, StartMode};

/// Usage of `/!`, shown when it has no command.
pub const SHELL_USAGE: &str = "usage: /!<command> [args], e.g. /!git status";
/// Usage of `/simulate`, shown when it has no Spec.
pub const SIMULATE_USAGE: &str =
    "usage: /simulate <spec> (a folder of .crest/specs/ or its number)";
/// Usage of `/mcp`.
pub const MCP_USAGE: &str = "usage: /mcp [list] | /mcp tools <name> | /mcp restart|enable|disable \
                             <name> | /mcp login|logout <name> | /mcp trust <name>";
/// Usage of `/plugin`.
pub const PLUGIN_USAGE: &str = "usage: /plugin list | /plugin trust <name>";

/// A `/` line split in its name and its argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandLine<'a> {
    /// The first word (`model`), as typed: aliases are not resolved.
    pub name: &'a str,
    /// The rest, trimmed.
    pub arg: &'a str,
}

/// Splits `command` (a `/` line without the slash) in its name and its
/// argument. `!cmd args` and `! cmd args` are the shell command `!`.
#[must_use]
pub fn parse(command: &str) -> CommandLine<'_> {
    let (name, arg) = match command.strip_prefix('!') {
        Some(line) => ("!", line),
        None => command.split_once(' ').unwrap_or((command, "")),
    };
    CommandLine {
        name,
        arg: arg.trim(),
    }
}

/// A `/` command, read: the session's own, the controller's, or another
/// one ([`Command::Other`]: a command of `init.lua` or a builtin action
/// with a dot, `/picker.files`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// `/q` (also `/quit`, `/qa`).
    Quit,
    /// `/help [section|search]` (also `/h`).
    Help(String),
    /// `/model ...`.
    Model(ModelCommand),
    /// `/theme ...`.
    Theme(ThemeCommand),
    /// `/mode <mode>`, or why the mode is not one.
    Mode(Result<PermissionMode, ParsePermissionModeError>),
    /// `/permissions`.
    Permissions,
    /// `/clear`.
    Clear,
    /// `/project`.
    Project,
    /// `/login [account]`.
    Login(String),
    /// `/logout [account]`.
    Logout(String),
    /// `/modal close`.
    ModalClose,
    /// `/modal <anything else>`: what was typed after it.
    Modal(String),
    /// `/implement ...`.
    Implement(ImplementCommand),
    /// `/research ...`.
    Research(ResearchCommand),
    /// `/aside [question]` (also `/aparte`).
    Aside(String),
    /// `/flow [answer]` (also `/volver`).
    Flow(String),
    /// `/stage`, `/back`, `/skip`, `/abort`, `/retry` or `/reopen`: the
    /// line for the active flow, slash included (`/back 3`).
    FlowLine(String),
    /// `/!command`: the command line, empty when none was typed.
    Shell(String),
    /// `/simulate <spec>`: the Spec, empty when none was typed.
    Simulate(String),
    /// `/mcp ...`.
    Mcp(McpCommand),
    /// `/plugin ...`.
    Plugin(PluginCommand),
    /// `/open @path`: the argument.
    Open(String),
    /// Any other name: a command of `init.lua`, or a builtin action
    /// (`/picker.files`).
    Other {
        /// The name, as typed.
        name: String,
        /// The argument, trimmed.
        arg: String,
    },
}

impl Command {
    /// Reads `command` (a `/` line without the slash).
    #[must_use]
    pub fn parse(command: &str) -> Self {
        Self::from_line(parse(command))
    }

    /// The command `line` names, its aliases resolved.
    #[must_use]
    pub fn from_line(line: CommandLine<'_>) -> Self {
        let CommandLine { name, arg } = line;
        match name {
            "q" | "quit" | "qa" => Self::Quit,
            "help" | "h" => Self::Help(arg.to_owned()),
            "model" => Self::Model(ModelCommand::parse(arg)),
            "theme" => Self::Theme(ThemeCommand::parse(arg)),
            "mode" => Self::Mode(arg.parse()),
            "permissions" => Self::Permissions,
            "clear" => Self::Clear,
            "project" => Self::Project,
            "login" => Self::Login(arg.to_owned()),
            "logout" => Self::Logout(arg.to_owned()),
            "modal" if arg == "close" => Self::ModalClose,
            "modal" => Self::Modal(arg.to_owned()),
            "implement" => Self::Implement(ImplementCommand::parse(arg)),
            "research" => Self::Research(ResearchCommand::parse(arg)),
            "aside" | "aparte" => Self::Aside(arg.to_owned()),
            "flow" | "volver" => Self::Flow(arg.to_owned()),
            "stage" | "back" | "skip" | "abort" | "retry" | "reopen" => {
                Self::FlowLine(format!("/{name} {arg}").trim_end().to_owned())
            }
            "!" => Self::Shell(arg.to_owned()),
            "simulate" => Self::Simulate(arg.to_owned()),
            "mcp" => Self::Mcp(McpCommand::parse(arg)),
            "plugin" => Self::Plugin(PluginCommand::parse(arg)),
            "open" => Self::Open(arg.to_owned()),
            _ => Self::Other {
                name: name.to_owned(),
                arg: arg.to_owned(),
            },
        }
    }
}

/// Splits `arg` at its first space, the rest trimmed.
fn first_word(arg: &str) -> (&str, &str) {
    arg.split_once(' ')
        .map_or((arg, ""), |(a, b)| (a, b.trim()))
}

/// What `/model <args>` asks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelCommand {
    /// `/model`: the popup of models.
    Popup,
    /// `/model clear`: drops the next turn's model.
    Clear,
    /// `/model set <id>`: saves `id` as the model.
    Set(String),
    /// `/model <alias>`: the model of the next turn only.
    Override(String),
    /// A form it does not take: its usage.
    Invalid(&'static str),
}

impl ModelCommand {
    /// Parses the arguments of `/model`.
    #[must_use]
    pub fn parse(arg: &str) -> Self {
        match first_word(arg) {
            ("", _) => Self::Popup,
            ("clear", "") => Self::Clear,
            ("set", "") => Self::Invalid("usage: /model set <id>"),
            ("set", id) => Self::Set(id.to_owned()),
            _ => Self::Override(arg.to_owned()),
        }
    }
}

/// What `/theme <args>` asks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThemeCommand {
    /// `/theme`: the popup of themes.
    Popup,
    /// `/theme [set] <name>`: saves the theme `name`.
    Save(String),
    /// A form it does not take: its usage.
    Invalid(&'static str),
}

impl ThemeCommand {
    /// Parses the arguments of `/theme`.
    #[must_use]
    pub fn parse(arg: &str) -> Self {
        match first_word(arg) {
            ("", _) => Self::Popup,
            ("set", "") => Self::Invalid("usage: /theme set <name>"),
            ("set", name) | (name, "") => Self::Save(name.to_owned()),
            _ => Self::Invalid("usage: /theme [set] <name>"),
        }
    }
}

/// What `/implement <args>` asks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImplementCommand {
    /// `/implement <spec>`: opens the flow of a Spec (a folder, its number
    /// or a `plan.json`).
    Start(String),
    /// `/implement [resume]`: continues the pending flow of the workspace.
    Resume,
    /// `/implement cancel`: stops the active or pending flow for good.
    Cancel,
}

impl ImplementCommand {
    /// Parses the arguments of `/implement`.
    #[must_use]
    pub fn parse(arg: &str) -> Self {
        match arg.trim() {
            "cancel" | "cancelar" | "descartar" | "drop" => Self::Cancel,
            "" | "resume" | "retry" | "continue" | "retomar" | "reanudar" => Self::Resume,
            target => Self::Start(target.to_owned()),
        }
    }
}

/// What `/mcp <name>` does to a server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpVerb {
    /// `restart`.
    Restart,
    /// `enable`.
    Enable,
    /// `disable`.
    Disable,
    /// `login` (OAuth).
    Login,
    /// `logout`: deletes the stored tokens.
    Logout,
}

impl McpVerb {
    /// The word typed for it.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Restart => "restart",
            Self::Enable => "enable",
            Self::Disable => "disable",
            Self::Login => "login",
            Self::Logout => "logout",
        }
    }
}

/// What `/mcp <args>` asks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpCommand {
    /// `/mcp [list]`: every server with its state.
    List,
    /// `/mcp tools <name>`.
    Tools(String),
    /// `/mcp trust <name>`: approves a server of the workspace config.
    Trust(String),
    /// `/mcp restart|enable|disable|login|logout <name>`.
    Manage {
        /// What to do.
        verb: McpVerb,
        /// The server.
        name: String,
    },
    /// A form it does not take ([`MCP_USAGE`]).
    Invalid,
}

impl McpCommand {
    /// Parses the arguments of `/mcp`.
    #[must_use]
    pub fn parse(arg: &str) -> Self {
        let words: Vec<&str> = arg.split_whitespace().collect();
        let verb = |word: &str| match word {
            "restart" => Some(McpVerb::Restart),
            "enable" => Some(McpVerb::Enable),
            "disable" => Some(McpVerb::Disable),
            "login" => Some(McpVerb::Login),
            "logout" => Some(McpVerb::Logout),
            _ => None,
        };
        match words.as_slice() {
            [] | ["list"] => Self::List,
            ["tools", name] => Self::Tools((*name).to_owned()),
            ["trust", name] => Self::Trust((*name).to_owned()),
            [word, name] => match verb(word) {
                Some(verb) => Self::Manage {
                    verb,
                    name: (*name).to_owned(),
                },
                None => Self::Invalid,
            },
            _ => Self::Invalid,
        }
    }
}

/// What `/plugin <args>` asks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginCommand {
    /// `/plugin list`.
    List,
    /// `/plugin trust <name>`: approves a plugin's capabilities.
    Trust(String),
    /// A form it does not take ([`PLUGIN_USAGE`]).
    Invalid,
}

impl PluginCommand {
    /// Parses the arguments of `/plugin`.
    #[must_use]
    pub fn parse(arg: &str) -> Self {
        let words: Vec<&str> = arg.split_whitespace().collect();
        match words.as_slice() {
            ["list"] => Self::List,
            ["trust", name] => Self::Trust((*name).to_owned()),
            _ => Self::Invalid,
        }
    }
}

/// What a command or a Lua callback run by the controller asks the
/// session to do once it finished.
#[derive(Debug, Clone, PartialEq)]
pub enum Reply {
    /// Nothing to show.
    Done,
    /// A message for the status bar.
    Notice(NoticeLevel, String),
    /// A block for the timeline (boxed: blocks are large).
    Block(Box<Block>),
    /// Actions requested by Lua (`app.cmd`, `app.send`), in order.
    Actions(Vec<PluginAction>),
}

impl Reply {
    /// A reply that shows `block`.
    #[must_use]
    pub fn block(block: Block) -> Self {
        Self::Block(Box::new(block))
    }
}

/// A future [`Reply`] that the session polls on its own executor.
pub type ReplyFuture = BoxFuture<'static, Reply>;

/// What runs the controller's commands, one method per command. Each
/// returns a future that owns what it needs, so the controller can run it
/// on its runtime and cancel it (`Ctrl+C`) while the session waits.
pub trait CommandHandler {
    /// `/implement ...`.
    fn implement(&self, command: ImplementCommand) -> ReplyFuture;
    /// `/research ...`.
    fn research(&self, command: ResearchCommand) -> ReplyFuture;
    /// `/aside [question]`.
    fn aside(&self, question: String) -> ReplyFuture;
    /// `/flow [answer]`.
    fn flow(&self, answer: String) -> ReplyFuture;
    /// `/stage`, `/back`...: `line` for the active flow.
    fn flow_line(&self, line: String) -> ReplyFuture;
    /// `/!command`, never empty.
    fn shell(&self, command: String) -> ReplyFuture;
    /// `/simulate <spec>`, never empty.
    fn simulate(&self, spec: String) -> ReplyFuture;
    /// `/mcp ...`.
    fn mcp(&self, command: McpCommand) -> ReplyFuture;
    /// `/plugin ...`.
    fn plugin(&self, command: PluginCommand) -> ReplyFuture;
    /// `/open @path`.
    fn open(&self, arg: String) -> ReplyFuture;
    /// Any other command: the one of `init.lua` called `name`, if there is
    /// one; `None` otherwise.
    fn callback(&self, name: &str, arg: String) -> Option<ReplyFuture>;
}

/// Runs `command` on `handler`; `None` when the handler has no such
/// command (the session's own commands, or an unknown name).
pub fn dispatch<H: CommandHandler + ?Sized>(handler: &H, command: Command) -> Option<ReplyFuture> {
    let usage = |text: &str| {
        futures::future::ready(Reply::Notice(NoticeLevel::Warning, text.into())).boxed()
    };
    Some(match command {
        Command::Implement(command) => handler.implement(command),
        Command::Research(command) => handler.research(command),
        Command::Aside(question) => handler.aside(question),
        Command::Flow(answer) => handler.flow(answer),
        Command::FlowLine(line) => handler.flow_line(line),
        Command::Shell(line) if line.trim().is_empty() => usage(SHELL_USAGE),
        Command::Shell(line) => handler.shell(line),
        Command::Simulate(spec) if spec.trim().is_empty() => usage(SIMULATE_USAGE),
        Command::Simulate(spec) => handler.simulate(spec.trim().to_owned()),
        Command::Mcp(command) => handler.mcp(command),
        Command::Plugin(command) => handler.plugin(command),
        Command::Open(arg) => handler.open(arg),
        Command::Other { name, arg } => return handler.callback(&name, arg),
        Command::Quit
        | Command::Help(_)
        | Command::Model(_)
        | Command::Theme(_)
        | Command::Mode(_)
        | Command::Permissions
        | Command::Clear
        | Command::Project
        | Command::Login(_)
        | Command::Logout(_)
        | Command::ModalClose
        | Command::Modal(_) => return None,
    })
}

#[cfg(test)]
mod tests;
