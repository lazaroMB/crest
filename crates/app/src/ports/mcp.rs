//! MCP port: tools exposed by Model Context Protocol servers.
//!
//! MCP tools are offered to the model under a qualified name,
//! `mcp__<server>__<tool>`, so tools of different servers never clash with
//! each other or with built-in tools. See [`qualified_name`] and
//! [`parse_qualified_name`].

/// Prefix of every MCP tool name as seen by the model.
pub const QUALIFIED_PREFIX: &str = "mcp__";

/// Separator between server and tool in a qualified name.
const SEPARATOR: &str = "__";

/// Name under which `tool` of `server` is offered to the model:
/// `mcp__<server>__<tool>`.
///
/// Server names must not contain `__`; tool names may.
#[must_use]
pub fn qualified_name(server: &str, tool: &str) -> String {
    format!("{QUALIFIED_PREFIX}{server}{SEPARATOR}{tool}")
}

/// Inverse of [`qualified_name`]: splits `mcp__<server>__<tool>` into
/// `(server, tool)`. Returns `None` for names that are not MCP tools or
/// have an empty server or tool.
#[must_use]
pub fn parse_qualified_name(name: &str) -> Option<(&str, &str)> {
    let rest = name.strip_prefix(QUALIFIED_PREFIX)?;
    let (server, tool) = rest.split_once(SEPARATOR)?;
    (!server.is_empty() && !tool.is_empty()).then_some((server, tool))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qualified_name_round_trips() {
        let name = qualified_name("github", "create_issue");
        assert_eq!(name, "mcp__github__create_issue");
        assert_eq!(
            parse_qualified_name(&name),
            Some(("github", "create_issue"))
        );
    }

    #[test]
    fn tool_names_may_contain_the_separator() {
        let name = qualified_name("fs", "read__raw");
        assert_eq!(parse_qualified_name(&name), Some(("fs", "read__raw")));
    }

    #[test]
    fn non_mcp_names_do_not_parse() {
        assert_eq!(parse_qualified_name("grep"), None);
        assert_eq!(parse_qualified_name("mcp__github"), None);
        assert_eq!(parse_qualified_name("mcp____tool"), None);
        assert_eq!(parse_qualified_name("mcp__github__"), None);
    }
}
