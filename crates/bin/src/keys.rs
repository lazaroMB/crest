//! The keymap of the window: the builtin one with the mappings of
//! `init.lua` (`app.keymap.set`) on top, and `app.opt.timeoutlen`.

use std::time::Duration;

use application::keymap::{Action, KeyCode, Keymap, default_keymap, parse_keys};
use application::ports::plugin::{AppOptions, KeyAction, KeymapSpec};

/// The builtin keymap with `specs` applied in order (a later mapping
/// replaces an earlier one with the same keys). `<leader>` in `specs`
/// expands to `app.opt.leader` when set; the builtin mappings keep the
/// default leader. Invalid mappings, and mappings that start with a plain
/// printable key (it would be typed into the prompt), are skipped with a
/// warning.
#[must_use]
pub fn keymap(specs: &[KeymapSpec], options: &AppOptions) -> (Keymap<Action>, Vec<String>) {
    let mut keymap = default_keymap();
    let mut warnings = Vec::new();
    if let Some(leader) = &options.leader
        && let Err(err) = keymap.set_leader(leader)
    {
        warnings.push(format!("invalid app.opt.leader `{leader}`: {err}"));
    }
    for spec in specs {
        let action = match &spec.action {
            KeyAction::Builtin(name) => Action::Builtin(name.clone()),
            KeyAction::Callback(id) => Action::Callback(id.0),
        };
        match parse_keys(&spec.lhs, keymap.leader()) {
            Ok(keys) if is_typed(keys[0]) => warnings.push(format!(
                "keymap `{}` ignored: it starts with a key the prompt types; use a chord \
                 (`<C-…>`, `<A-…>`) or `<leader>`",
                spec.lhs
            )),
            Ok(keys) => keymap.set_keys(&keys, action),
            Err(err) => warnings.push(format!("keymap `{}` ignored: {err}", spec.lhs)),
        }
    }
    (keymap, warnings)
}

/// Whether `key` would be typed into the prompt: a character without Ctrl,
/// Alt or Cmd.
fn is_typed(key: application::keymap::Key) -> bool {
    matches!(key.code, KeyCode::Char(_)) && !(key.mods.ctrl || key.mods.alt || key.mods.cmd)
}

/// `app.opt.timeoutlen`, if set.
#[must_use]
pub fn timeoutlen(options: &AppOptions) -> Option<Duration> {
    options.timeoutlen_ms.map(Duration::from_millis)
}

#[cfg(test)]
mod tests {
    use application::keymap::{DEFAULT_LEADER, Key, parse_keys};
    use application::ports::plugin::CallbackId;

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
            spec("<A-Up>", KeyAction::Builtin("block.last".into())),
            spec("j", KeyAction::Builtin("block.next".into())),
            spec("<Nope>", KeyAction::Builtin("nada".into())),
        ];
        let (keymap, warnings) = keymap(&specs, &AppOptions::default());
        let x = parse_keys("<leader>x", DEFAULT_LEADER).unwrap();
        assert_eq!(lookup(&keymap, &x), Some(Action::Callback(3)));
        let alt = |n: &str| parse_keys(n, DEFAULT_LEADER).unwrap();
        assert_eq!(
            lookup(&keymap, &alt("<A-Up>")),
            Some(Action::builtin("block.last"))
        );
        // The rest of the defaults stay.
        assert_eq!(
            lookup(&keymap, &alt("<A-Down>")),
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
        assert_eq!(timeoutlen(&options), Some(Duration::from_millis(300)));
    }
}
