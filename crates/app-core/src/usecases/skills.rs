//! What the `skill` tool does with a skill's text, in Claude Code's terms.
//!
//! - [`expand`]: the arguments (`$ARGUMENTS`, `$ARGUMENTS[0]`, `$0`) and
//!   the variables (`${CLAUDE_SKILL_DIR}`, `${CLAUDE_SESSION_ID}`) put in
//!   the body.
//! - [`shell_snippets`] and [`splice`]: the `` !`command` `` lines, run
//!   before the model reads the skill and replaced by their output.
//! - [`translate_tool`]: an `allowed-tools` entry (`Bash(git diff:*)`,
//!   `Read`, `Edit`) as crest's permission rules.
//! - [`listing`]: the tool's description, with the skills the model can
//!   load.
//! - [`invoked`] and [`compaction_reminder`]: the skills a compaction
//!   drops from the history, named in its summary.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::ops::Range;

use domain::conversation::{ContentPart, ToolUseId, Turn};
use domain::permission::{PermissionRule, RuleDecision, RuleScope, ToolEffect, split_command};
use serde_json::Value;

use crate::ports::skills::{SkillContext, SkillMeta};

/// The name of the tool that loads skills.
pub const SKILL_TOOL: &str = "skill";

/// How the ids of the `skill` calls a user's `/name` starts begin: the
/// model's calls never have it (providers choose their ids), so the tool
/// can tell the user's calls apart.
pub const USER_CALL_PREFIX: &str = "skill_user_";

/// Characters of the descriptions listed in the tool's description.
pub const LISTING_BUDGET: usize = 8_000;

/// The id of the `skill` call that runs the user's `/name` in `turn`.
#[must_use]
pub fn user_call_id(turn: &impl std::fmt::Display) -> ToolUseId {
    let id: String = turn
        .to_string()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    ToolUseId::new(format!("{USER_CALL_PREFIX}{id}"))
}

/// Whether the call `id` was started by the user (`/name`).
#[must_use]
pub fn by_user(id: &ToolUseId) -> bool {
    id.as_str().starts_with(USER_CALL_PREFIX)
}

/// `body` with the arguments and variables in place. When it names no
/// argument and `args` is not empty, the arguments follow it on an
/// `ARGUMENTS:` line, as in Claude Code.
#[must_use]
pub fn expand(body: &str, args: &str, dir: &str, session_id: &str) -> String {
    let args = args.trim();
    let words = split_command(args)
        .unwrap_or_else(|_| args.split_whitespace().map(str::to_owned).collect());
    let mut out = String::with_capacity(body.len() + args.len());
    let mut named = false;
    let mut rest = body;
    while let Some(at) = rest.find('$') {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        if let Some(after) = tail.strip_prefix("$ARGUMENTS") {
            named = true;
            if let Some((index, len)) = bracket_index(after) {
                out.push_str(words.get(index).map_or("", String::as_str));
                rest = &after[len..];
            } else {
                out.push_str(args);
                rest = after;
            }
        } else if let Some(after) = tail.strip_prefix("${CLAUDE_SKILL_DIR}") {
            out.push_str(dir);
            rest = after;
        } else if let Some(after) = tail.strip_prefix("${CLAUDE_SESSION_ID}") {
            out.push_str(session_id);
            rest = after;
        } else if let Some((index, len)) = digits(&tail[1..]) {
            named = true;
            out.push_str(words.get(index).map_or("", String::as_str));
            rest = &tail[1 + len..];
        } else {
            out.push('$');
            rest = &tail[1..];
        }
    }
    out.push_str(rest);
    if !named && !args.is_empty() {
        let _ = write!(out, "\n\nARGUMENTS: {args}");
    }
    out
}

/// `[N]` at the start of `text`: N and the length taken.
fn bracket_index(text: &str) -> Option<(usize, usize)> {
    let inner = text.strip_prefix('[')?;
    let (index, len) = digits(inner)?;
    inner[len..].starts_with(']').then_some((index, len + 2))
}

/// The number at the start of `text` and its length.
fn digits(text: &str) -> Option<(usize, usize)> {
    let len = text.bytes().take_while(u8::is_ascii_digit).count();
    if len == 0 {
        return None;
    }
    text[..len].parse().ok().map(|n| (n, len))
}

/// The `` !`command` `` snippets of `body`, outside fenced code: where
/// each is and its command line.
#[must_use]
pub fn shell_snippets(body: &str) -> Vec<(Range<usize>, String)> {
    let mut out = Vec::new();
    let mut fence = false;
    let mut offset = 0;
    for line in body.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fence = !fence;
        } else if !fence {
            let mut from = 0;
            while let Some(found) = line[from..].find("!`") {
                let start = from + found;
                let Some(len) = line[start + 2..].find('`') else {
                    break;
                };
                let command = line[start + 2..start + 2 + len].trim();
                let end = start + 2 + len + 1;
                if !command.is_empty() {
                    out.push((offset + start..offset + end, command.to_owned()));
                }
                from = end;
            }
        }
        offset += line.len();
    }
    out
}

/// `body` with each snippet replaced by its output, in order.
#[must_use]
pub fn splice(body: &str, snippets: &[(Range<usize>, String)], outputs: &[String]) -> String {
    let mut out = String::with_capacity(body.len());
    let mut at = 0;
    for ((range, _), output) in snippets.iter().zip(outputs) {
        out.push_str(&body[at..range.start]);
        out.push_str(output.trim_end());
        at = range.end;
    }
    out.push_str(&body[at..]);
    out
}

/// The rules an `allowed-tools` entry stands for: Claude Code's names
/// (`Bash(git diff:*)`, `Read`, `Edit`, `WebFetch`) as crest's tools, and
/// crest's own notation as it is. Empty for a tool crest does not have.
#[must_use]
pub fn translate_tool(entry: &str) -> Vec<PermissionRule> {
    let entry = entry.trim();
    let (name, inner) = match entry.split_once('(') {
        Some((name, rest)) => (name.trim(), rest.strip_suffix(')').map(str::trim)),
        None => (entry, None),
    };
    let rule = |tool: &str, pattern: Option<&str>| {
        let text = match pattern {
            Some(pattern) if !pattern.is_empty() => format!("{tool}({pattern})"),
            _ => tool.to_owned(),
        };
        PermissionRule::parse(&text, RuleDecision::AllowAlways, RuleScope::Session).ok()
    };
    let path = inner.map(path_pattern);
    let rules: Vec<Option<PermissionRule>> = match name {
        "Bash" => vec![rule("run_command", inner.map(command_pattern).as_deref())],
        "Read" => ["read_file", "grep", "find_file", "list_files", "tree"]
            .iter()
            .map(|tool| rule(tool, path.as_deref()))
            .collect(),
        "Edit" | "MultiEdit" | "NotebookEdit" => vec![
            rule("edit_file", path.as_deref()),
            rule("write_file", path.as_deref()),
        ],
        "Write" => vec![rule("write_file", path.as_deref())],
        "Grep" => vec![rule("grep", None)],
        "Glob" => vec![rule("find_file", None), rule("list_files", None)],
        "LS" => vec![rule("list_files", None)],
        "WebFetch" => match inner.and_then(|inner| inner.strip_prefix("domain:")) {
            Some(domain) => vec![
                rule("web_fetch", Some(&format!("https://{domain}*"))),
                rule("web_fetch", Some(&format!("http://{domain}*"))),
            ],
            None => vec![rule("web_fetch", None)],
        },
        "WebSearch" => vec![rule("web_search", None)],
        "Task" | "Agent" => vec![rule("task", None)],
        "Skill" => vec![rule(SKILL_TOOL, inner)],
        _ if name.starts_with("mcp__") || is_crest_tool(name) => vec![rule(name, inner)],
        _ => Vec::new(),
    };
    rules.into_iter().flatten().collect()
}

/// Whether `name` is written like a crest tool (`run_command`).
fn is_crest_tool(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// Claude Code's `git diff:*` (or `git diff *`) as crest's command
/// notation (`git diff *`).
fn command_pattern(inner: &str) -> String {
    let inner = inner.trim();
    if let Some(prefix) = inner.strip_suffix(":*") {
        return format!("{} *", prefix.trim_end());
    }
    inner.to_owned()
}

/// A path pattern (`./src/**`, `src/*.rs`) as crest's notation: a prefix
/// for one that ends in `*`, the path itself otherwise.
fn path_pattern(inner: &str) -> String {
    let inner = inner.trim().trim_start_matches("./");
    if inner == "**" || inner == "*" {
        return "*".to_owned();
    }
    match inner.find('*') {
        Some(star) => format!("{}*", &inner[..star]),
        None => inner.to_owned(),
    }
}

/// What a tool allowed by a rule does, to tell whether a project's skill
/// asks for more than reading.
#[must_use]
pub fn rule_effect(tool: &str) -> ToolEffect {
    match tool {
        "read_file" | "grep" | "find_file" | "list_files" | "tree" | "task" | SKILL_TOOL => {
            ToolEffect::Read
        }
        "web_fetch" | "web_search" => ToolEffect::Network,
        "edit_file" | "write_file" => ToolEffect::Write,
        _ => ToolEffect::Execute,
    }
}

/// The description of the `skill` tool: what it does and the skills the
/// model can load, cut to [`LISTING_BUDGET`] characters.
#[must_use]
pub fn listing(skills: &[SkillMeta]) -> String {
    let mut out = String::from(
        "Loads a skill: instructions the user keeps for a kind of task, with the files they \
         mention.\n\
         - When a request matches a skill below, call this tool first, before anything else, \
         and follow what it returns.\n\
         - `skill` is its name; `args` is what the request gives it (a file, a number...).\n\
         - Do not call it for a skill that is already loaded in this conversation, and never \
         for one that is not listed.\n\
         - A user's `/name` message already ran the skill: its result follows it.\n\
         - A skill that runs in a subagent waits for its report; with `background: true` it \
         runs while you go on, and the report arrives later as <agent-report>.\n\n\
         Skills:",
    );
    let usable: Vec<_> = skills.iter().filter(|s| s.model_invocable).collect();
    if usable.is_empty() {
        out.push_str("\n(none right now)");
        return out;
    }
    let mut used = 0;
    for (shown, skill) in usable.iter().enumerate() {
        let mut line = format!("\n- {}: {}", skill.name, one_line(&skill.description));
        if let Some(when) = &skill.when_to_use {
            let _ = write!(line, " {}", one_line(when));
        }
        if skill.context == SkillContext::Fork {
            line.push_str(" (runs in a subagent)");
        }
        if used + line.len() > LISTING_BUDGET {
            let _ = write!(
                out,
                "\n(+{} more, not listed: the list is too long)",
                usable.len() - shown
            );
            break;
        }
        used += line.len();
        out.push_str(&line);
    }
    out
}

/// `text` on one line, its whitespace collapsed.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The skills loaded in `turns`, by name.
pub fn invoked<'a>(turns: impl IntoIterator<Item = &'a Turn>) -> BTreeSet<String> {
    turns
        .into_iter()
        .flat_map(|turn| &turn.messages)
        .flat_map(|message| &message.content)
        .filter_map(|part| match part {
            ContentPart::ToolUse { name, input, .. } if name == SKILL_TOOL => {
                input.get("skill").and_then(Value::as_str)
            }
            _ => None,
        })
        .map(|name| name.trim().trim_start_matches('/').to_owned())
        .filter(|name| !name.is_empty())
        .collect()
}

/// What the summary of a compaction adds when it replaced turns that
/// loaded skills: their names, so the model loads them again if the work
/// goes on.
#[must_use]
pub fn compaction_reminder(names: &BTreeSet<String>) -> Option<String> {
    if names.is_empty() {
        return None;
    }
    let list = names
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    Some(format!(
        "Skills loaded earlier (their instructions were summarized; load them again with \
         `skill` if the work still needs them): {list}."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::permission::{ActionRequest, PermissionMode, PermissionPolicy, RuleMatcher};

    #[test]
    fn arguments_go_where_the_body_names_them() {
        assert_eq!(
            expand("Fix issue $ARGUMENTS now", "123 high", "/s", "id"),
            "Fix issue 123 high now"
        );
        assert_eq!(
            expand(
                "First $0, second $ARGUMENTS[1], none $5.",
                "a \"b c\"",
                "/s",
                "id"
            ),
            "First a, second b c, none ."
        );
        assert_eq!(
            expand(
                "Run ${CLAUDE_SKILL_DIR}/x.sh for ${CLAUDE_SESSION_ID}",
                "",
                "/s",
                "id"
            ),
            "Run /s/x.sh for id"
        );
        assert_eq!(expand("Costs $5? No: $x", "", "/s", "id"), "Costs ? No: $x");
    }

    #[test]
    fn unnamed_arguments_follow_the_body() {
        assert_eq!(
            expand("Review the code.", "src/lib.rs", "/s", "id"),
            "Review the code.\n\nARGUMENTS: src/lib.rs"
        );
        assert_eq!(
            expand("Review the code.", "  ", "/s", "id"),
            "Review the code."
        );
    }

    #[test]
    fn snippets_are_found_outside_code_and_spliced() {
        let body = "Status: !`git status --short`\n```\n!`not this`\n```\nLog: !`git log -1` end\n";
        let snippets = shell_snippets(body);
        let commands: Vec<_> = snippets.iter().map(|(_, c)| c.as_str()).collect();
        assert_eq!(commands, ["git status --short", "git log -1"]);
        let out = splice(body, &snippets, &["M a.rs\n".into(), "abc fix".into()]);
        assert_eq!(
            out,
            "Status: M a.rs\n```\n!`not this`\n```\nLog: abc fix end\n"
        );
    }

    fn shown(rules: &[PermissionRule]) -> Vec<String> {
        rules.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn claude_code_tools_become_crest_rules() {
        assert_eq!(
            shown(&translate_tool("Bash(git diff:*)")),
            ["run_command(git diff *)"]
        );
        assert_eq!(shown(&translate_tool("Bash")), ["run_command"]);
        assert_eq!(
            shown(&translate_tool("Edit(./src/**)")),
            ["edit_file(src/*)", "write_file(src/*)"]
        );
        assert_eq!(translate_tool("Read").len(), 5);
        assert_eq!(
            shown(&translate_tool("mcp__github__get_issue")),
            ["mcp__github__get_issue"]
        );
        assert_eq!(
            shown(&translate_tool("run_command(cargo test *)")),
            ["run_command(cargo test *)"]
        );
        assert!(translate_tool("NoSuchTool").is_empty());
        assert!(matches!(
            translate_tool("Grep")[0].matcher,
            RuleMatcher::Any
        ));
    }

    #[test]
    fn a_translated_rule_allows_the_command() {
        let mut policy = PermissionPolicy::new(PermissionMode::Ask);
        policy.grant(translate_tool("Bash(git diff:*)"));
        let request = ActionRequest::new("run_command", ToolEffect::Execute, "git diff --cached")
            .with_argv(vec!["git".into(), "diff".into(), "--cached".into()]);
        assert_eq!(policy.decide(&request), domain::permission::Decision::Allow);
    }

    #[test]
    fn the_listing_shows_what_the_model_may_load_within_the_budget() {
        let mut hidden = SkillMeta::new("deploy", "Deploys");
        hidden.model_invocable = false;
        let mut forked = SkillMeta::new("research", "Researches");
        forked.context = SkillContext::Fork;
        forked.when_to_use = Some("Use when\n asked".into());
        let text = listing(&[SkillMeta::new("review", "Reviews the diff"), hidden, forked]);
        assert!(text.contains("\n- review: Reviews the diff"));
        assert!(text.contains("\n- research: Researches Use when asked (runs in a subagent)"));
        assert!(!text.contains("deploy"));
        let many: Vec<_> = (0..400)
            .map(|i| SkillMeta::new(format!("s{i}"), "x".repeat(40)))
            .collect();
        let text = listing(&many);
        assert!(text.len() < LISTING_BUDGET + 1_000);
        assert!(text.contains("more, not listed"));
        assert!(listing(&[]).contains("(none right now)"));
    }

    #[test]
    fn user_calls_are_told_apart() {
        let id = user_call_id(&"0193-abcd");
        assert_eq!(id.as_str(), "skill_user_0193abcd");
        assert!(by_user(&id));
        assert!(!by_user(&ToolUseId::new("toolu_01")));
    }
}
