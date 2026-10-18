//! Which MCP servers may start, and the store of approved workspace
//! servers.
//!
//! Servers of the global config (and its plugins) start right away: the
//! user wrote them. Servers of `<workspace>/.crest/init.lua` come with the
//! repository, and starting one runs a program or contacts a URL chosen by
//! whoever wrote that repository. They stay pending, with a warning, until
//! the user approves them once with `crest --trust-mcp <name>`.
//!
//! An approval is stored per workspace (its canonical root) together with
//! the exact transport (command, arguments, environment, URL, headers) that
//! was approved, so a later change to the declaration asks again. The store
//! is `$XDG_DATA_HOME/crest/mcp-trust.json` (by default
//! `~/.local/share/crest/`); deleting an entry revokes it. The file holds
//! the declared values, header values included, so it is written readable
//! only by its owner.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

use application::ports::plugin::{ConfigOrigin, McpServerSpec, McpTransport};
use serde::{Deserialize, Serialize};

/// Name of the store file in crest's data directory.
const TRUST_FILE: &str = "mcp-trust.json";

/// Default location of the store, following `XDG_DATA_HOME`.
#[must_use]
pub fn default_trust_path() -> Option<PathBuf> {
    trust_path_from(std::env::var_os("XDG_DATA_HOME"), dirs::home_dir())
}

fn trust_path_from(xdg_data_home: Option<OsString>, home: Option<PathBuf>) -> Option<PathBuf> {
    let data_home = xdg_data_home
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| home.map(|h| h.join(".local").join("share")))?;
    Some(data_home.join("crest").join(TRUST_FILE))
}

/// Approved workspace servers, by workspace root and server name.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustStore {
    #[serde(default)]
    workspaces: BTreeMap<String, BTreeMap<String, McpTransport>>,
}

impl TrustStore {
    /// Reads the store at `path`; a missing file is an empty store.
    ///
    /// # Errors
    ///
    /// The file cannot be read or is not a valid store.
    pub fn load(path: &Path) -> io::Result<Self> {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e),
        }
    }

    /// Writes the store to `path`, creating its directory.
    ///
    /// # Errors
    ///
    /// The file cannot be written.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        let tmp = path.with_extension("json.tmp");
        write_private(&tmp, &json)?;
        std::fs::rename(&tmp, path)
    }

    /// Whether `spec`, declared in `workspace`, was approved as it is now.
    #[must_use]
    pub fn is_trusted(&self, workspace: &Path, spec: &McpServerSpec) -> bool {
        self.workspaces
            .get(&key(workspace))
            .and_then(|servers| servers.get(&spec.name))
            .is_some_and(|approved| approved == &spec.transport)
    }

    /// Approves `spec` for `workspace`, replacing an earlier approval.
    pub fn trust(&mut self, workspace: &Path, spec: &McpServerSpec) {
        self.workspaces
            .entry(key(workspace))
            .or_default()
            .insert(spec.name.clone(), spec.transport.clone());
    }
}

fn key(workspace: &Path) -> String {
    workspace.display().to_string()
}

#[cfg(unix)]
fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)
}

#[cfg(not(unix))]
fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    std::fs::write(path, bytes)
}

/// Servers split into those to start and those held back.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Admission {
    /// Servers to start.
    pub start: Vec<McpServerSpec>,
    /// Workspace servers waiting for approval.
    pub pending: Vec<McpServerSpec>,
    /// Workspace servers approved now (`--trust-mcp`), to save.
    pub newly_trusted: Vec<McpServerSpec>,
}

/// Decides which of `specs` start in `workspace`: global ones, workspace
/// ones approved before as they are, and those named in `trust_now`.
#[must_use]
pub fn admit(
    specs: Vec<McpServerSpec>,
    workspace: &Path,
    store: &TrustStore,
    trust_now: &[String],
) -> Admission {
    let mut out = Admission::default();
    for spec in specs {
        if spec.origin == ConfigOrigin::Global || store.is_trusted(workspace, &spec) {
            out.start.push(spec);
        } else if trust_now.contains(&spec.name) {
            out.newly_trusted.push(spec.clone());
            out.start.push(spec);
        } else {
            out.pending.push(spec);
        }
    }
    out
}

/// What a held-back server looks like to the user.
#[must_use]
pub fn pending_warning(spec: &McpServerSpec) -> String {
    let what = match &spec.transport {
        McpTransport::Stdio { command, args, .. } => {
            let mut line = command.clone();
            for arg in args {
                line.push(' ');
                line.push_str(arg);
            }
            format!("ejecutaría `{line}`")
        }
        McpTransport::Http { url, .. } => format!("conectaría con {url}"),
    };
    format!(
        "servidor MCP `{name}` del workspace no iniciado: {what}. Si confías en él, \
         arranca con `crest --trust-mcp {name}` (se recuerda para este workspace)",
        name = spec.name
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stdio(name: &str, command: &str, origin: ConfigOrigin) -> McpServerSpec {
        McpServerSpec {
            origin,
            ..McpServerSpec::new(
                name,
                McpTransport::Stdio {
                    command: command.into(),
                    args: vec!["--x".into()],
                    env: BTreeMap::new(),
                },
            )
        }
    }

    #[test]
    fn global_servers_start_and_workspace_ones_wait() {
        let ws = Path::new("/ws");
        let specs = vec![
            stdio("mine", "a", ConfigOrigin::Global),
            stdio("repo", "b", ConfigOrigin::Workspace),
        ];
        let got = admit(specs.clone(), ws, &TrustStore::default(), &[]);
        assert_eq!(got.start, [specs[0].clone()]);
        assert_eq!(got.pending, [specs[1].clone()]);
        assert!(got.newly_trusted.is_empty());
    }

    #[test]
    fn trusting_now_starts_it_and_asks_to_save() {
        let ws = Path::new("/ws");
        let repo = stdio("repo", "b", ConfigOrigin::Workspace);
        let got = admit(
            vec![repo.clone()],
            ws,
            &TrustStore::default(),
            &["repo".into()],
        );
        assert_eq!(got.start, std::slice::from_ref(&repo));
        assert_eq!(got.newly_trusted, [repo]);
    }

    #[test]
    fn an_approval_covers_only_that_workspace_and_that_transport() {
        let repo = stdio("repo", "b", ConfigOrigin::Workspace);
        let mut store = TrustStore::default();
        store.trust(Path::new("/ws"), &repo);
        assert!(store.is_trusted(Path::new("/ws"), &repo));
        assert!(!store.is_trusted(Path::new("/other"), &repo));
        let changed = stdio("repo", "evil", ConfigOrigin::Workspace);
        assert!(!store.is_trusted(Path::new("/ws"), &changed));
        let got = admit(vec![changed.clone()], Path::new("/ws"), &store, &[]);
        assert_eq!(got.pending, [changed]);
    }

    #[test]
    fn the_store_round_trips_through_its_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("deep").join(TRUST_FILE);
        assert_eq!(TrustStore::load(&path).unwrap(), TrustStore::default());
        let mut store = TrustStore::default();
        store.trust(
            Path::new("/ws"),
            &stdio("repo", "b", ConfigOrigin::Workspace),
        );
        store.save(&path).unwrap();
        assert_eq!(TrustStore::load(&path).unwrap(), store);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn a_corrupt_store_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(TRUST_FILE);
        std::fs::write(&path, "nope").unwrap();
        assert!(TrustStore::load(&path).is_err());
    }

    #[test]
    fn the_warning_says_what_would_run_and_how_to_allow_it() {
        let msg = pending_warning(&stdio("repo", "b", ConfigOrigin::Workspace));
        assert!(msg.contains("`b --x`"), "{msg}");
        assert!(msg.contains("crest --trust-mcp repo"), "{msg}");
    }

    #[test]
    fn the_store_follows_xdg_data_home() {
        assert_eq!(
            trust_path_from(Some("/data".into()), Some("/home/u".into())),
            Some(PathBuf::from("/data/crest/mcp-trust.json"))
        );
        assert_eq!(
            trust_path_from(Some("rel".into()), Some("/home/u".into())),
            Some(PathBuf::from("/home/u/.local/share/crest/mcp-trust.json"))
        );
        assert_eq!(trust_path_from(None, None), None);
    }
}
