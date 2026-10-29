use application::keymap::{Action, default_keymap};
use domain::model::ModelId;
use domain::permission::PermissionMode;

use super::*;

fn snapshot() -> HelpSnapshot {
    HelpSnapshot {
        commands: vec![CommandRow {
            name: "review".into(),
            usage: "/review [args]".into(),
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
            approve: Some("/plugin trust jira".into()),
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
    let help = full.keys.iter().find(|k| k.keys == "<F1>").unwrap();
    assert_eq!(help.action, HELP_ACTION);
    assert_eq!(help.description, "Open the help");
    assert_eq!(help.source, "default");
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
        .set("<leader>k", Action::builtin("block.last"))
        .unwrap();
    keymap.set("<leader>z", Action::Callback(4)).unwrap();
    let rows = key_rows(&keymap, |entry| match entry.action {
        Action::Callback(4) => Some((Some("Clear".into()), "jira".into())),
        _ => None,
    });
    let find = |keys: &str| rows.iter().find(|r| r.keys == keys).unwrap();
    let j = find("<C-g>k");
    assert!(j.overridden);
    assert_eq!(j.description, "Select the last block");
    assert_eq!(j.source, "global");
    let x = find("<C-g>z");
    assert_eq!(
        (x.action.as_str(), x.description.as_str(), x.source.as_str()),
        ("Lua function", "Clear", "jira")
    );
    assert!(!x.overridden);
    assert!(!find("<C-g>j").overridden);
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
        // Keys: F1 and <leader>h, then the help's own eight.
        ("help", &[(Section::Commands, 1), (Section::Keys, 10)]),
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
    assert_eq!(plugins[0].copy, "/plugin trust jira");
    let commands = section_rows(&full, Section::Commands);
    assert_eq!(commands[0].copy, "/help");
    assert_eq!(
        commands.last().map(|r| r.tags.clone()),
        Some(vec!["jira".into()])
    );
    let cli = section_rows(&full, Section::Cli);
    assert_eq!(cli[0].copy, "--model");
    let keys = section_rows(&full, Section::Keys);
    assert!(
        keys[0]
            .group
            .as_deref()
            .is_some_and(|g| g.starts_with("Shortcuts"))
    );
    let files = section_rows(&full, Section::Files);
    assert_eq!(files.len(), 2);
    assert_eq!(files[1].group.as_deref(), Some("Problems at startup"));
}

#[test]
fn keys_read_as_a_person_types_them() {
    use application::keymap::parse_keys;
    let label = |notation: &str| keys_label(&parse_keys(notation, "<C-g>").unwrap());
    assert_eq!(label("<leader>ff"), "Ctrl+G ff");
    assert_eq!(label("<A-p>"), "Alt+P");
    assert_eq!(label("<F1>"), "F1");
    assert_eq!(label("<C-S-c>"), "Ctrl+Shift+C");
    assert_eq!(label("<Space>fg"), "Space fg");
    assert_eq!(label("<leader>G"), "Ctrl+G G");

    let keymap = default_keymap();
    assert_eq!(
        action_keys(&keymap, "picker.files").as_deref(),
        Some("Ctrl+G ff")
    );
    assert_eq!(action_keys(&keymap, "help.open").as_deref(), Some("F1"));
    assert_eq!(action_keys(&keymap, "nothing.here"), None);
}

#[test]
fn commands_of_builtin_actions_say_their_keys() {
    let keymap = default_keymap();
    let full = snapshot().complete(&LocalHelp {
        keymap: &keymap,
        models: &[],
        permission_mode: PermissionMode::Ask,
        turn_override: None,
    });
    let row = |name: &str| full.commands.iter().find(|row| row.name == name).unwrap();
    assert!(row("picker.files").description.ends_with("(Ctrl+G ff)"));
    assert!(row("picker.grep").description.ends_with("(Ctrl+G fg)"));
    assert_eq!(row("review").description, "Review the changes");
}

#[test]
fn the_keys_list_each_panel_after_the_keymap_once() {
    let keymap = default_keymap();
    let full = snapshot().complete(&local(&keymap, &[]));
    let fixed = full.keys.iter().filter(|r| r.panel.is_some()).count();
    assert_eq!(fixed, panel_keys::PANEL_KEYS.len());
    // The panels come after the keymap, and completing again adds nothing.
    let first_fixed = full.keys.iter().position(|r| r.panel.is_some()).unwrap();
    assert!(full.keys[first_fixed..].iter().all(|r| r.panel.is_some()));
    let again = full.clone().complete(&local(&keymap, &[]));
    assert_eq!(again.keys.len(), full.keys.len());

    let rows = section_rows(&full, Section::Keys);
    let groups: Vec<&str> = rows.iter().filter_map(|r| r.group.as_deref()).collect();
    assert!(groups[0].starts_with("Shortcuts"));
    assert!(groups.contains(&"Chats sidebar (fixed keys)"), "{groups:?}");
    assert!(groups.contains(&"Approval dialog (fixed keys)"));
    // A fixed row says where it works and that it is fixed, with no
    // snippet; the search finds a panel by name.
    assert_eq!(rows[first_fixed].tags[1], "fixed");
    assert!(rows[first_fixed].snippet.is_none());
    assert!(rows[first_fixed].details[0].starts_with("fixed: "));
    let sidebar = filter(&full, "chats sidebar rename");
    assert_eq!(
        sidebar
            .keys
            .iter()
            .map(|r| r.keys.as_str())
            .collect::<Vec<_>>(),
        ["F2, r"]
    );
}

#[test]
fn remap_snippets_use_the_leader_and_escape_quotes() {
    let mut keymap = application::keymap::default_keymap_with(
        "<C-Space>",
        application::keymap::Platform::current(),
    );
    keymap.set("<F9>", Action::Callback(2)).unwrap();
    let rows = key_rows(&keymap, |_| None);
    let row = |keys: &str| rows.iter().find(|r| r.keys == keys).unwrap();
    let files = row("<C-Space>ff");
    assert_eq!(
        files.source, "default",
        "moved with the leader, still a default"
    );
    assert!(!files.overridden);
    assert_eq!(
        files.snippet.as_deref(),
        Some(
            "-- init.lua\napp.keymap.set(\"<leader>ff\", \"picker.files\")\n\n\
             # keymap.toml\n[[bind]]\nkeys = \"<leader>ff\"\naction = \"picker.files\"\n"
        )
    );
    let f9 = row("<F9>");
    assert_eq!(f9.source, "global");
    assert!(
        f9.snippet
            .as_deref()
            .is_some_and(|s| s.contains("app.keymap.del(\"<F9>\")") && s.contains("[[unbind]]"))
    );
    assert_eq!(
        remap_snippet("a\"b\\", Some("x")).lines().nth(1),
        Some("app.keymap.set(\"a\\\"b\\\\\", \"x\")")
    );
}

#[test]
fn next_keys_say_what_follows_a_prefix() {
    let mut keymap = default_keymap();
    keymap.set("<leader>f", Action::Callback(1)).unwrap();
    let leader = application::keymap::parse_keys("<leader>", keymap.leader()).unwrap();
    let next = next_keys(&keymap, &leader);
    let what = |key: &str| next.iter().find(|n| n.key == key).map(|n| n.what.as_str());
    assert_eq!(what("e"), Some("Show or hide the workspace tree"));
    assert_eq!(what("f"), Some("Lua function (+2 more)"));
    assert_eq!(what("Ctrl+C"), Some("Interrupt the running turn"));
    let q = application::keymap::Key::char('q');
    assert!(next_keys(&keymap, &[q]).is_empty(), "nothing starts with q");
}
