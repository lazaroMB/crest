//! Which plugin capabilities are active, and the store of approved ones.
//!
//! A plugin (or the global or workspace `init.lua`) declares what it may do
//! beyond the Lua sandbox with `app.plugin{ capabilities = ... }`: HTTP to
//! some hosts, running some programs, reading its secrets. Nothing is
//! active until the user approves the declaration once, with
//! `crest --trust-plugin <name>` or `/plugin trust <name>`.
//!
//! An approval covers one exact declaration: the plugin's name, the file
//! that declared it (for the workspace config that path holds the
//! workspace root), its layer and its capability list. Any change asks
//! again. The store is `$XDG_DATA_HOME/crest/plugin-trust.json` (by default
//! `~/.local/share/crest/`), written readable only by its owner; deleting
//! an entry revokes it.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use application::ports::plugin::{ConfigOrigin, PluginCapabilities, PluginHostPort};
use serde::{Deserialize, Serialize};

use crate::mcp::write_private;

/// Name of the store file in crest's data directory.
const TRUST_FILE: &str = "plugin-trust.json";

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

/// Approved capability declarations.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginTrustStore {
    #[serde(default)]
    plugins: Vec<PluginCapabilities>,
}

impl PluginTrustStore {
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

    /// Whether `decl` was approved exactly as it is now.
    #[must_use]
    pub fn is_trusted(&self, decl: &PluginCapabilities) -> bool {
        self.plugins.contains(decl)
    }

    /// Approves `decl`, replacing an earlier approval of the same plugin
    /// from the same file.
    pub fn trust(&mut self, decl: &PluginCapabilities) {
        self.plugins.retain(|p| !same_plugin(p, decl));
        self.plugins.push(decl.clone());
    }
}

fn same_plugin(a: &PluginCapabilities, b: &PluginCapabilities) -> bool {
    a.plugin == b.plugin && a.source == b.source && a.origin == b.origin
}

/// Declarations split by whether they may run.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Admission {
    /// Approved declarations, to grant.
    pub granted: Vec<PluginCapabilities>,
    /// Declarations waiting for approval.
    pub pending: Vec<PluginCapabilities>,
    /// Declarations approved now (`--trust-plugin`), to save.
    pub newly_trusted: Vec<PluginCapabilities>,
}

/// Decides which of `decls` are granted: those approved before as they
/// are and those of the plugins named in `trust_now`.
#[must_use]
pub fn admit(
    decls: &[PluginCapabilities],
    store: &PluginTrustStore,
    trust_now: &[String],
) -> Admission {
    let mut out = Admission::default();
    for decl in decls {
        if store.is_trusted(decl) {
            out.granted.push(decl.clone());
        } else if trust_now.contains(&decl.plugin) {
            out.newly_trusted.push(decl.clone());
            out.granted.push(decl.clone());
        } else {
            out.pending.push(decl.clone());
        }
    }
    out
}

fn label(decl: &PluginCapabilities) -> String {
    match decl.origin {
        ConfigOrigin::Global => format!("plugin `{}`", decl.plugin),
        ConfigOrigin::Workspace => format!("workspace plugin `{}`", decl.plugin),
    }
}

/// What a declaration waiting for approval looks like to the user.
#[must_use]
pub fn pending_notice(decl: &PluginCapabilities) -> String {
    format!(
        "{} requests {} — approve with `crest --trust-plugin {name}` or `/plugin trust {name}`",
        label(decl),
        decl.summary(),
        name = decl.plugin
    )
}

/// Where the store lives and what the command line approved.
#[derive(Debug, Clone, Default)]
pub struct TrustOptions {
    /// Store file; `None` keeps no approvals.
    pub store: Option<PathBuf>,
    /// `--trust-plugin` names.
    pub trust_now: Vec<String>,
}

/// Grants the approved declarations of `decls` in `host` and saves the
/// ones approved on this run. Returns what was granted and warnings: the
/// pending declarations, unknown `--trust-plugin` names and store
/// problems.
pub async fn start(
    host: Option<&dyn PluginHostPort>,
    decls: &[PluginCapabilities],
    trust: &TrustOptions,
) -> (Vec<PluginCapabilities>, Vec<String>) {
    let mut warnings = Vec::new();
    let mut store = match trust.store.as_deref().map(PluginTrustStore::load) {
        Some(Ok(store)) => store,
        Some(Err(err)) => {
            warnings.push(format!(
                "cannot read the approved plugin capabilities: {err}"
            ));
            PluginTrustStore::default()
        }
        None => PluginTrustStore::default(),
    };
    for name in &trust.trust_now {
        if !decls.iter().any(|d| &d.plugin == name) {
            warnings.push(format!(
                "--trust-plugin {name}: no plugin with that name declares capabilities"
            ));
        }
    }
    let admission = admit(decls, &store, &trust.trust_now);
    warnings.extend(admission.pending.iter().map(pending_notice));
    if !admission.newly_trusted.is_empty() {
        for decl in &admission.newly_trusted {
            store.trust(decl);
        }
        if let Some(path) = &trust.store
            && let Err(err) = store.save(path)
        {
            warnings.push(format!(
                "cannot save the plugin approvals to {}: {err}",
                path.display()
            ));
        }
    }
    if let Some(host) = host
        && let Err(err) = host.grant_capabilities(admission.granted.clone()).await
    {
        warnings.push(format!("cannot activate the plugin capabilities: {err}"));
        return (Vec::new(), warnings);
    }
    (admission.granted, warnings)
}

/// `/plugin trust` and `/plugin list` over the declarations of a session.
pub struct PluginTrust {
    /// Every declaration of the loaded config.
    pub decls: Vec<PluginCapabilities>,
    /// Store file, if any.
    pub store: Option<PathBuf>,
    /// Declarations granted now.
    pub granted: Arc<Mutex<Vec<PluginCapabilities>>>,
    /// The Lua host, to grant new approvals at once.
    pub host: Option<Arc<dyn PluginHostPort>>,
}

impl PluginTrust {
    fn granted(&self) -> Vec<PluginCapabilities> {
        self.granted
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Whether `decl` is granted now.
    #[must_use]
    pub fn is_granted(&self, decl: &PluginCapabilities) -> bool {
        self.granted().contains(decl)
    }

    /// `/plugin list`: every declaration and whether it is active.
    #[must_use]
    pub fn list(&self) -> String {
        if self.decls.is_empty() {
            return "no plugin declares capabilities".into();
        }
        let granted = self.granted();
        let lines: Vec<String> = self
            .decls
            .iter()
            .map(|d| {
                let state = if granted.contains(d) {
                    "approved"
                } else {
                    "pending"
                };
                format!(
                    "{} ({}): {} [{state}]",
                    label(d),
                    d.source.display(),
                    d.summary()
                )
            })
            .collect();
        lines.join("\n")
    }

    /// `/plugin trust <name>`: approves, saves and activates every pending
    /// declaration of `name`. Returns what to tell the user.
    ///
    /// # Errors
    ///
    /// No declaration of that name, no store, or the store cannot be read
    /// or written.
    pub async fn trust(&self, name: &str) -> Result<String, String> {
        let wanted: Vec<PluginCapabilities> = self
            .decls
            .iter()
            .filter(|d| d.plugin == name)
            .cloned()
            .collect();
        if wanted.is_empty() {
            return Err(format!("no plugin `{name}` declares capabilities"));
        }
        let mut granted = self.granted();
        let new: Vec<PluginCapabilities> = wanted
            .into_iter()
            .filter(|d| !granted.contains(d))
            .collect();
        if new.is_empty() {
            return Ok(format!(
                "the capabilities of plugin `{name}` were already approved"
            ));
        }
        let path = self.store.clone().ok_or_else(|| {
            "nowhere to save the approval (no data directory); use --trust-plugin".to_owned()
        })?;
        let to_save = new.clone();
        tokio::task::spawn_blocking(move || {
            let mut store = PluginTrustStore::load(&path)
                .map_err(|e| format!("cannot read the approved plugin capabilities: {e}"))?;
            for decl in &to_save {
                store.trust(decl);
            }
            store
                .save(&path)
                .map_err(|e| format!("cannot save the approval to {}: {e}", path.display()))
        })
        .await
        .map_err(|e| e.to_string())??;
        let summary: Vec<String> = new.iter().map(PluginCapabilities::summary).collect();
        granted.extend(new);
        let active = match &self.host {
            Some(host) => host.grant_capabilities(granted.clone()).await.is_ok(),
            None => false,
        };
        *self.granted.lock().unwrap_or_else(PoisonError::into_inner) = granted;
        let when = if active {
            "they are active now"
        } else {
            "they will be active the next time you open crest"
        };
        Ok(format!(
            "plugin `{name}` approved ({}); {when}",
            summary.join("; ")
        ))
    }
}

#[cfg(test)]
mod tests {
    use application::ports::plugin::Capability;

    use super::*;

    fn decl(plugin: &str, caps: &[&str], origin: ConfigOrigin) -> PluginCapabilities {
        PluginCapabilities {
            plugin: plugin.into(),
            source: PathBuf::from(format!("/c/plugins/{plugin}/init.lua")),
            origin,
            capabilities: caps.iter().map(|c| Capability::parse(c).unwrap()).collect(),
        }
    }

    #[test]
    fn the_store_round_trips_through_a_private_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("deep").join(TRUST_FILE);
        assert_eq!(
            PluginTrustStore::load(&path).unwrap(),
            PluginTrustStore::default()
        );
        let mut store = PluginTrustStore::default();
        store.trust(&decl(
            "jira",
            &["network:jira.example.com"],
            ConfigOrigin::Global,
        ));
        store.save(&path).unwrap();
        assert_eq!(PluginTrustStore::load(&path).unwrap(), store);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("network:jira.example.com"), "{text}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn changed_capabilities_ask_again() {
        let old = decl("jira", &["network:jira.example.com"], ConfigOrigin::Global);
        let mut store = PluginTrustStore::default();
        store.trust(&old);
        assert_eq!(
            admit(std::slice::from_ref(&old), &store, &[]).granted,
            std::slice::from_ref(&old)
        );

        let more = decl(
            "jira",
            &["network:jira.example.com", "shell:jq"],
            ConfigOrigin::Global,
        );
        let got = admit(std::slice::from_ref(&more), &store, &[]);
        assert!(got.granted.is_empty());
        assert_eq!(got.pending, std::slice::from_ref(&more));

        let moved = PluginCapabilities {
            source: PathBuf::from("/elsewhere/init.lua"),
            ..old.clone()
        };
        assert_eq!(
            admit(std::slice::from_ref(&moved), &store, &[]).pending,
            [moved]
        );

        store.trust(&more);
        assert!(store.is_trusted(&more) && !store.is_trusted(&old));
    }

    #[test]
    fn trusting_now_grants_and_asks_to_save() {
        let d = decl("jira", &["shell:jq"], ConfigOrigin::Workspace);
        let got = admit(
            std::slice::from_ref(&d),
            &PluginTrustStore::default(),
            &["jira".into()],
        );
        assert_eq!(got.granted, std::slice::from_ref(&d));
        assert_eq!(got.newly_trusted, [d]);
    }

    #[test]
    fn the_notice_lists_the_capabilities_and_how_to_approve() {
        let msg = pending_notice(&decl(
            "jira",
            &["network:jira.example.com", "shell:jq"],
            ConfigOrigin::Global,
        ));
        assert_eq!(
            msg,
            "plugin `jira` requests network: jira.example.com; shell: jq — approve with \
             `crest --trust-plugin jira` or `/plugin trust jira`"
        );
        let ws = pending_notice(&decl("repo", &["shell:make"], ConfigOrigin::Workspace));
        assert!(ws.starts_with("workspace plugin `repo`"), "{ws}");
    }

    #[tokio::test]
    async fn plugin_trust_saves_and_lists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(TRUST_FILE);
        let d = decl("jira", &["shell:jq"], ConfigOrigin::Global);
        let trust = PluginTrust {
            decls: vec![d.clone()],
            store: Some(path.clone()),
            granted: Arc::default(),
            host: None,
        };
        assert!(trust.list().contains("[pending]"));
        let said = trust.trust("jira").await.unwrap();
        assert!(said.contains("next time"), "{said}");
        assert!(PluginTrustStore::load(&path).unwrap().is_trusted(&d));
        assert!(trust.list().contains("[approved]"));
        let again = trust.trust("jira").await.unwrap();
        assert!(again.contains("already approved"), "{again}");
        assert!(trust.trust("nope").await.is_err());
    }

    #[test]
    fn the_store_follows_xdg_data_home() {
        assert_eq!(
            trust_path_from(Some("/data".into()), None),
            Some(PathBuf::from("/data/crest/plugin-trust.json"))
        );
        assert_eq!(trust_path_from(None, None), None);
    }
}
