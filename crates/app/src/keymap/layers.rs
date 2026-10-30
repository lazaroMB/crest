//! The keymap a person ends up with: the builtin mappings under the leader
//! of `app.opt.leader`, with the mappings of every config layer applied on
//! top, and the warnings worth showing about them.
//!
//! Layers, lowest first: the plugins, the global `keymap.toml`, the global
//! `init.lua`, the workspace `keymap.toml`, the workspace `init.lua`. So
//! the workspace wins over the global config and, within one layer, Lua
//! wins over the TOML file (it can do more: functions, `desc`).
//!
//! `keymap.toml` is the declarative form for those who do not want Lua:
//!
//! ```toml
//! [[bind]]
//! keys = "<leader>ff"
//! action = "picker.files"
//! desc = "Find a file"   # optional
//!
//! [[unbind]]
//! keys = "<C-g>x"
//! ```
//!
//! Its `[[unbind]]` entries apply before its `[[bind]]` ones, so a file can
//! free a builtin key and bind another action to it. Problems never stop
//! the app: a file that does not parse, or an entry that does not make
//! sense, becomes a warning and is skipped.

use std::collections::{HashMap, HashSet};

use toml::Value;

use super::{
    Action, DEFAULT_LEADER, Key, KeyCode, KeySequence, Keymap, Platform, builtin_actions,
    default_keymap_with, parse_keys_on,
};
use crate::ports::plugin::{KeyAction, KeymapSpec};

/// Name of the declarative keymap file, next to `init.lua` (global and
/// workspace).
pub const KEYMAP_FILE: &str = "keymap.toml";
/// `plugin` of the mappings of the global `init.lua`.
pub const GLOBAL_CONFIG: &str = "config";
/// `plugin` of the mappings of the workspace `init.lua`.
pub const WORKSPACE_CONFIG: &str = "workspace";
/// `plugin` of the mappings of the global `keymap.toml`.
pub const GLOBAL_TOML: &str = "global keymap.toml";
/// `plugin` of the mappings of the workspace `keymap.toml`.
pub const WORKSPACE_TOML: &str = "workspace keymap.toml";
/// Source of the builtin mappings, as [`source_label`] names them.
pub const DEFAULT_SOURCE: &str = "default";

/// Which config layer a `keymap.toml` belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeymapLayer {
    /// `<config dir>/keymap.toml`.
    Global,
    /// `<workspace>/.crest/keymap.toml`.
    Workspace,
}

impl KeymapLayer {
    /// The `plugin` its mappings carry.
    #[must_use]
    pub const fn source(self) -> &'static str {
        match self {
            Self::Global => GLOBAL_TOML,
            Self::Workspace => WORKSPACE_TOML,
        }
    }
}

/// Precedence of a mapping's `plugin`, lowest first.
fn rank(plugin: &str) -> u8 {
    match plugin {
        GLOBAL_TOML => 1,
        GLOBAL_CONFIG | "" => 2,
        WORKSPACE_TOML => 3,
        WORKSPACE_CONFIG => 4,
        _ => 0,
    }
}

/// Where a mapping comes from, for people: `default`, `global`,
/// `workspace`, `global keymap.toml`, `workspace keymap.toml` or
/// `plugin <name>`.
#[must_use]
pub fn source_label(plugin: &str) -> String {
    match plugin {
        GLOBAL_CONFIG | "" => "global".to_owned(),
        WORKSPACE_CONFIG => "workspace".to_owned(),
        GLOBAL_TOML | WORKSPACE_TOML | DEFAULT_SOURCE => plugin.to_owned(),
        name => format!("plugin {name}"),
    }
}

/// The mappings of Lua (already in precedence order) and of both
/// `keymap.toml` files, in the order to apply them.
#[must_use]
pub fn layered(
    lua: Vec<KeymapSpec>,
    global_toml: Vec<KeymapSpec>,
    workspace_toml: Vec<KeymapSpec>,
) -> Vec<KeymapSpec> {
    let mut all: Vec<KeymapSpec> = lua
        .into_iter()
        .chain(global_toml)
        .chain(workspace_toml)
        .collect();
    // Stable: each layer keeps its own order.
    all.sort_by_key(|spec| rank(&spec.plugin));
    all
}

/// Whether `key` would be typed into the prompt: a character without Ctrl,
/// Alt or Cmd.
#[must_use]
pub fn is_typed(key: Key) -> bool {
    matches!(key.code, KeyCode::Char(_)) && !(key.mods.ctrl || key.mods.alt || key.mods.cmd)
}

fn notation(keys: &[Key]) -> String {
    keys.iter().map(ToString::to_string).collect()
}

/// The leader to use for `wanted` (`app.opt.leader`), with a warning when
/// it cannot be one.
fn leader_of(wanted: Option<&str>, platform: Platform) -> (&str, Option<String>) {
    let Some(leader) = wanted else {
        return (DEFAULT_LEADER, None);
    };
    match parse_keys_on("<leader>", leader, platform) {
        Err(err) => (
            DEFAULT_LEADER,
            Some(format!(
                "invalid app.opt.leader `{leader}`: {err}; the leader stays `{DEFAULT_LEADER}`"
            )),
        ),
        Ok(keys) if keys.first().copied().is_some_and(is_typed) => (
            DEFAULT_LEADER,
            Some(format!(
                "app.opt.leader `{leader}` starts with a key the prompt types; use a chord \
                 (`<C-…>`, `<F…>`); the leader stays `{DEFAULT_LEADER}`"
            )),
        ),
        Ok(_) => (leader, None),
    }
}

/// The warning for `later` setting the keys `earlier` set, when that is
/// likely a mistake: twice in the same file, or two plugins on the same
/// keys. A higher layer replacing a lower one is what layers are for.
fn clash(earlier: &KeymapSpec, later: &KeymapSpec) -> Option<String> {
    let (a, b) = (&earlier.plugin, &later.plugin);
    if rank(a) == rank(b) && (a == b || rank(a) != 0) {
        return Some(format!(
            "keymap `{}` is mapped twice in {}; the last one wins",
            later.lhs,
            source_label(b)
        ));
    }
    (rank(a) == 0 && rank(b) == 0).then(|| {
        format!(
            "plugins `{a}` and `{b}` both map `{}`; `{b}` wins",
            later.lhs
        )
    })
}

/// The builtin keymap of `platform` with `specs` applied in order (see
/// [`layered`]) and the warnings to show.
///
/// `<leader>` expands to `leader` (`app.opt.leader`), in the builtin
/// mappings too: changing the leader moves them and frees `<C-g>`. A
/// [`KeyAction::Unmap`] removes the mapping of its keys, a builtin one
/// included. Skipped with a warning: keys that do not parse, mappings that
/// start with a key the prompt types, unknown builtin actions. Also
/// warned about: the same keys mapped twice in one file (or by two
/// plugins), and a mapping that is the start of a longer one, which then
/// waits for `timeoutlen`.
#[must_use]
pub fn build_keymap(
    specs: &[KeymapSpec],
    leader: Option<&str>,
    platform: Platform,
) -> (Keymap<Action>, Vec<String>) {
    let (leader, leader_warning) = leader_of(leader, platform);
    let mut warnings: Vec<String> = leader_warning.into_iter().collect();
    let mut keymap = default_keymap_with(leader, platform);
    let builtins = builtin_actions();
    // The spec that last set each sequence.
    let mut set_by: HashMap<KeySequence, usize> = HashMap::new();
    for (ix, spec) in specs.iter().enumerate() {
        let keys = match parse_keys_on(&spec.lhs, leader, platform) {
            Ok(keys) => keys,
            Err(err) => {
                warnings.push(format!("keymap `{}` ignored: {err}", spec.lhs));
                continue;
            }
        };
        let action = match &spec.action {
            KeyAction::Unmap => {
                if keymap.del_keys(&keys).is_none() {
                    warnings.push(format!(
                        "keymap.del `{}`: nothing is mapped to these keys",
                        spec.lhs
                    ));
                }
                set_by.remove(&keys);
                continue;
            }
            KeyAction::Builtin(name) if !builtins.contains(&name.as_str()) => {
                warnings.push(format!(
                    "keymap `{}` ignored: unknown action `{name}` (the help's Keys section \
                     lists the actions)",
                    spec.lhs
                ));
                continue;
            }
            KeyAction::Builtin(name) => Action::Builtin(name.clone()),
            KeyAction::Callback(id) => Action::Callback(id.0),
        };
        if keys.first().copied().is_some_and(is_typed) {
            warnings.push(format!(
                "keymap `{}` ignored: it starts with a key the prompt types; use a chord \
                 (`<C-…>`, `<F…>`) or `<leader>`",
                spec.lhs
            ));
            continue;
        }
        if let Some(warning) = set_by
            .get(&keys)
            .and_then(|earlier| clash(&specs[*earlier], spec))
        {
            warnings.push(warning);
        }
        keymap.set_keys(&keys, action);
        set_by.insert(keys, ix);
    }
    warnings.extend(shadowing(&keymap, &set_by));
    (keymap, warnings)
}

/// Warnings for the mappings that are the start of longer ones, where a
/// person's mapping is one of the two: the short one fires only after
/// `timeoutlen` (or the next key).
fn shadowing(keymap: &Keymap<Action>, set_by: &HashMap<KeySequence, usize>) -> Vec<String> {
    let mut mine: Vec<(&KeySequence, usize)> = set_by.iter().map(|(k, ix)| (k, *ix)).collect();
    mine.sort_by_key(|(_, ix)| *ix);
    let entries = keymap.entries();
    let mut reported: HashSet<KeySequence> = HashSet::new();
    let mut out = Vec::new();
    for (keys, _) in mine {
        for len in 1..=keys.len() {
            let prefix = &keys[..len];
            let found = keymap.lookup(prefix);
            if found.exact.is_none() || !found.is_prefix || !reported.insert(prefix.to_vec()) {
                continue;
            }
            let longer = entries
                .iter()
                .find(|e| e.keys.len() > len && e.keys.starts_with(prefix))
                .map(super::KeymapEntry::notation)
                .unwrap_or_default();
            out.push(format!(
                "keymap `{}` is also the start of `{longer}`: it runs only after timeoutlen \
                 (app.opt.timeoutlen) or the next key",
                notation(prefix)
            ));
        }
    }
    out
}

/// The mappings of a `keymap.toml` of `layer`, read from `text`, and the
/// problems found (prefixed with `file`). Unbinds come first.
#[must_use]
pub fn parse_keymap_toml(
    text: &str,
    file: &str,
    layer: KeymapLayer,
) -> (Vec<KeymapSpec>, Vec<String>) {
    let mut warnings = Vec::new();
    let table: toml::Table = match text.parse() {
        Ok(table) => table,
        Err(err) => {
            let err: toml::de::Error = err;
            let at = err
                .span()
                .map(|span| {
                    let line = text[..span.start.min(text.len())].matches('\n').count() + 1;
                    format!(":{line}")
                })
                .unwrap_or_default();
            warnings.push(format!(
                "{file}{at}: {}; the file is ignored",
                err.message().trim()
            ));
            return (Vec::new(), warnings);
        }
    };
    for key in table.keys() {
        if key != "bind" && key != "unbind" {
            warnings.push(format!(
                "{file}: unknown key `{key}` (expected [[bind]] and [[unbind]])"
            ));
        }
    }
    let mut specs = Vec::new();
    for (section, unbind) in [("unbind", true), ("bind", false)] {
        let Some(value) = table.get(section) else {
            continue;
        };
        let Value::Array(entries) = value else {
            warnings.push(format!(
                "{file}: `{section}` must be a list of tables ([[{section}]])"
            ));
            continue;
        };
        for (n, entry) in entries.iter().enumerate() {
            let at = format!("{file}: [[{section}]] #{}", n + 1);
            match toml_entry(entry, unbind, layer) {
                Ok((spec, unknown)) => {
                    warnings.extend(
                        unknown
                            .into_iter()
                            .map(|field| format!("{at}: unknown field `{field}`")),
                    );
                    specs.push(spec);
                }
                Err(message) => warnings.push(format!("{at}: {message}; the entry is ignored")),
            }
        }
    }
    (specs, warnings)
}

/// One `[[bind]]` or `[[unbind]]` entry and the fields it does not know.
fn toml_entry(
    entry: &Value,
    unbind: bool,
    layer: KeymapLayer,
) -> Result<(KeymapSpec, Vec<String>), String> {
    let Value::Table(fields) = entry else {
        return Err("expected a table".to_owned());
    };
    let text = |name: &str| -> Result<Option<String>, String> {
        match fields.get(name) {
            None => Ok(None),
            Some(Value::String(s)) => Ok(Some(s.trim().to_owned()).filter(|s| !s.is_empty())),
            Some(other) => Err(format!(
                "`{name}` must be a string, got {}",
                other.type_str()
            )),
        }
    };
    let keys = text("keys")?.ok_or("missing `keys`")?;
    let known: &[&str] = if unbind {
        &["keys"]
    } else {
        &["keys", "action", "desc"]
    };
    let unknown = fields
        .keys()
        .filter(|k| !known.contains(&k.as_str()))
        .cloned()
        .collect();
    let (action, description) = if unbind {
        (KeyAction::Unmap, None)
    } else {
        let action = text("action")?.ok_or("missing `action`")?;
        (KeyAction::Builtin(action), text("desc")?)
    };
    Ok((
        KeymapSpec {
            lhs: keys,
            action,
            description,
            plugin: layer.source().to_owned(),
        },
        unknown,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::plugin::CallbackId;

    const ON: Platform = Platform::Other;

    fn spec(lhs: &str, action: &str, plugin: &str) -> KeymapSpec {
        KeymapSpec {
            lhs: lhs.into(),
            action: match action {
                "-" => KeyAction::Unmap,
                "fn" => KeyAction::Callback(CallbackId(1)),
                name => KeyAction::Builtin(name.into()),
            },
            description: None,
            plugin: plugin.into(),
        }
    }

    fn get(keymap: &Keymap<Action>, notation: &str) -> Option<Action> {
        let keys = parse_keys_on(notation, keymap.leader(), ON).unwrap_or_default();
        keymap.lookup(&keys).exact.cloned()
    }

    fn b(name: &str) -> Option<Action> {
        Some(Action::builtin(name))
    }

    #[test]
    fn del_removes_a_builtin_and_its_keys_can_be_mapped_again() {
        let specs = [
            spec("<leader>x", "-", GLOBAL_CONFIG),
            spec("<C-c>", "-", GLOBAL_CONFIG),
            spec("<leader>e", "-", WORKSPACE_CONFIG),
            spec("<leader>e", "picker.files", WORKSPACE_CONFIG),
        ];
        let (keymap, warnings) = build_keymap(&specs, None, ON);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(get(&keymap, "<C-g>x"), None);
        assert_eq!(get(&keymap, "<C-c>"), None);
        assert_eq!(get(&keymap, "<C-g>e"), b("picker.files"));
        assert_eq!(get(&keymap, "<C-g>j"), b("block.next"), "the rest stay");
    }

    #[test]
    fn deleting_nothing_warns() {
        let (_, warnings) = build_keymap(&[spec("<C-k>", "-", GLOBAL_CONFIG)], None, ON);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("nothing is mapped"), "{warnings:?}");
    }

    #[test]
    fn the_leader_moves_the_builtin_mappings_and_frees_ctrl_g() {
        let specs = [spec("<leader>r", "picker.grep", GLOBAL_CONFIG)];
        let (keymap, warnings) = build_keymap(&specs, Some("<C-Space>"), ON);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(keymap.leader(), "<C-Space>");
        assert_eq!(get(&keymap, "<C-Space>ff"), b("picker.files"));
        assert_eq!(get(&keymap, "<C-Space>r"), b("picker.grep"));
        let ctrl_g = parse_keys_on("<C-g>", DEFAULT_LEADER, ON).unwrap_or_default();
        let found = keymap.lookup(&ctrl_g);
        assert!(found.exact.is_none() && !found.is_prefix, "Ctrl+G is free");
        // `del` follows the new leader too.
        let specs = [spec("<leader>h", "-", GLOBAL_CONFIG)];
        let (keymap, warnings) = build_keymap(&specs, Some("<C-Space>"), ON);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(get(&keymap, "<C-Space>h"), None);
    }

    #[test]
    fn a_bad_or_typed_leader_keeps_ctrl_g() {
        for leader in ["<Nope>", " ", ","] {
            let (keymap, warnings) = build_keymap(&[], Some(leader), ON);
            assert_eq!(keymap.leader(), DEFAULT_LEADER, "{leader:?}");
            assert_eq!(warnings.len(), 1, "{leader:?}: {warnings:?}");
            assert!(warnings[0].contains("app.opt.leader"), "{warnings:?}");
            assert_eq!(get(&keymap, "<C-g>ff"), b("picker.files"));
        }
    }

    #[test]
    fn the_same_keys_twice_in_one_file_warn() {
        let specs = [
            spec("<C-k>", "help.open", GLOBAL_CONFIG),
            spec("<C-K>", "panel.tree", GLOBAL_CONFIG),
        ];
        let (keymap, warnings) = build_keymap(&specs, None, ON);
        assert_eq!(get(&keymap, "<C-k>"), b("panel.tree"), "the last one wins");
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(
            warnings[0].contains("mapped twice in global"),
            "{warnings:?}"
        );
    }

    #[test]
    fn two_plugins_on_the_same_keys_warn_but_layers_do_not() {
        let specs = layered(
            vec![
                spec("<C-k>", "help.open", "alpha"),
                spec("<C-k>", "panel.tree", "beta"),
                spec("<C-k>", "picker.files", GLOBAL_CONFIG),
                spec("<C-k>", "picker.grep", WORKSPACE_CONFIG),
            ],
            vec![spec("<C-k>", "model.pick", GLOBAL_TOML)],
            vec![],
        );
        let (keymap, warnings) = build_keymap(&specs, None, ON);
        assert_eq!(get(&keymap, "<C-k>"), b("picker.grep"));
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(
            warnings[0].contains("plugins `alpha` and `beta`"),
            "{warnings:?}"
        );
    }

    #[test]
    fn a_mapping_that_starts_another_warns_once() {
        let specs = [
            spec("<leader>f", "help.open", GLOBAL_CONFIG),
            spec("<C-k>", "help.open", GLOBAL_CONFIG),
            spec("<C-k>a", "panel.tree", GLOBAL_CONFIG),
            spec("<C-k>b", "panel.tree", GLOBAL_CONFIG),
        ];
        let (_, warnings) = build_keymap(&specs, None, ON);
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(
            warnings[0].contains("`<C-g>f` is also the start of `<C-g>f"),
            "{warnings:?}"
        );
        assert!(
            warnings[1].contains("`<C-k>` is also the start of `<C-k>"),
            "{warnings:?}"
        );
    }

    #[test]
    fn unknown_actions_bad_keys_and_typed_keys_are_skipped() {
        let specs = [
            spec("<C-k>", "nada", GLOBAL_CONFIG),
            spec("<Nope>", "help.open", GLOBAL_CONFIG),
            spec("j", "block.next", GLOBAL_CONFIG),
            spec("<C-l>", "fn", GLOBAL_CONFIG),
        ];
        let (keymap, warnings) = build_keymap(&specs, None, ON);
        assert_eq!(warnings.len(), 3, "{warnings:?}");
        assert!(
            warnings[0].contains("unknown action `nada`"),
            "{warnings:?}"
        );
        assert!(warnings[1].contains("<Nope>"), "{warnings:?}");
        assert!(warnings[2].contains("`j`"), "{warnings:?}");
        assert_eq!(get(&keymap, "<C-k>"), None);
        assert_eq!(get(&keymap, "<C-l>"), Some(Action::Callback(1)));
    }

    #[test]
    fn lua_wins_over_toml_within_a_layer_and_the_workspace_over_both() {
        let lua = vec![
            spec("<C-k>", "help.open", "plugin"),
            spec("<C-k>", "panel.tree", GLOBAL_CONFIG),
            spec("<C-l>", "panel.tree", WORKSPACE_CONFIG),
        ];
        let global = vec![
            spec("<C-k>", "picker.files", GLOBAL_TOML),
            spec("<C-j>", "picker.files", GLOBAL_TOML),
        ];
        let workspace = vec![
            spec("<C-l>", "picker.grep", WORKSPACE_TOML),
            spec("<C-j>", "picker.grep", WORKSPACE_TOML),
        ];
        let specs = layered(lua, global, workspace);
        let order: Vec<&str> = specs.iter().map(|s| s.plugin.as_str()).collect();
        assert_eq!(
            order,
            [
                "plugin",
                GLOBAL_TOML,
                GLOBAL_TOML,
                GLOBAL_CONFIG,
                WORKSPACE_TOML,
                WORKSPACE_TOML,
                WORKSPACE_CONFIG
            ]
        );
        let (keymap, warnings) = build_keymap(&specs, None, ON);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(get(&keymap, "<C-k>"), b("panel.tree"));
        assert_eq!(get(&keymap, "<C-l>"), b("panel.tree"));
        assert_eq!(get(&keymap, "<C-j>"), b("picker.grep"));
    }

    #[test]
    fn toml_binds_and_unbinds() {
        let text = r#"
[[bind]]
keys = "<leader>r"
action = "picker.grep"
desc = "Grep"

[[unbind]]
keys = "<C-g>x"
"#;
        let (specs, warnings) = parse_keymap_toml(text, "keymap.toml", KeymapLayer::Workspace);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(
            specs,
            [
                KeymapSpec {
                    lhs: "<C-g>x".into(),
                    action: KeyAction::Unmap,
                    description: None,
                    plugin: WORKSPACE_TOML.into(),
                },
                KeymapSpec {
                    lhs: "<leader>r".into(),
                    action: KeyAction::Builtin("picker.grep".into()),
                    description: Some("Grep".into()),
                    plugin: WORKSPACE_TOML.into(),
                },
            ]
        );
    }

    #[test]
    fn toml_problems_are_warnings() {
        let (specs, warnings) =
            parse_keymap_toml("[[bind]\nkeys = 1", "k.toml", KeymapLayer::Global);
        assert!(specs.is_empty());
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].starts_with("k.toml:"), "{warnings:?}");
        assert!(warnings[0].ends_with("the file is ignored"), "{warnings:?}");

        let text = r#"
leader = "x"
bind = 3
[[unbind]]
keys = 2
[[unbind]]
keys = "<C-k>"
oops = true
"#;
        let (specs, warnings) = parse_keymap_toml(text, "k.toml", KeymapLayer::Global);
        assert_eq!(specs.len(), 1);
        assert_eq!(warnings.len(), 4, "{warnings:?}");
        let all = warnings.join("\n");
        assert!(all.contains("unknown key `leader`"), "{all}");
        assert!(all.contains("`bind` must be a list"), "{all}");
        assert!(
            all.contains("[[unbind]] #1: `keys` must be a string"),
            "{all}"
        );
        assert!(all.contains("[[unbind]] #2: unknown field `oops`"), "{all}");

        let (specs, warnings) = parse_keymap_toml(
            "[[bind]]\nkeys = \"<C-k>\"\n",
            "k.toml",
            KeymapLayer::Global,
        );
        assert!(specs.is_empty());
        assert!(warnings[0].contains("missing `action`"), "{warnings:?}");
    }

    #[test]
    fn sources_have_names_for_people() {
        assert_eq!(source_label(GLOBAL_CONFIG), "global");
        assert_eq!(source_label(""), "global");
        assert_eq!(source_label(WORKSPACE_CONFIG), "workspace");
        assert_eq!(source_label(GLOBAL_TOML), "global keymap.toml");
        assert_eq!(source_label("jira"), "plugin jira");
    }
}
