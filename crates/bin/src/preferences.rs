//! Choices the user makes in the window that outlive it: the provider of
//! the last `/login` and the model picked with `/model` for each provider.
//!
//! They live in `preferences.json` in crest's data directory, written
//! privately like the trust stores. `app.opt.providers.default` and the
//! roles of `init.lua` still win over them (see
//! [`application::providers::effective_default`] and
//! [`domain::model::ModelPolicy`]).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use domain::model::{Account, ModelId, ProviderId};
use serde::{Deserialize, Serialize};

/// Name of the file in crest's data directory.
const FILE: &str = "preferences.json";

/// Default location of the file (see [`platform::dirs::data_dir`]).
#[must_use]
pub fn default_path() -> Option<PathBuf> {
    platform::dirs::data_dir().map(|dir| dir.join(FILE))
}

/// The saved choices.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Preferences {
    /// Provider of the last successful `/login`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<ProviderId>,
    /// Model picked with `/model`, by the provider that was the default
    /// when it was picked.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub models: BTreeMap<ProviderId, ModelId>,
}

impl Preferences {
    /// Reads the file at `path`; a missing file is empty.
    ///
    /// # Errors
    ///
    /// When it cannot be read or is not valid JSON.
    pub fn load(path: &Path) -> Result<Self, String> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                serde_json::from_str(&text).map_err(|err| format!("{}: {err}", path.display()))
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(err) => Err(format!("{}: {err}", path.display())),
        }
    }

    /// Writes the file at `path`, creating its directory.
    ///
    /// # Errors
    ///
    /// When it cannot be written.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let shown = |err: &dyn std::fmt::Display| format!("{}: {err}", path.display());
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|err| shown(&err))?;
        }
        let json = serde_json::to_string_pretty(self).map_err(|err| shown(&err))?;
        crate::mcp::write_private(path, json.as_bytes()).map_err(|err| shown(&err))
    }

    /// The provider a `/login` to `account` makes the default. OpenCode's
    /// key serves Go and Zen; Go is its flat-rate plan.
    #[must_use]
    pub fn provider_of(account: Account) -> ProviderId {
        match account {
            Account::Anthropic => ProviderId::Anthropic,
            Account::OpenCode => ProviderId::OpencodeGo,
        }
    }
}

/// Reads the preferences at `path` (or the default location), applies
/// `change` and saves them.
///
/// # Errors
///
/// When there is no data directory, or the file cannot be read or written.
pub fn update(path: Option<&Path>, change: impl FnOnce(&mut Preferences)) -> Result<(), String> {
    let path = match path {
        Some(path) => path.to_path_buf(),
        None => default_path().ok_or("there is no data directory to save preferences in")?,
    };
    // A corrupt file is replaced rather than blocking every later choice.
    let mut preferences = Preferences::load(&path).unwrap_or_default();
    change(&mut preferences);
    preferences.save(&path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_missing_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("crest").join(FILE);
        assert_eq!(Preferences::load(&path), Ok(Preferences::default()));
        update(Some(&path), |p| {
            p.provider = Some(ProviderId::OpencodeGo);
            p.models
                .insert(ProviderId::OpencodeGo, ModelId::new("opencode-go:glm-5.3"));
        })
        .unwrap();
        let loaded = Preferences::load(&path).unwrap();
        assert_eq!(loaded.provider, Some(ProviderId::OpencodeGo));
        assert_eq!(
            loaded.models[&ProviderId::OpencodeGo].as_str(),
            "opencode-go:glm-5.3"
        );
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"opencode-go\""), "{text}");
    }

    #[test]
    fn a_corrupt_file_is_an_error_and_is_replaced_on_update() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE);
        std::fs::write(&path, "{ not json").unwrap();
        assert!(Preferences::load(&path).is_err());
        update(Some(&path), |p| p.provider = Some(ProviderId::Anthropic)).unwrap();
        assert_eq!(
            Preferences::load(&path).unwrap().provider,
            Some(ProviderId::Anthropic)
        );
    }

    #[test]
    fn logins_map_to_providers() {
        assert_eq!(
            Preferences::provider_of(Account::Anthropic),
            ProviderId::Anthropic
        );
        assert_eq!(
            Preferences::provider_of(Account::OpenCode),
            ProviderId::OpencodeGo
        );
    }
}
