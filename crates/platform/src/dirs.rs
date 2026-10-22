//! crest's per-user directories.
//!
//! `XDG_CONFIG_HOME` and `XDG_DATA_HOME` win on every platform when set to
//! an absolute path (the XDG spec ignores relative values). Otherwise Unix
//! (macOS included) uses the XDG defaults `~/.config` and `~/.local/share`,
//! as crest always has, and Windows its native folders: `%APPDATA%` for the
//! config and `%LOCALAPPDATA%` for the data. crest's files go in a `crest`
//! directory under each.

use std::ffi::OsString;
use std::path::PathBuf;

/// The directory under the config and data homes holding crest's files.
pub const APP_DIR: &str = "crest";

/// crest's config directory (`init.lua`, plugins): `~/.config/crest` or
/// `%APPDATA%\crest`. `None` without a home directory.
#[must_use]
pub fn config_dir() -> Option<PathBuf> {
    pick(std::env::var_os("XDG_CONFIG_HOME"), native_config_home()).map(|d| d.join(APP_DIR))
}

/// crest's data directory (sessions, trust stores):
/// `~/.local/share/crest` or `%LOCALAPPDATA%\crest`. `None` without a home
/// directory.
#[must_use]
pub fn data_dir() -> Option<PathBuf> {
    pick(std::env::var_os("XDG_DATA_HOME"), native_data_home()).map(|d| d.join(APP_DIR))
}

/// The user's home directory (`$HOME`, `%USERPROFILE%`).
#[must_use]
pub fn home_dir() -> Option<PathBuf> {
    ::dirs::home_dir()
}

fn native_config_home() -> Option<PathBuf> {
    if cfg!(windows) {
        ::dirs::config_dir()
    } else {
        home_dir().map(|h| h.join(".config"))
    }
}

fn native_data_home() -> Option<PathBuf> {
    if cfg!(windows) {
        ::dirs::data_local_dir()
    } else {
        home_dir().map(|h| h.join(".local").join("share"))
    }
}

/// The XDG variable when it is an absolute path, else `native`.
fn pick(xdg: Option<OsString>, native: Option<PathBuf>) -> Option<PathBuf> {
    xdg.map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or(native)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn absolute(name: &str) -> PathBuf {
        std::env::temp_dir().join(name)
    }

    #[test]
    fn an_absolute_xdg_variable_wins() {
        let xdg = absolute("xdg");
        assert_eq!(
            pick(Some(xdg.clone().into()), Some(absolute("native"))),
            Some(xdg)
        );
    }

    #[test]
    fn empty_or_relative_xdg_values_are_ignored() {
        for xdg in [None, Some("".into()), Some("relative/dir".into())] {
            assert_eq!(
                pick(xdg, Some(absolute("native"))),
                Some(absolute("native"))
            );
        }
        assert_eq!(pick(None, None), None);
    }

    #[cfg(unix)]
    #[test]
    fn unix_defaults_follow_xdg() {
        let home = home_dir().unwrap();
        assert_eq!(native_config_home(), Some(home.join(".config")));
        assert_eq!(native_data_home(), Some(home.join(".local/share")));
    }

    #[cfg(windows)]
    #[test]
    fn windows_defaults_are_the_app_data_folders() {
        assert_eq!(native_config_home(), ::dirs::config_dir());
        assert_eq!(native_data_home(), ::dirs::data_local_dir());
    }
}
