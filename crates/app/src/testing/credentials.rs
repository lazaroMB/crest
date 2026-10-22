//! An in-memory [`ApiKeyPort`].

use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use domain::model::Account;

use crate::ports::credentials::{ApiKeyPort, CredentialError, KeyCheck};

/// Answers every check with the same [`KeyCheck`] and keeps the stored keys
/// in memory.
#[derive(Debug, Clone)]
pub struct FakeKeys {
    check: KeyCheck,
    stored: Arc<Mutex<Vec<(Account, String)>>>,
    env: Option<String>,
    failing: bool,
}

impl FakeKeys {
    /// Keys that `check` answers with `check`.
    #[must_use]
    pub fn new(check: KeyCheck) -> Self {
        Self {
            check,
            stored: Arc::default(),
            env: None,
            failing: false,
        }
    }

    /// As if the environment variable `var` were set.
    #[must_use]
    pub fn with_env(mut self, var: &str) -> Self {
        self.env = Some(var.to_owned());
        self
    }

    /// Every keyring call fails.
    #[must_use]
    pub fn failing(mut self) -> Self {
        self.failing = true;
        self
    }

    /// The stored key of Anthropic.
    #[must_use]
    pub fn stored(&self) -> Option<String> {
        self.stored_for(Account::Anthropic)
    }

    /// The stored key of `account`.
    #[must_use]
    pub fn stored_for(&self, account: Account) -> Option<String> {
        self.stored
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .find(|(a, _)| *a == account)
            .map(|(_, key)| key.clone())
    }

    fn fail(&self) -> Result<(), CredentialError> {
        if self.failing {
            Err(CredentialError("no keyring".into()))
        } else {
            Ok(())
        }
    }
}

#[async_trait]
impl ApiKeyPort for FakeKeys {
    async fn check(&self, _account: Account, _key: &str) -> KeyCheck {
        self.check.clone()
    }

    async fn save(&self, account: Account, key: &str) -> Result<(), CredentialError> {
        self.fail()?;
        let mut stored = self.stored.lock().unwrap_or_else(PoisonError::into_inner);
        stored.retain(|(a, _)| *a != account);
        stored.push((account, key.to_owned()));
        Ok(())
    }

    async fn delete(&self, account: Account) -> Result<bool, CredentialError> {
        self.fail()?;
        let mut stored = self.stored.lock().unwrap_or_else(PoisonError::into_inner);
        let before = stored.len();
        stored.retain(|(a, _)| *a != account);
        Ok(stored.len() != before)
    }

    fn env_override(&self, _account: Account) -> Option<String> {
        self.env.clone()
    }

    fn configured(&self) -> Vec<Account> {
        self.stored
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .map(|(a, _)| *a)
            .collect()
    }
}
