//! Startup steps that only read configuration: the Lua config, the
//! sensitive path rules and the model policy validated against the
//! catalog. Each one degrades to defaults and returns warnings instead of
//! failing, so a broken `init.lua` or an unreachable API never stops crest.

use std::time::Duration;

use application::ports::llm::ModelCatalogPort;
use application::ports::plugin::{ConfigSources, LoadedConfig, PluginDiagnostic, PluginHostPort};
use domain::block::{Block, BlockKind};
use domain::model::{ModelId, ModelPolicy};
use domain::workspace::SensitivePathRules;

/// How long the catalog may take before startup goes on without it.
pub const CATALOG_TIMEOUT: Duration = Duration::from_secs(5);

/// Loads the global and workspace `init.lua` through `host`. Problems in
/// single files stay in [`LoadedConfig::errors`] (each one an `Error`
/// block, see [`config_error_block`]); if loading fails altogether the
/// defaults are used with a warning.
pub async fn load_config(
    host: Option<&dyn PluginHostPort>,
    sources: ConfigSources,
) -> (LoadedConfig, Vec<String>) {
    let Some(host) = host else {
        return (LoadedConfig::default(), Vec::new());
    };
    match host.load_config(sources).await {
        Ok(config) => (config, Vec::new()),
        Err(err) => (
            LoadedConfig::default(),
            vec![format!("config: {err}; using the defaults")],
        ),
    }
}

/// `file:line: message`, or `file: message` without a line.
#[must_use]
pub fn diagnostic(d: &PluginDiagnostic) -> String {
    match d.line {
        Some(line) => format!("{}:{line}: {}", d.source, d.message),
        None => format!("{}: {}", d.source, d.message),
    }
}

/// The `Error` block that shows a problem of a config file: crest goes on
/// without what that file defines.
#[must_use]
pub fn config_error_block(d: &PluginDiagnostic) -> Block {
    Block::new(BlockKind::Error {
        message: diagnostic(d),
        hint: Some(
            "crest started without what that file defines (using the defaults); fix it \
             and reopen crest"
                .into(),
        ),
    })
}

/// The sensitive path rules of `config`, or the defaults (with a warning)
/// when it has none or they are invalid.
pub fn sensitive_rules(config: &LoadedConfig) -> (SensitivePathRules, Option<String>) {
    match &config.sensitive_paths {
        None => (SensitivePathRules::default(), None),
        Some(patterns) => match SensitivePathRules::new(patterns.iter().cloned()) {
            Ok(rules) => (rules, None),
            Err(err) => (
                SensitivePathRules::default(),
                Some(format!(
                    "invalid sensitive paths: {err}; using the defaults"
                )),
            ),
        },
    }
}

/// Checks the roles of `policy` against the models of `catalog`.
///
/// When the catalog answers within `timeout`, the policy learns the
/// available models (so unavailable ids resolve to their fallbacks) and
/// every invalid role is a warning. Otherwise the policy is returned as is
/// with one warning saying it could not be validated.
pub async fn validate_models(
    policy: ModelPolicy,
    catalog: &dyn ModelCatalogPort,
    timeout: Duration,
) -> (ModelPolicy, Vec<String>) {
    match tokio::time::timeout(timeout, catalog.list_models()).await {
        Ok(Ok(models)) => {
            let ids: Vec<ModelId> = models.into_iter().map(|m| m.id).collect();
            let warnings = policy
                .validate_against(&ids)
                .iter()
                .map(ToString::to_string)
                .collect();
            (policy.with_available_models(&ids), warnings)
        }
        Ok(Err(err)) => (
            policy,
            vec![format!("could not validate the configured models: {err}")],
        ),
        Err(_) => (
            policy,
            vec![format!(
                "could not validate the configured models: /v1/models did not answer in {} s",
                timeout.as_secs()
            )],
        ),
    }
}

#[cfg(test)]
mod tests {
    use application::ports::llm::LlmError;
    use application::testing::llm::FakeModelCatalog;
    use domain::model::{ModelRole, ResolveRequest, default_spec};

    use super::*;

    #[tokio::test]
    async fn without_a_host_the_defaults_are_used() {
        let (config, warnings) = load_config(None, ConfigSources::default()).await;
        assert_eq!(config, LoadedConfig::default());
        assert!(warnings.is_empty());
    }

    #[test]
    fn diagnostics_mention_file_and_line() {
        let d = PluginDiagnostic {
            source: "init.lua".into(),
            message: "syntax error".into(),
            line: Some(3),
        };
        assert_eq!(diagnostic(&d), "init.lua:3: syntax error");
        let block = config_error_block(&d);
        assert!(
            matches!(&block.kind, BlockKind::Error { message, hint: Some(_) } if message == "init.lua:3: syntax error"),
            "{block:?}"
        );
    }

    #[test]
    fn invalid_sensitive_paths_fall_back_to_the_defaults() {
        let mut config = LoadedConfig::default();
        assert_eq!(
            sensitive_rules(&config),
            (SensitivePathRules::default(), None)
        );
        config.sensitive_paths = Some(vec!["secrets/".into()]);
        let (rules, warning) = sensitive_rules(&config);
        assert_eq!(rules.patterns().collect::<Vec<_>>(), ["secrets/"]);
        assert_eq!(warning, None);
        config.sensitive_paths = Some(vec![String::new()]);
        let (rules, warning) = sensitive_rules(&config);
        assert_eq!(rules, SensitivePathRules::default());
        assert!(warning.is_some());
    }

    #[tokio::test]
    async fn unavailable_roles_are_warned_and_fall_back() {
        let main = default_spec(ModelRole::Main).id;
        let catalog = FakeModelCatalog::with_ids([main.clone()]);
        let (policy, warnings) =
            validate_models(ModelPolicy::default(), &catalog, CATALOG_TIMEOUT).await;
        assert!(!warnings.is_empty());
        assert!(
            warnings.iter().all(|w| w.starts_with("role `")),
            "{warnings:?}"
        );
        let resolved = policy.resolve(&ResolveRequest::new(ModelRole::Main));
        assert_eq!(resolved.spec.id, main);
    }

    #[tokio::test]
    async fn a_failing_catalog_is_one_warning() {
        let catalog = FakeModelCatalog::failing(LlmError::Network("offline".into()));
        let (policy, warnings) =
            validate_models(ModelPolicy::default(), &catalog, CATALOG_TIMEOUT).await;
        assert_eq!(policy, ModelPolicy::default());
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("offline"), "{warnings:?}");
    }
}
