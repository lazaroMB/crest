//! The `/` commands the session runs itself, with their usage and what
//! they do: the source of the help's Commands section and of the hint for
//! an unknown command.

/// A `/` command, for the help.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandInfo {
    /// Name typed after the slash (`model`).
    pub name: &'static str,
    /// How to type it (`/model [alias]`).
    pub usage: &'static str,
    /// What it does.
    pub description: &'static str,
}

/// The commands [`Session`](crate::session::Session) handles.
pub const BUILTIN_COMMANDS: &[CommandInfo] = &[
    CommandInfo {
        name: "help",
        usage: "/help [section|search]",
        description: "Open this help (also /h), on a section (commands, keys, tools, plugins, mcp, models, \
                      cli, files) or filtered by a search",
    },
    CommandInfo {
        name: "model",
        usage: "/model [alias]",
        description: "Model of the next turn only (alias, role or id); no argument clears it",
    },
    CommandInfo {
        name: "mode",
        usage: "/mode <ask|auto-edit|plan>",
        description: "Change the permission mode",
    },
    CommandInfo {
        name: "clear",
        usage: "/clear",
        description: "Start a new, empty session (not while a turn runs)",
    },
    CommandInfo {
        name: "project",
        usage: "/project",
        description: "Close this project and choose another one",
    },
    CommandInfo {
        name: "login",
        usage: "/login",
        description: "Store your Anthropic API key in the system keyring and restart the agent",
    },
    CommandInfo {
        name: "logout",
        usage: "/logout",
        description: "Delete the API key from the system keyring",
    },
    CommandInfo {
        name: "modal",
        usage: "/modal close",
        description: "Close the front modal window",
    },
    CommandInfo {
        name: "q",
        usage: "/q",
        description: "Quit crest (also /quit and /qa)",
    },
];

/// The status-bar hint for `/command`, which nothing runs.
pub fn unknown_command_hint(command: &str) -> String {
    let names: Vec<String> = BUILTIN_COMMANDS
        .iter()
        .map(|info| format!("/{}", info.name))
        .collect();
    format!(
        "unknown command /{command} (use {}; /help lists every command)",
        names.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_unknown_command_hint_names_every_builtin_command() {
        let hint = unknown_command_hint("frobnicate");
        assert!(hint.starts_with("unknown command /frobnicate"), "{hint}");
        for info in BUILTIN_COMMANDS {
            assert!(hint.contains(&format!("/{}", info.name)), "{hint}");
            assert!(info.usage.starts_with(&format!("/{}", info.name)));
            assert!(!info.description.is_empty());
        }
        assert!(hint.contains("/help lists every command"), "{hint}");
    }
}
