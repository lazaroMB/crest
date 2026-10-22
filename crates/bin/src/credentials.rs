//! [`ApiKeyPort`] over the providers' APIs and the system keyring: one
//! entry per account in service `crest` (`ANTHROPIC_API_KEY`,
//! `OPENCODE_API_KEY`), read at startup by [`crate::root`]. The environment
//! variable of the same name wins over the stored key.

use adapter_anthropic::ClientConfig;
use adapter_anthropic::credentials::{check_key, delete_key, env_override, read_key, save_key};
use application::ports::credentials::{ApiKeyPort, CredentialError, KeyCheck};
use application::providers::ProvidersOptions;
use async_trait::async_trait;
use domain::model::Account;

/// Checks keys with each provider and stores them in the keyring.
#[derive(Debug, Clone, Default)]
pub struct KeyringCredentials {
    anthropic: ClientConfig,
    providers: ProvidersOptions,
}

impl KeyringCredentials {
    /// Checks against the APIs of `anthropic` and `providers` (tests point
    /// them at a mock).
    #[must_use]
    pub fn new(anthropic: ClientConfig, providers: ProvidersOptions) -> Self {
        Self {
            anthropic,
            providers,
        }
    }
}

/// The key of `account` from its environment variable or the keyring.
/// Blocks on the keyring.
#[must_use]
pub fn stored_key(account: Account) -> Option<String> {
    read_key(account.key_name())
}

#[async_trait]
impl ApiKeyPort for KeyringCredentials {
    async fn check(&self, account: Account, key: &str) -> KeyCheck {
        match account {
            Account::Anthropic => check_key(&self.anthropic, key).await,
            Account::OpenCode => crate::opencode::check_key(key, &self.providers).await,
        }
    }

    async fn save(&self, account: Account, key: &str) -> Result<(), CredentialError> {
        save_key(account.key_name(), key).await
    }

    async fn delete(&self, account: Account) -> Result<bool, CredentialError> {
        delete_key(account.key_name()).await
    }

    fn env_override(&self, account: Account) -> Option<String> {
        env_override(account.key_name())
    }

    fn configured(&self) -> Vec<Account> {
        Account::ALL
            .into_iter()
            .filter(|account| stored_key(*account).is_some())
            .collect()
    }
}
