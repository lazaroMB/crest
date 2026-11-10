//! The `mcp.json` files: the servers the MCP servers manager saves, next
//! to each `init.lua` (`~/.config/crest/mcp.json` for every project,
//! `<workspace>/.crest/mcp.json` for one). What they hold and how they
//! merge with `app.mcp` is [`application::usecases::mcp_servers`]; this
//! module only reads and writes them, readable by their owner alone like
//! the other stores of crest.

use std::io;
use std::path::{Path, PathBuf};

use application::ports::plugin::{ConfigOrigin, ConfigSources, McpServerSpec};
use application::usecases::mcp_servers::McpFile;

/// Name of the file, next to `init.lua`.
pub const FILE: &str = "mcp.json";

/// Where the two `mcp.json` live, when their `init.lua` has a home.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct McpConfigPaths {
    /// The global file.
    pub global: Option<PathBuf>,
    /// The workspace file.
    pub workspace: Option<PathBuf>,
}

impl McpConfigPaths {
    /// The files next to the `init.lua` of `sources`.
    #[must_use]
    pub fn from_sources(sources: &ConfigSources) -> Self {
        let beside = |init: &Option<PathBuf>| {
            init.as_deref()
                .and_then(Path::parent)
                .map(|dir| dir.join(FILE))
        };
        Self {
            global: beside(&sources.global),
            workspace: beside(&sources.workspace),
        }
    }

    /// The file of `origin`.
    #[must_use]
    pub fn of(&self, origin: ConfigOrigin) -> Option<&Path> {
        match origin {
            ConfigOrigin::Global => self.global.as_deref(),
            ConfigOrigin::Workspace => self.workspace.as_deref(),
        }
    }

    /// The servers of both files, global then workspace, and the problems
    /// found reading them.
    #[must_use]
    pub fn load_all(&self) -> (Vec<McpServerSpec>, Vec<McpServerSpec>, Vec<String>) {
        let mut warnings = Vec::new();
        let mut of = |origin| match self.of(origin) {
            Some(path) => {
                let (specs, problems) = load(path, origin);
                warnings.extend(problems);
                specs
            }
            None => Vec::new(),
        };
        let global = of(ConfigOrigin::Global);
        let workspace = of(ConfigOrigin::Workspace);
        (global, workspace, warnings)
    }
}

/// Reads the file at `path`; a missing file holds no servers.
///
/// # Errors
///
/// The file cannot be read or is not an `mcp.json`.
pub fn read(path: &Path) -> Result<McpFile, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => McpFile::parse(&text).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(McpFile::default()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// The servers declared in the file at `path` as `origin`, and the
/// problems: one for the whole file when it cannot be read, else one per
/// entry that cannot be used.
#[must_use]
pub fn load(path: &Path, origin: ConfigOrigin) -> (Vec<McpServerSpec>, Vec<String>) {
    match read(path) {
        Ok(file) => {
            let (specs, problems) = file.into_specs(origin);
            let problems = problems
                .into_iter()
                .map(|p| format!("{}: {p}", path.display()))
                .collect();
            (specs, problems)
        }
        Err(problem) => (
            Vec::new(),
            vec![format!("cannot read the MCP servers of {problem}")],
        ),
    }
}

/// Writes `file` to `path`, creating its directory, readable only by its
/// owner.
///
/// # Errors
///
/// The directory or the file cannot be written.
pub fn save(path: &Path, file: &McpFile) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    crate::mcp::write_private(&tmp, file.render().as_bytes())?;
    std::fs::rename(&tmp, path)
}

/// Reads the file at `path` (empty when missing), applies `change` and
/// writes it back.
///
/// # Errors
///
/// The file cannot be read or written; nothing is written then.
pub fn update(path: &Path, change: impl FnOnce(&mut McpFile)) -> Result<(), String> {
    let mut file = read(path)?;
    change(&mut file);
    save(path, &file).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use application::ports::plugin::McpTransport;

    use super::*;

    fn spec(name: &str) -> McpServerSpec {
        McpServerSpec::new(
            name,
            McpTransport::Stdio {
                command: "npx".into(),
                args: vec!["-y".into()],
                env: BTreeMap::new(),
                cwd: None,
            },
        )
    }

    #[test]
    fn the_files_sit_next_to_each_init_lua() {
        let paths = McpConfigPaths::from_sources(&ConfigSources {
            global: Some(PathBuf::from("/home/u/.config/crest/init.lua")),
            workspace: None,
        });
        assert_eq!(
            paths.global.as_deref(),
            Some(Path::new("/home/u/.config/crest/mcp.json"))
        );
        assert_eq!(paths.workspace, None);
        assert_eq!(paths.of(ConfigOrigin::Workspace), None);
    }

    #[test]
    fn a_missing_file_is_empty_and_a_corrupt_one_warns() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE);
        assert_eq!(load(&path, ConfigOrigin::Global), (Vec::new(), Vec::new()));
        std::fs::write(&path, "nope").unwrap();
        let (specs, warnings) = load(&path, ConfigOrigin::Global);
        assert!(specs.is_empty());
        assert_eq!(warnings.len(), 1);
        assert!(
            warnings[0].contains("cannot read the MCP servers"),
            "{warnings:?}"
        );
        std::fs::write(
            &path,
            r#"{"servers":{"a__b":{"command":"x"},"ok":{"command":"y"}}}"#,
        )
        .unwrap();
        let (specs, warnings) = load(&path, ConfigOrigin::Workspace);
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].origin, ConfigOrigin::Workspace);
        assert!(warnings[0].contains("a__b"), "{warnings:?}");
    }

    #[test]
    fn update_writes_the_file_privately_through_a_temporary_one() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("deep").join(FILE);
        update(&path, |file| file.upsert(&spec("git"))).unwrap();
        update(&path, |file| {
            file.upsert(&spec("docs"));
            assert!(file.remove("git"));
        })
        .unwrap();
        let (specs, warnings) = load(&path, ConfigOrigin::Global);
        assert!(warnings.is_empty());
        let names: Vec<_> = specs.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["docs"]);
        assert!(!path.with_extension("json.tmp").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let paths = McpConfigPaths {
            global: Some(path),
            workspace: Some(dir.path().join("none").join(FILE)),
        };
        let (global, workspace, warnings) = paths.load_all();
        assert_eq!(global.len(), 1);
        assert!(workspace.is_empty());
        assert!(warnings.is_empty());
    }
}
