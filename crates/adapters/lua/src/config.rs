//! Loading `init.lua` files and plugins into a [`LoadedConfig`].
//!
//! The global config (`$XDG_CONFIG_HOME/crest/init.lua`, by default
//! `~/.config/crest/init.lua`) runs first, then the plugins in
//! `<config dir>/plugins/<name>/init.lua` in alphabetical order, then the
//! workspace config (`<workspace>/.crest/init.lua`). Each file runs in its
//! own environment with its own `app` table.
//!
//! [`LoadedConfig`]: application::ports::plugin::LoadedConfig

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use application::ports::plugin::ConfigSources;

mod api;

/// Directory under the config home that holds crest's files.
const APP_DIR: &str = "crest";
/// Name of every config file.
const INIT_FILE: &str = "init.lua";
/// Workspace directory holding the workspace config.
const WORKSPACE_DIR: &str = ".crest";

/// Config files for `workspace`, following `XDG_CONFIG_HOME` and falling
/// back to `~/.config` on every platform.
#[must_use]
pub fn default_sources(workspace: &Path) -> ConfigSources {
    sources_from(
        std::env::var_os("XDG_CONFIG_HOME"),
        dirs::home_dir(),
        workspace,
    )
}

fn sources_from(xdg: Option<OsString>, home: Option<PathBuf>, workspace: &Path) -> ConfigSources {
    let config_home = xdg
        .map(PathBuf::from)
        // The XDG spec says relative values must be ignored.
        .filter(|p| p.is_absolute())
        .or_else(|| home.map(|h| h.join(".config")));
    ConfigSources {
        global: config_home.map(|dir| dir.join(APP_DIR).join(INIT_FILE)),
        workspace: Some(workspace.join(WORKSPACE_DIR).join(INIT_FILE)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uses_xdg_config_home_when_set() {
        let s = sources_from(
            Some("/xdg".into()),
            Some("/home/u".into()),
            Path::new("/ws"),
        );
        assert_eq!(s.global, Some(PathBuf::from("/xdg/crest/init.lua")));
        assert_eq!(s.workspace, Some(PathBuf::from("/ws/.crest/init.lua")));
    }

    #[test]
    fn falls_back_to_dot_config_in_home() {
        let s = sources_from(None, Some("/home/u".into()), Path::new("/ws"));
        assert_eq!(
            s.global,
            Some(PathBuf::from("/home/u/.config/crest/init.lua"))
        );
    }

    #[test]
    fn ignores_a_relative_xdg_config_home() {
        let s = sources_from(Some("rel".into()), Some("/home/u".into()), Path::new("/ws"));
        assert_eq!(
            s.global,
            Some(PathBuf::from("/home/u/.config/crest/init.lua"))
        );
    }

    #[test]
    fn no_home_means_no_global_config() {
        let s = sources_from(None, None, Path::new("/ws"));
        assert_eq!(s.global, None);
        assert!(s.workspace.is_some());
    }
}
