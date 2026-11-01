//! The model ports: the providers with a key, routed by the prefix of each
//! model id, and what an OpenCode key may use.

use std::path::Path;
use std::sync::Arc;

use adapter_anthropic::{
    AnthropicBackend, AnthropicCatalog, AnthropicClient, ApiKey, ClientConfig,
};
use adapter_support::TokioClock;
use anyhow::anyhow;
use application::llm_retry::RetryingLlm;
use application::ports::llm::{LlmPort, ModelCatalogPort};
use application::ports::plugin::AppOptions;
use application::providers::{
    CachedCatalog, Provider, ProviderRouter, ProvidersOptions, effective_default,
};
use domain::model::ProviderId;

use super::{MissingApiKey, Overrides};
use crate::opencode::PlanAccess;
use crate::preferences::{self, Preferences};
use crate::startup::CATALOG_TIMEOUT;

type ModelPorts = (
    Arc<dyn LlmPort>,
    Option<Arc<dyn ModelCatalogPort>>,
    ProviderId,
);

/// The model and its catalog: the overrides, or every provider with a key
/// in the environment or the keyring (Anthropic; OpenCode Go and Zen),
/// routed by the prefix of each model id (`app.opt.providers`).
/// Asks OpenCode what `key` may use and saves the answer in the
/// preferences at `path`, unless nothing could be found out (offline): then
/// the next start asks again.
async fn opencode_access(
    key: &str,
    providers: &ProvidersOptions,
    listings: &crate::opencode::PlanListings,
    path: Option<&Path>,
    warnings: &mut Vec<String>,
) -> PlanAccess {
    let access = crate::opencode::probe_access(key, providers, listings, CATALOG_TIMEOUT).await;
    let known = access.go.is_some() || access.zen.is_some() || access.zen_free.is_some();
    if let (true, Some(path)) = (known, path) {
        let path = path.to_path_buf();
        let saved = tokio::task::spawn_blocking(move || {
            preferences::update(Some(&path), |p| p.opencode_access = Some(access))
        })
        .await
        .map_err(|err| err.to_string())
        .and_then(|saved| saved);
        if let Err(err) = saved {
            warnings.push(format!(
                "what the OpenCode key may use was not saved: {err}"
            ));
        }
    }
    access
}

pub(super) async fn model_ports(
    overrides: &Overrides,
    options: &AppOptions,
    preferences: &Preferences,
    preferences_path: Option<&Path>,
    keys: (Option<ApiKey>, Option<String>),
    warnings: &mut Vec<String>,
) -> anyhow::Result<ModelPorts> {
    let providers = ProvidersOptions::from_options(options).unwrap_or_else(|err| {
        warnings.push(format!("{err}; using the default providers"));
        ProvidersOptions::default()
    });
    if let Some(llm) = &overrides.llm {
        // A scripted model: only the config names the default provider.
        let default = effective_default(providers.default, None, &[]);
        return Ok((Arc::clone(llm), overrides.catalog.clone(), default));
    }
    let (anthropic, opencode) = keys;
    // The plans' model lists, shared by the probe and the catalogs.
    let listings = opencode
        .as_deref()
        .map(|key| crate::opencode::PlanListings::new(key, &providers))
        .transpose()
        .map_err(|err| anyhow!("cannot create the OpenCode client: {err}"))?;
    // A plan left unknown (offline, a timeout) is asked again at each start.
    let access = match (&opencode, &listings, preferences.opencode_access) {
        (Some(key), Some(listings), saved) if saved.is_none_or(|a| !a.is_complete()) => {
            opencode_access(key, &providers, listings, preferences_path, warnings).await
        }
        (_, _, saved) => saved.unwrap_or_default(),
    };
    let mut with_key = Vec::new();
    if anthropic.is_some() {
        with_key.push(ProviderId::Anthropic);
    }
    if opencode.is_some() {
        with_key.extend(
            [ProviderId::OpencodeGo, ProviderId::OpencodeZen]
                .into_iter()
                .filter(|plan| access.serves(*plan)),
        );
    }
    let preferred = preferences
        .provider
        .map(|p| Preferences::opencode_plan(p, &access));
    let default = effective_default(providers.default, preferred, &with_key);
    if opencode.is_some() && !access.serves(default) {
        warnings.push(format!(
            "your OpenCode key cannot use {default} (no subscription or balance); \
             pick another model with /model or set app.opt.providers.default"
        ));
    }
    let mut router = ProviderRouter::new(default);
    if let Some(key) = anthropic {
        let client = AnthropicClient::new(&key, ClientConfig::default())
            .map_err(|err| anyhow!("cannot create the Anthropic client: {err}"))?;
        router = router.with(
            ProviderId::Anthropic,
            Provider {
                backend: Arc::new(AnthropicBackend::new(client.clone())),
                catalog: Arc::new(CachedCatalog::new(Arc::new(AnthropicCatalog::new(client)))),
            },
        );
    }
    if let (Some(key), Some(listings)) = (opencode, &listings) {
        for plan in [ProviderId::OpencodeGo, ProviderId::OpencodeZen] {
            let provider = crate::opencode::provider(&key, plan, &providers, access, listings)
                .map_err(|err| anyhow!("cannot create the OpenCode client: {err}"))?;
            router = router.with(plan, provider);
        }
    }
    if router.is_empty() {
        return Err(MissingApiKey.into());
    }
    if !router.providers().any(|p| p == default) {
        let account = default.account();
        warnings.push(format!(
            "no {} API key, so models without a provider prefix (the defaults) cannot run: \
             /login {account}, or point app.models.roles at prefixed models \
             (opencode-go:glm-5.3) or set app.opt.providers.default",
            account.display_name()
        ));
    }
    let catalog = overrides
        .catalog
        .clone()
        .unwrap_or_else(|| Arc::new(router.catalog()));
    let llm = RetryingLlm::new(Arc::new(router), Arc::new(TokioClock));
    Ok((Arc::new(llm), Some(catalog), default))
}
