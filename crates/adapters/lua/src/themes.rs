//! [`ThemeSourcePort`] over a directory of Lua theme files:
//! `<config dir>/themes/<name>.lua`, each returning a table.
//!
//! Every file runs in a fresh sandbox (the same restrictions and limits as
//! config files, plus the `color` helpers) and nothing it defines
//! survives the call. Files whose name starts with `_` (such as
//! `_template.lua`) are not listed.

use std::path::{Path, PathBuf};

use application::ports::theme::{LoadedTheme, ThemeSourceError, ThemeSourcePort};

use crate::convert;
use crate::sandbox::{self, Failure, Limiter};

/// Name of the theme directory inside the config directory.
pub const THEMES_DIR: &str = "themes";

/// Instructions a theme file may run.
const INSTRUCTION_LIMIT: u64 = 10_000_000;
/// Memory a theme file may use.
const MEMORY_LIMIT: usize = 32 * 1024 * 1024;
/// Largest theme file read.
const MAX_FILE_BYTES: u64 = 1024 * 1024;

/// Theme files in one directory.
#[derive(Debug, Clone)]
pub struct LuaThemeSource {
    dir: PathBuf,
}

impl LuaThemeSource {
    /// The themes in `dir`.
    #[must_use]
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// The themes in `<config dir>/themes`, if there is a config dir.
    #[must_use]
    pub fn from_config_dir() -> Option<Self> {
        platform::dirs::config_dir().map(|dir| Self::new(dir.join(THEMES_DIR)))
    }

    /// The directory it reads.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

/// Whether `name` can be a theme file name: no path separators, no
/// leading dot or underscore.
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with(['.', '_'])
        && !name.contains(['/', '\\', ':'])
        && !name.contains("..")
}

impl ThemeSourcePort for LuaThemeSource {
    fn list(&self) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut names: Vec<String> = entries
            .filter_map(Result::ok)
            .filter(|entry| entry.path().is_file())
            .filter_map(|entry| {
                let name = entry.file_name().into_string().ok()?;
                let stem = name.strip_suffix(".lua")?;
                valid_name(stem).then(|| stem.to_ascii_lowercase())
            })
            .collect();
        names.sort();
        names.dedup();
        names
    }

    fn load(&self, name: &str) -> Result<LoadedTheme, ThemeSourceError> {
        if !valid_name(name) {
            return Err(ThemeSourceError::NotFound(name.to_owned()));
        }
        let path = self.dir.join(format!("{name}.lua"));
        let origin = path.display().to_string();
        let broken = |reason: String| ThemeSourceError::Broken(format!("{origin}: {reason}"));
        let size = match std::fs::metadata(&path) {
            Ok(meta) if meta.is_file() => meta.len(),
            Ok(_) | Err(_) => return Err(ThemeSourceError::NotFound(name.to_owned())),
        };
        if size > MAX_FILE_BYTES {
            return Err(broken(format!("larger than {MAX_FILE_BYTES} bytes")));
        }
        let code = std::fs::read_to_string(&path).map_err(|err| broken(err.to_string()))?;
        let table = run(&code, &origin).map_err(broken)?;
        Ok(LoadedTheme { origin, table })
    }
}

/// Runs a theme file and returns the table it returned, as JSON.
fn run(code: &str, origin: &str) -> Result<serde_json::Value, String> {
    let lua = sandbox::new_lua(Some(MEMORY_LIMIT)).map_err(|err| err.to_string())?;
    let limiter = Limiter::install(&lua, INSTRUCTION_LIMIT).map_err(|err| err.to_string())?;
    let color = crate::api::theme::color_table(&lua).map_err(|err| err.to_string())?;
    lua.globals()
        .raw_set("color", color)
        .map_err(|err| err.to_string())?;
    let value = limiter.run(|| {
        let value: mlua::Value = lua
            .load(code)
            .set_name(format!("@{origin}"))
            .set_mode(mlua::prelude::LuaChunkMode::Text)
            .eval()?;
        if !value.is_table() {
            return Err(mlua::Error::runtime(
                "a theme file must return a table (`return { ... }`)",
            ));
        }
        convert::to_json(&lua, value)
    });
    value.map_err(|failure| match failure {
        Failure::InstructionLimit => "instruction limit exceeded".to_owned(),
        Failure::Lua(err) => convert::runtime_error(&err).to_string(),
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn dir_with(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (name, code) in files {
            std::fs::write(dir.path().join(name), code).unwrap();
        }
        dir
    }

    #[test]
    fn lists_and_loads_theme_files() {
        let dir = dir_with(&[
            (
                "night.lua",
                r##"local p = { blue = "#7aa2f7" }
                    return { appearance = "dark", palette = p,
                             ui = { selection = color.alpha(p.blue, 0.2) } }"##,
            ),
            ("_template.lua", "return {}"),
            ("notes.txt", "x"),
        ]);
        let source = LuaThemeSource::new(dir.path());
        assert_eq!(source.list(), ["night"]);
        let loaded = source.load("night").unwrap();
        assert!(loaded.origin.ends_with("night.lua"));
        assert_eq!(
            loaded.table,
            json!({
                "appearance": "dark",
                "palette": { "blue": "#7aa2f7" },
                "ui": { "selection": "#7aa2f733" },
            })
        );
    }

    #[test]
    fn failures() {
        let dir = dir_with(&[
            ("syntax.lua", "return {"),
            ("number.lua", "return 3"),
            ("forever.lua", "while true do end"),
            ("io.lua", "return { x = io.open('/etc/passwd') }"),
        ]);
        let source = LuaThemeSource::new(dir.path());
        assert_eq!(
            source.load("missing"),
            Err(ThemeSourceError::NotFound("missing".into()))
        );
        assert_eq!(
            source.load("../syntax"),
            Err(ThemeSourceError::NotFound("../syntax".into()))
        );
        for (name, expected) in [
            ("syntax", "syntax.lua"),
            ("number", "must return a table"),
            ("forever", "instruction limit exceeded"),
            ("io", "io"),
        ] {
            match source.load(name) {
                Err(ThemeSourceError::Broken(reason)) => {
                    assert!(reason.contains(expected), "{name}: {reason}");
                }
                other => panic!("{name}: {other:?}"),
            }
        }
    }

    #[test]
    fn a_missing_directory_lists_nothing() {
        assert!(LuaThemeSource::new("/nonexistent/crest/themes").list().is_empty());
    }
}
