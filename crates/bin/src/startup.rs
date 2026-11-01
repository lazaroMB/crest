//! Startup steps that only read configuration: the Lua config, the
//! sensitive path rules and the model policy validated against the
//! catalog. Each one degrades to defaults and returns warnings instead of
//! failing, so a broken `init.lua` or an unreachable API never stops crest.

use std::sync::Arc;
use std::time::Duration;

use adapter_anthropic::ApiKey;
use adapter_cloudflare::{ACCOUNT_NAME, ClefClient, TOKEN_NAME, secret_from_env_or_keyring};
use adapter_shell::RtkRewriter;
use adapter_web::{CurlWeb, brave_key_from_env_or_keyring};
use application::decide::{DECISION_OPTION, Decider, DecisionOptions};
use application::ports::decision::DecisionPort;
use application::ports::llm::ModelCatalogPort;
use application::ports::plugin::{
    AppOptions, ConfigSources, LoadedConfig, PluginDiagnostic, PluginHostPort,
};
use application::ports::shell::{CommandRewritePort, ShellSpec};
use application::ports::web::WebPort;
use application::privacy::{PrivacyOptions, Redactor};
use application::rtk::RtkOptions;
use application::tools::web::WebTools;
use application::web::{SearchBackend, WebOptions};
use domain::block::{Block, BlockKind};
use domain::model::{Account, ModelId, ModelPolicy};
use domain::workspace::{SensitivePathRules, WorkspaceRoot};

/// How long the catalog may take before startup goes on without it.
pub const CATALOG_TIMEOUT: Duration = Duration::from_secs(5);

/// Which secrets startup reads from the environment or the keyring.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SecretsWanted {
    /// The API keys of the model providers (not with a scripted model).
    pub models: bool,
    /// The Cloudflare token and account of the decision model, when it is
    /// on; the account set in the options, if any, is not read.
    pub decision: Option<Option<String>>,
    /// The Brave key, when web search asks for Brave.
    pub brave: bool,
}

impl SecretsWanted {
    /// What [`decider`] and [`web_tools`] will need with `options`; `None`
    /// arguments are the ports tests give instead (nothing to read then).
    #[must_use]
    pub fn for_options(
        options: &AppOptions,
        decision: Option<&Arc<dyn DecisionPort>>,
        web: Option<&Arc<dyn WebPort>>,
    ) -> Self {
        let decision = decision
            .is_none()
            .then(|| DecisionOptions::from_options(options).ok())
            .flatten()
            .filter(|options| options.enabled)
            .map(|options| options.account_id);
        // A broken `app.opt.web` falls back to the defaults, as there.
        let web_options = WebOptions::from_options(options).unwrap_or_default();
        let brave =
            web.is_none() && web_options.enabled && web_options.backend == SearchBackend::Brave;
        Self {
            models: false,
            decision,
            brave,
        }
    }

    /// Reads them. Call it off the async runtime: the keyring may block
    /// (D-Bus, Keychain). One task reads them all, so a locked keyring
    /// asks to be unlocked once.
    #[must_use]
    pub fn read(self) -> StartupSecrets {
        let mut secrets = StartupSecrets::default();
        if self.models {
            secrets.anthropic = ApiKey::from_env_or_keyring().ok();
            secrets.opencode = crate::credentials::stored_key(Account::OpenCode);
        }
        if let Some(account) = self.decision {
            secrets.decision = Some((
                secret_from_env_or_keyring(TOKEN_NAME),
                account.or_else(|| secret_from_env_or_keyring(ACCOUNT_NAME)),
            ));
        }
        if self.brave {
            secrets.brave = brave_key_from_env_or_keyring();
        }
        secrets
    }
}

/// What [`SecretsWanted::read`] found.
#[derive(Debug, Default)]
pub struct StartupSecrets {
    /// The Anthropic API key.
    pub anthropic: Option<ApiKey>,
    /// The OpenCode API key.
    pub opencode: Option<String>,
    /// The decision model's token and account, when they were wanted.
    pub decision: Option<(Option<String>, Option<String>)>,
    /// The Brave key.
    pub brave: Option<String>,
}

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

/// The redactor of `app.opt.privacy`, or the defaults (every rule on,
/// with a warning) when it is invalid.
pub fn redactor(options: &AppOptions) -> (Redactor, Option<String>) {
    match PrivacyOptions::from_options(options).and_then(|o| Redactor::new(&o)) {
        Ok(redactor) => (redactor, None),
        Err(err) => (
            Redactor::default(),
            Some(format!("{err}; every privacy rule is on")),
        ),
    }
}

/// The rtk rewriter of `app.opt.rtk` in `root`: `None` when it is off,
/// and also when its binary is missing or the config is broken, with a
/// warning saying so.
pub fn rtk(
    options: &AppOptions,
    root: &WorkspaceRoot,
) -> (Option<Arc<dyn CommandRewritePort>>, Option<String>) {
    let options = match RtkOptions::from_options(options) {
        Ok(options) => options,
        Err(err) => return (None, Some(format!("{err}; rtk is off"))),
    };
    if !options.enabled {
        return (None, None);
    }
    match RtkRewriter::detect(&options.path, root.clone()) {
        Some(rtk) => (Some(Arc::new(rtk)), None),
        None => (
            None,
            Some(format!(
                "rtk is enabled but `{}` was not found; commands run uncompacted \
                 (install it with `cargo install --git https://github.com/rtk-ai/rtk rtk`)",
                options.path
            )),
        ),
    }
}

/// Loads only the global `init.lua` (and its plugins), for what is needed
/// before a project is open: the window mode and the theme of the project
/// picker. Problems come back as warnings.
pub fn global_config(handle: &tokio::runtime::Handle) -> (LoadedConfig, Vec<String>) {
    let host = match adapter_lua::LuaHost::spawn(adapter_lua::HostOptions::default()) {
        Ok(host) => host,
        Err(err) => {
            return (
                LoadedConfig::default(),
                vec![format!("Lua unavailable: {err}")],
            );
        }
    };
    let sources = ConfigSources {
        global: platform::dirs::config_dir().map(|dir| dir.join("init.lua")),
        workspace: None,
    };
    let (config, mut warnings) = handle.block_on(load_config(Some(&host), sources));
    warnings.extend(config.errors.iter().map(diagnostic));
    (config, warnings)
}

/// The window mode of `app.opt.window`, or the platform default (with a
/// warning when the option is broken).
pub fn window(options: &AppOptions) -> (application::window::WindowMode, Option<String>) {
    use application::window::{WindowMode, from_options};
    match from_options(options) {
        Ok(mode) => (mode.unwrap_or_default(), None),
        Err(err) => (WindowMode::default(), Some(err.to_string())),
    }
}

/// The theme catalog: the theme files next to the global config
/// (`<config dir>/themes/*.lua`) and the themes `init.lua` defined.
pub fn theme_catalog(
    global_config: Option<&std::path::Path>,
    defined: &[serde_json::Value],
) -> (application::theme::ThemeCatalog, Vec<String>) {
    let mut catalog = application::theme::ThemeCatalog::new();
    if let Some(dir) = global_config.and_then(std::path::Path::parent) {
        let source = adapter_lua::LuaThemeSource::new(dir.join(adapter_lua::THEMES_DIR));
        catalog = catalog.with_files(Arc::new(source));
    }
    catalog.with_defined(defined)
}

/// The color theme: `app.opt.theme` when set, else the theme `picked`
/// with `/theme`, else TwoCrest; `app.opt.colors` goes over it. A theme
/// that cannot be resolved falls back to TwoCrest with a warning.
pub fn theme(
    options: &AppOptions,
    catalog: &application::theme::ThemeCatalog,
    picked: Option<&str>,
) -> (Arc<domain::theme::Theme>, Vec<String>) {
    // Problems of `app.opt.colors` were reported when the config loaded.
    let (overrides, _) = application::theme::overrides_from_options(options);
    let name = options
        .theme
        .as_deref()
        .or(picked)
        .unwrap_or(application::theme::TWOCREST);
    let resolved = catalog.resolve_or_default(name, overrides.as_ref());
    (resolved.theme, resolved.warnings)
}

/// The shell for whole command lines: `app.opt.shell` when set and valid;
/// else `sh -c` on Unix and, on Windows, `pwsh` when it is on `PATH` or the
/// bundled `powershell`. A broken option is a warning.
pub fn line_shell(options: &AppOptions) -> (ShellSpec, Option<String>) {
    let fallback = || {
        if cfg!(windows) && platform::process::find_program("pwsh", None).is_some() {
            ShellSpec::powershell("pwsh")
        } else {
            ShellSpec::default()
        }
    };
    match application::shell::from_options(options) {
        Ok(Some(shell)) => (shell, None),
        Ok(None) => (fallback(), None),
        Err(err) => {
            let shell = fallback();
            let warning = format!("{err}; using `{}`", shell.program);
            (shell, Some(warning))
        }
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

/// The decision model of `app.opt.decision`: `port` when given (tests),
/// else Clef on Cloudflare when its token and account are set (`secrets`,
/// from the environment or the keyring: see [`SecretsWanted`]). Without
/// them crest works as before; a warning says so only when
/// `app.opt.decision` asks for it.
pub fn decider(
    options: &AppOptions,
    port: Option<Arc<dyn DecisionPort>>,
    secrets: Option<(Option<String>, Option<String>)>,
) -> (Decider, Vec<String>) {
    let mut warnings = Vec::new();
    let configured = options.extra.contains_key(DECISION_OPTION);
    let options = DecisionOptions::from_options(options).unwrap_or_else(|err| {
        warnings.push(format!("{err}; the decision model is off"));
        DecisionOptions {
            enabled: false,
            ..DecisionOptions::default()
        }
    });
    if !options.enabled {
        return (Decider::none(), warnings);
    }
    if let Some(port) = port {
        return (Decider::new(port, options), warnings);
    }
    let (Some(token), Some(account)) = secrets.unwrap_or_default() else {
        if configured {
            warnings.push(format!(
                "app.opt.{DECISION_OPTION} is set but {TOKEN_NAME} or {ACCOUNT_NAME} is missing \
                 (environment or keyring entry crest / …); the decision model is off"
            ));
        }
        return (Decider::none(), warnings);
    };
    match ClefClient::new(&options, &account, token) {
        Ok(client) => (Decider::new(Arc::new(client), options), warnings),
        Err(err) => {
            warnings.push(format!("{err}; the decision model is off"));
            (Decider::none(), warnings)
        }
    }
}

/// The web tools of `app.opt.web`: `web` when given (tests), else curl.
/// `None`, with a warning saying why, when the web is disabled, curl is
/// missing or the config is broken (then the defaults are used). A Brave
/// backend without a key (`brave_key`, see [`SecretsWanted`]) falls back
/// to DuckDuckGo.
pub async fn web_tools(
    options: &AppOptions,
    web: Option<Arc<dyn WebPort>>,
    brave_key: Option<String>,
) -> (Option<WebTools>, Vec<String>) {
    let mut warnings = Vec::new();
    let mut options = WebOptions::from_options(options).unwrap_or_else(|err| {
        warnings.push(format!("{err}; web search uses the defaults"));
        WebOptions::default()
    });
    if !options.enabled {
        return (None, warnings);
    }
    let default_results = options.max_results;
    if let Some(web) = web {
        return (
            Some(WebTools {
                web,
                default_results,
            }),
            warnings,
        );
    }
    let mut key = None;
    if options.backend == SearchBackend::Brave {
        key = brave_key;
        if key.is_none() {
            warnings.push(
                "app.opt.web.backend is \"brave\" but there is no key (BRAVE_API_KEY or the \
                 keyring entry crest / BRAVE_API_KEY); searching with DuckDuckGo"
                    .to_owned(),
            );
            options.backend = SearchBackend::DuckDuckGo;
        }
    }
    let curl = CurlWeb::new(options, key);
    if !curl.curl_available().await {
        warnings.push("web search unavailable: install curl".to_owned());
        return (None, warnings);
    }
    (
        Some(WebTools {
            web: Arc::new(curl),
            default_results,
        }),
        warnings,
    )
}

#[cfg(test)]
mod tests {
    use application::ports::llm::LlmError;
    use application_testing::llm::FakeModelCatalog;
    use domain::model::{ModelRole, ResolveRequest, default_spec};

    use super::*;

    #[tokio::test]
    async fn web_tools_follow_the_options() {
        use application_testing::web::ScriptedWeb;
        use serde_json::json;

        let scripted = || Some(Arc::new(ScriptedWeb::new()) as Arc<dyn WebPort>);
        let (web, warnings) = web_tools(&AppOptions::default(), scripted(), None).await;
        assert_eq!(web.map(|w| w.default_results), Some(8));
        assert!(warnings.is_empty(), "{warnings:?}");

        let mut options = AppOptions::default();
        options
            .extra
            .insert("web".into(), json!({ "enabled": false }));
        let (web, warnings) = web_tools(&options, scripted(), None).await;
        assert!(web.is_none() && warnings.is_empty());

        options
            .extra
            .insert("web".into(), json!({ "max_results": 99 }));
        let (web, warnings) = web_tools(&options, scripted(), None).await;
        assert!(web.is_some());
        assert_eq!(warnings.len(), 1, "{warnings:?}");
    }

    #[test]
    fn startup_reads_only_the_secrets_it_needs() {
        use application_testing::web::ScriptedWeb;
        use serde_json::json;

        let wanted = SecretsWanted::for_options(&AppOptions::default(), None, None);
        assert_eq!(
            wanted,
            SecretsWanted {
                models: false,
                decision: Some(None),
                brave: false,
            }
        );

        let mut options = AppOptions::default();
        options
            .extra
            .insert(DECISION_OPTION.into(), json!({ "account_id": "acct" }));
        options
            .extra
            .insert("web".into(), json!({ "backend": "brave" }));
        let wanted = SecretsWanted::for_options(&options, None, None);
        assert_eq!(wanted.decision, Some(Some("acct".to_owned())));
        assert!(wanted.brave);

        // Ports given instead (tests), or the decision model off: nothing.
        let web = Arc::new(ScriptedWeb::new()) as Arc<dyn WebPort>;
        let wanted = SecretsWanted::for_options(&options, None, Some(&web));
        assert!(!wanted.brave);
        options
            .extra
            .insert(DECISION_OPTION.into(), json!({ "enabled": false }));
        assert_eq!(
            SecretsWanted::for_options(&options, None, None).decision,
            None
        );
    }

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
