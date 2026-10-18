//! The keymap of the window: the builtin one with the mappings of
//! `init.lua` (`app.keymap.set`) on top, and `app.opt.timeoutlen`.

use std::time::Duration;

use application::keymap::{Action, Keymap, Mode, default_keymap};
use application::ports::plugin::{AppOptions, KeyAction, KeyMode, KeymapSpec};

/// The builtin keymap with `specs` applied in order (a later mapping
/// replaces an earlier one with the same keys). `<leader>` in `specs`
/// expands to `app.opt.leader` when set; the builtin mappings keep the
/// default leader. Invalid mappings are skipped with a warning.
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
        let mode = match spec.mode {
            KeyMode::Insert => Mode::Insert,
            KeyMode::Normal => Mode::Normal,
            KeyMode::Command => Mode::Command,
        };
        let action = match &spec.action {
            KeyAction::Builtin(name) => Action::Builtin(name.clone()),
            KeyAction::Callback(id) => Action::Callback(id.0),
        };
        if let Err(err) = keymap.set(mode, &spec.lhs, action) {
            warnings.push(format!("keymap `{}` ignorado: {err}", spec.lhs));
        }
    }
    (keymap, warnings)
}

/// `app.opt.timeoutlen`, if set.
#[must_use]
pub fn timeoutlen(options: &AppOptions) -> Option<Duration> {
    options.timeoutlen_ms.map(Duration::from_millis)
}

#[cfg(test)]
mod tests {
    use application::keymap::{Key, parse_keys};
    use application::ports::plugin::CallbackId;

    use super::*;

    fn spec(mode: KeyMode, lhs: &str, action: KeyAction) -> KeymapSpec {
        KeymapSpec {
            mode,
            lhs: lhs.into(),
            action,
        }
    }

    fn lookup(keymap: &Keymap<Action>, mode: Mode, keys: &[Key]) -> Option<Action> {
        keymap.lookup(mode, keys).exact.cloned()
    }

    #[test]
    fn lua_mappings_go_over_the_defaults() {
        let specs = [
            spec(
                KeyMode::Normal,
                "<leader>x",
                KeyAction::Callback(CallbackId(3)),
            ),
            spec(
                KeyMode::Normal,
                "j",
                KeyAction::Builtin("block.last".into()),
            ),
            spec(KeyMode::Insert, "<", KeyAction::Builtin("nada".into())),
            spec(KeyMode::Insert, "<Nope>", KeyAction::Builtin("nada".into())),
        ];
        let (keymap, warnings) = keymap(&specs, &AppOptions::default());
        let x = parse_keys("<leader>x", " ").unwrap();
        assert_eq!(lookup(&keymap, Mode::Normal, &x), Some(Action::Callback(3)));
        assert_eq!(
            lookup(&keymap, Mode::Normal, &[Key::char('j')]),
            Some(Action::builtin("block.last"))
        );
        // The rest of the defaults stay.
        assert_eq!(
            lookup(&keymap, Mode::Normal, &[Key::char('k')]),
            Some(Action::builtin("block.prev"))
        );
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(warnings[1].contains("<Nope>"), "{warnings:?}");
    }

    #[test]
    fn the_leader_and_timeoutlen_come_from_app_opt() {
        let options = AppOptions {
            leader: Some(",".into()),
            timeoutlen_ms: Some(300),
            ..AppOptions::default()
        };
        let specs = [spec(
            KeyMode::Normal,
            "<leader>r",
            KeyAction::Builtin("session.resume".into()),
        )];
        let (keymap, warnings) = keymap(&specs, &options);
        assert!(warnings.is_empty(), "{warnings:?}");
        let r = [Key::char(','), Key::char('r')];
        assert_eq!(
            lookup(&keymap, Mode::Normal, &r),
            Some(Action::builtin("session.resume"))
        );
        assert_eq!(timeoutlen(&options), Some(Duration::from_millis(300)));
    }
}
