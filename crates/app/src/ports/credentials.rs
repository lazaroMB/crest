//! The API keys of the model providers' accounts: checking a new one and
//! keeping it in the system keyring, for `/login` and `/logout`.

use async_trait::async_trait;
use domain::model::Account;

/// What the provider said about a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyCheck {
    /// The provider accepted it.
    Valid,
    /// It does not look like a key (empty, wrong prefix).
    Malformed(String),
    /// The provider rejected it (401/403).
    Rejected(String),
    /// The provider could not be reached; the key may still be right.
    Unreachable(String),
}

/// The keyring could not be used.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct CredentialError(pub String);

/// Checks API keys and stores them where crest reads them at startup, one
/// per [`Account`].
#[async_trait]
pub trait ApiKeyPort: Send + Sync {
    /// Asks the provider of `account` whether `key` works.
    async fn check(&self, account: Account, key: &str) -> KeyCheck;

    /// Stores `key` for `account`, replacing the one stored before.
    async fn save(&self, account: Account, key: &str) -> Result<(), CredentialError>;

    /// Deletes the stored key of `account`; `false` when there was none.
    async fn delete(&self, account: Account) -> Result<bool, CredentialError>;

    /// Name of the environment variable that wins over the stored key of
    /// `account`, when it is set (`ANTHROPIC_API_KEY`).
    fn env_override(&self, account: Account) -> Option<String>;

    /// The accounts that have a key (environment or keyring). May block on
    /// the keyring.
    fn configured(&self) -> Vec<Account>;
}
