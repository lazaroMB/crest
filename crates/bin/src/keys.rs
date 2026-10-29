//! The keymap of the window: the builtin one with the mappings of
//! `init.lua` (`app.keymap.set`/`del`) and of `keymap.toml` on top, and
//! `app.opt.timeoutlen`.
//!
//! The rules live in [`application::keymap::layers`]; this module reads the
//! `keymap.toml` files next to the `init.lua` files.

use std::path::Path;
use std::time::Duration;

use application::keymap::layers::{
    KEYMAP_FILE, KeymapLayer, build_keymap, layered, parse_keymap_toml,
};
use application::keymap::{Action, Keymap, Platform};
use application::ports::plugin::{AppOptions, ConfigSources, KeymapSpec};

/// The builtin keymap with `specs` applied in order (see
/// [`application::keymap::layers::build_keymap`]): `app.opt.leader` moves
/// the builtin `<leader>` mappings too. Returns the warnings to show.
#[must_use]
pub fn keymap(specs: &[KeymapSpec], options: &AppOptions) -> (Keymap<Action>, Vec<String>) {
    build_keymap(specs, options.leader.as_deref(), Platform::current())
}

/// `lua` with the mappings of the `keymap.toml` next to each `init.lua` of
/// `sources`, in the order to apply them, and the problems found in those
/// files. A missing file is fine.
#[must_use]
pub fn with_keymap_files(
    lua: Vec<KeymapSpec>,
    sources: &ConfigSources,
) -> (Vec<KeymapSpec>, Vec<String>) {
    let mut warnings = Vec::new();
    let mut read = |init: Option<&Path>, layer: KeymapLayer| {
        let Some(path) = init.and_then(Path::parent).map(|dir| dir.join(KEYMAP_FILE)) else {
            return Vec::new();
        };
        let file = path.display().to_string();
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                let (specs, problems) = parse_keymap_toml(&text, &file, layer);
                warnings.extend(problems);
                specs
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(err) => {
                warnings.push(format!("{file}: cannot read the file: {err}"));
                Vec::new()
            }
        }
    };
    let global = read(sources.global.as_deref(), KeymapLayer::Global);
    let workspace = read(sources.workspace.as_deref(), KeymapLayer::Workspace);
    (layered(lua, global, workspace), warnings)
}

/// `app.opt.timeoutlen`, if set.
#[must_use]
pub fn timeoutlen(options: &AppOptions) -> Option<Duration> {
    options.timeoutlen_ms.map(Duration::from_millis)
}

#[cfg(test)]
mod tests {
    use application::keymap::{DEFAULT_LEADER, Key, parse_keys};
    use application::ports::plugin::{CallbackId, KeyAction};

    use super::*;

    fn spec(lhs: &str, action: KeyAction) -> KeymapSpec {
        KeymapSpec {
            lhs: lhs.into(),
            action,
            description: None,
            plugin: "config".into(),
        }
    }

    fn lookup(keymap: &Keymap<Action>, keys: &[Key]) -> Option<Action> {
        keymap.lookup(keys).exact.cloned()
    }

    #[test]
    fn lua_mappings_go_over_the_defaults() {
        let specs = [
            spec("<leader>x", KeyAction::Callback(CallbackId(3))),
            spec("<leader>k", KeyAction::Builtin("block.last".into())),
            spec("j", KeyAction::Builtin("block.next".into())),
            spec("<Nope>", KeyAction::Builtin("nada".into())),
        ];
        let (keymap, warnings) = keymap(&specs, &AppOptions::default());
        let x = parse_keys("<leader>x", DEFAULT_LEADER).unwrap();
        assert_eq!(lookup(&keymap, &x), Some(Action::Callback(3)));
        let keys = |n: &str| parse_keys(n, DEFAULT_LEADER).unwrap();
        assert_eq!(
            lookup(&keymap, &keys("<leader>k")),
            Some(Action::builtin("block.last"))
        );
        // The rest of the defaults stay.
        assert_eq!(
            lookup(&keymap, &keys("<leader>j")),
            Some(Action::builtin("block.next"))
        );
        assert_eq!(lookup(&keymap, &[Key::char('j')]), None, "j is typed");
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(warnings[0].contains("`j`"), "{warnings:?}");
        assert!(warnings[1].contains("<Nope>"), "{warnings:?}");
    }

    #[test]
    fn the_leader_and_timeoutlen_come_from_app_opt() {
        let options = AppOptions {
            leader: Some("<C-,>".into()),
            timeoutlen_ms: Some(300),
            ..AppOptions::default()
        };
        let specs = [spec("<leader>r", KeyAction::Builtin("picker.files".into()))];
        let (keymap, warnings) = keymap(&specs, &options);
        assert!(warnings.is_empty(), "{warnings:?}");
        let r = parse_keys("<C-,>r", DEFAULT_LEADER).unwrap();
        assert_eq!(lookup(&keymap, &r), Some(Action::builtin("picker.files")));
        // The builtin mappings moved with the leader.
        let h = parse_keys("<C-,>h", DEFAULT_LEADER).unwrap();
        assert_eq!(lookup(&keymap, &h), Some(Action::builtin("help.open")));
        let old = parse_keys("<C-g>h", DEFAULT_LEADER).unwrap();
        assert_eq!(lookup(&keymap, &old), None);
        assert_eq!(timeoutlen(&options), Some(Duration::from_millis(300)));
    }

    #[test]
    fn keymap_files_sit_next_to_each_init_lua() {
        let dir = tempfile::tempdir().unwrap();
        let global = dir.path().join("config");
        let workspace = dir.path().join("ws").join(".crest");
        std::fs::create_dir_all(&global).unwrap();
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(
            global.join(KEYMAP_FILE),
            "[[bind]]\nkeys = \"<C-k>\"\naction = \"help.open\"\n",
        )
        .unwrap();
        std::fs::write(workspace.join(KEYMAP_FILE), "[[unbind]\n").unwrap();
        let sources = ConfigSources {
            global: Some(global.join("init.lua")),
            workspace: Some(workspace.join("init.lua")),
        };
        let lua = vec![spec("<C-k>", KeyAction::Builtin("panel.tree".into()))];
        let (specs, warnings) = with_keymap_files(lua, &sources);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("the file is ignored"), "{warnings:?}");
        let order: Vec<&str> = specs.iter().map(|s| s.plugin.as_str()).collect();
        assert_eq!(order, ["global keymap.toml", "config"], "Lua wins");
        let (keymap, _) = keymap(&specs, &AppOptions::default());
        let k = parse_keys("<C-k>", DEFAULT_LEADER).unwrap();
        assert_eq!(lookup(&keymap, &k), Some(Action::builtin("panel.tree")));
        // No files: nothing changes.
        let none = ConfigSources::default();
        assert_eq!(
            with_keymap_files(Vec::new(), &none),
            (Vec::new(), Vec::new())
        );
    }
}
