//! An in-memory [`ApiKeyPort`].

use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;

use crate::ports::credentials::{ApiKeyPort, CredentialError, KeyCheck};

/// Answers every check with the same [`KeyCheck`] and keeps the stored key
/// in memory.
#[derive(Debug, Clone)]
pub struct FakeKeys {
    check: KeyCheck,
    stored: Arc<Mutex<Option<String>>>,
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

    /// The stored key.
    #[must_use]
    pub fn stored(&self) -> Option<String> {
        self.stored
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
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
    async fn check(&self, _key: &str) -> KeyCheck {
        self.check.clone()
    }

    async fn save(&self, key: &str) -> Result<(), CredentialError> {
        self.fail()?;
        *self.stored.lock().unwrap_or_else(PoisonError::into_inner) = Some(key.to_owned());
        Ok(())
    }

    async fn delete(&self) -> Result<bool, CredentialError> {
        self.fail()?;
        Ok(self
            .stored
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
            .is_some())
    }

    fn env_override(&self) -> Option<String> {
        self.env.clone()
    }
}
