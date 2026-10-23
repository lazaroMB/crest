//! Choices the user makes in the window that outlive it: the provider of
//! the last `/login`, the model picked with `/model` for each provider and
//! which OpenCode plans the key may use.
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

use crate::opencode::PlanAccess;

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
    /// What the stored OpenCode key may use; `None` until it is asked
    /// (see [`crate::opencode::probe_access`]), and again after a `/login`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opencode_access: Option<PlanAccess>,
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
    /// key serves Go and Zen; Go, its flat-rate plan, until the next start
    /// asks which plans the key may use (see [`Self::opencode_plan`]).
    #[must_use]
    pub fn provider_of(account: Account) -> ProviderId {
        match account {
            Account::Anthropic => ProviderId::Anthropic,
            Account::OpenCode => ProviderId::OpencodeGo,
        }
    }

    /// The OpenCode plan to use instead of `preferred` when `access` says
    /// the other one works and `preferred` is ruled out or not known to
    /// work: Zen instead of Go without a subscription, and the other way
    /// round.
    #[must_use]
    pub fn opencode_plan(preferred: ProviderId, access: &PlanAccess) -> ProviderId {
        let other = match preferred {
            ProviderId::OpencodeGo => ProviderId::OpencodeZen,
            ProviderId::OpencodeZen => ProviderId::OpencodeGo,
            other => return other,
        };
        let give_way = !access.works(preferred)
            && access.serves(other)
            && (access.works(other) || !access.serves(preferred));
        if give_way { other } else { preferred }
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
    fn a_plan_the_key_cannot_use_gives_way_to_the_other() {
        let no_go = PlanAccess {
            go: Some(false),
            zen: Some(true),
            ..PlanAccess::default()
        };
        assert_eq!(
            Preferences::opencode_plan(ProviderId::OpencodeGo, &no_go),
            ProviderId::OpencodeZen
        );
        let nothing = PlanAccess {
            go: Some(false),
            zen: Some(false),
            zen_free: Some(false),
            ..PlanAccess::default()
        };
        assert_eq!(
            Preferences::opencode_plan(ProviderId::OpencodeGo, &nothing),
            ProviderId::OpencodeGo
        );
        assert_eq!(
            Preferences::opencode_plan(ProviderId::OpencodeGo, &PlanAccess::default()),
            ProviderId::OpencodeGo
        );
        // Go could not be asked, Zen answered: Zen.
        let zen_only = PlanAccess {
            zen: Some(true),
            zen_free: Some(false),
            ..PlanAccess::default()
        };
        assert_eq!(
            Preferences::opencode_plan(ProviderId::OpencodeGo, &zen_only),
            ProviderId::OpencodeZen
        );
        // The file of 0.12.1 still loads.
        let old: Preferences = serde_json::from_str(
            r#"{"provider":"opencode-go","models":{"opencode-go":"beam-free"}}"#,
        )
        .unwrap();
        assert_eq!(old.opencode_access, None);
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
