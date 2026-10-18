use application::keymap::{Action, Mode, default_keymap};
use domain::model::ModelId;
use domain::permission::PermissionMode;

use super::*;

fn snapshot() -> HelpSnapshot {
    HelpSnapshot {
        commands: vec![CommandRow {
            name: "review".into(),
            usage: ":review [args]".into(),
            description: "Review the changes".into(),
            source: "jira".into(),
        }],
        tools: vec![
            ToolRow {
                name: "read_file".into(),
                description: "Reads a file".into(),
                source: ToolSource::Builtin,
                effect: "read".into(),
                schema: "{}".into(),
            },
            ToolRow {
                name: "jira_issue".into(),
                description: "Fetches an issue".into(),
                source: ToolSource::Plugin("jira".into()),
                effect: "execute".into(),
                schema: "{\n  \"type\": \"object\"\n}".into(),
            },
            ToolRow {
                name: "echo__say".into(),
                description: "Echoes".into(),
                source: ToolSource::Mcp("echo".into()),
                effect: "execute".into(),
                schema: "{}".into(),
            },
        ],
        plugins: vec![PluginRow {
            name: "jira".into(),
            layer: "global".into(),
            adds: vec!["tool jira_issue".into()],
            capabilities: vec!["network:jira.example.com".into()],
            trust: "pending".into(),
            approve: Some(":plugin trust jira".into()),
            ..PluginRow::default()
        }],
        mcp: vec![McpRow {
            name: "echo".into(),
            transport: "stdio: echo-server".into(),
            origin: "global".into(),
            state: "ready (1 tool)".into(),
            trust_command: None,
        }],
        cli: vec![CliRow {
            flag: "-m, --model <ALIAS|ID>".into(),
            description: "Model for the turn".into(),
        }],
        paths: vec![PathRow {
            label: "workspace".into(),
            value: "/ws".into(),
        }],
        diagnostics: vec!["init.lua:3: unexpected symbol".into()],
        ..HelpSnapshot::default()
    }
}

fn local<'a>(keymap: &'a Keymap<Action>, models: &'a [ModelChoice]) -> LocalHelp<'a> {
    LocalHelp {
        keymap,
        models,
        permission_mode: PermissionMode::Plan,
        turn_override: Some("opus"),
    }
}

#[test]
fn complete_adds_the_builtin_commands_keys_and_models() {
    let keymap = default_keymap();
    let models = vec![ModelChoice {
        alias: "opus".into(),
        model: ModelId::new("claude-opus"),
    }];
    let full = snapshot().complete(&local(&keymap, &models));
    let names: Vec<_> = full.commands.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names.first(), Some(&"help"));
    assert_eq!(names.last(), Some(&"review"), "plugin commands after");
    assert!(names.contains(&"model") && names.contains(&"q"));
    let help = full
        .keys
        .iter()
        .find(|k| k.mode == Mode::Normal && k.keys == "?")
        .unwrap();
    assert_eq!(help.action, HELP_ACTION);
    assert_eq!(help.description, "Open the help");
    assert_eq!(help.source, BUILTIN_SOURCE);
    assert!(!help.overridden);
    let models: Vec<_> = full
        .models
        .iter()
        .map(|m| (m.kind.as_str(), m.name.as_str(), m.value.as_str()))
        .collect();
    assert_eq!(
        models,
        [
            ("alias", "opus", "claude-opus"),
            ("setting", "next turn", "opus"),
            ("setting", "permission mode", "plan"),
        ]
    );
}

#[test]
fn key_rows_mark_overrides_and_name_the_plugin() {
    let mut keymap = default_keymap();
    keymap
        .set(Mode::Normal, "j", Action::builtin("block.last"))
        .unwrap();
    keymap
        .set(Mode::Normal, "<leader>x", Action::Callback(4))
        .unwrap();
    let rows = key_rows(&keymap, |entry| match entry.action {
        Action::Callback(4) => Some((Some("Clear".into()), "jira".into())),
        _ => None,
    });
    let find = |keys: &str| rows.iter().find(|r| r.keys == keys).unwrap();
    let j = find("j");
    assert!(j.overridden);
    assert_eq!(j.description, "Select the last block");
    assert_eq!(j.source, "config");
    let x = find("<Space>x");
    assert_eq!(
        (x.action.as_str(), x.description.as_str(), x.source.as_str()),
        ("Lua function", "Clear", "jira")
    );
    assert!(!x.overridden);
    assert!(!find("k").overridden);
}

#[test]
fn filter_keeps_matching_rows_in_every_section() {
    let keymap = default_keymap();
    let full = snapshot().complete(&local(&keymap, &[]));
    let cases: &[(&str, &[(Section, usize)])] = &[
        ("", &[(Section::Tools, 3), (Section::Plugins, 1)]),
        (
            "jira",
            &[
                (Section::Commands, 1),
                (Section::Tools, 1),
                (Section::Plugins, 1),
                (Section::Keys, 0),
                (Section::Mcp, 0),
            ],
        ),
        ("ECHO", &[(Section::Tools, 1), (Section::Mcp, 1)]),
        ("issue fetches", &[(Section::Tools, 1)]),
        ("issue nope", &[(Section::Tools, 0)]),
        ("help", &[(Section::Commands, 1), (Section::Keys, 4)]),
        ("model", &[(Section::Cli, 1)]),
        ("unexpected", &[(Section::Files, 1)]),
        ("pending", &[(Section::Plugins, 1)]),
    ];
    for (query, expected) in cases {
        let filtered = filter(&full, query);
        for (section, n) in *expected {
            assert_eq!(filtered.len(*section), *n, "{query:?} in {section:?}");
        }
    }
    assert_eq!(filter(&full, "   "), full);
}

#[test]
fn sections_parse_from_names_and_numbers() {
    assert_eq!(Section::parse("tools"), Some(Section::Tools));
    assert_eq!(Section::parse("MCP"), Some(Section::Mcp));
    assert_eq!(Section::parse("3"), Some(Section::Tools));
    assert_eq!(Section::parse("8"), Some(Section::Files));
    assert_eq!(Section::parse("9"), None);
    assert_eq!(Section::parse("0"), None);
    assert_eq!(Section::parse("jira"), None);
    assert_eq!(
        parse_help_arg(" keys "),
        (Some(Section::Keys), String::new())
    );
    assert_eq!(parse_help_arg("jira issue"), (None, "jira issue".into()));
    assert_eq!(parse_help_arg(""), (None, String::new()));
}

#[test]
fn rows_group_and_copy_what_is_useful() {
    let keymap = default_keymap();
    let full = snapshot().complete(&local(&keymap, &[]));
    let tools = section_rows(&full, Section::Tools);
    let groups: Vec<_> = tools.iter().map(|r| r.group.as_deref()).collect();
    assert_eq!(groups, [Some("Built-in"), Some("Plugin"), Some("MCP")]);
    assert_eq!(tools[1].copy, "jira_issue");
    assert!(tools[1].details.iter().any(|d| d.contains("\"type\"")));
    let plugins = section_rows(&full, Section::Plugins);
    assert_eq!(plugins[0].copy, ":plugin trust jira");
    let commands = section_rows(&full, Section::Commands);
    assert_eq!(commands[0].copy, ":help");
    assert_eq!(
        commands.last().map(|r| r.tags.clone()),
        Some(vec!["jira".into()])
    );
    let cli = section_rows(&full, Section::Cli);
    assert_eq!(cli[0].copy, "--model");
    let keys = section_rows(&full, Section::Keys);
    assert_eq!(keys[0].group.as_deref(), Some("insert mode"));
    let files = section_rows(&full, Section::Files);
    assert_eq!(files.len(), 2);
    assert_eq!(files[1].group.as_deref(), Some("Problems at startup"));
}
