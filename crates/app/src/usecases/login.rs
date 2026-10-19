//! `/login` and `/logout`: the API key stored in the system keyring.
//!
//! `/login` checks the key with the provider before storing it: a rejected
//! key is never stored, and one that cannot be checked (no network) is
//! stored with a warning. Both return the message the window shows, and
//! the window then reopens the workspace so the agent starts with the new
//! key.

use crate::ports::credentials::{ApiKeyPort, KeyCheck};

/// Checks and stores `key`; `Err` keeps the dialog open with the reason.
///
/// # Errors
///
/// When the key is malformed or rejected, or the keyring fails.
pub async fn login(port: &dyn ApiKeyPort, key: &str) -> Result<String, String> {
    let key = key.trim();
    if key.is_empty() {
        return Err("paste your API key (it starts with sk-ant-)".to_owned());
    }
    let warning = match port.check(key).await {
        KeyCheck::Valid => None,
        KeyCheck::Malformed(reason) | KeyCheck::Rejected(reason) => return Err(reason),
        KeyCheck::Unreachable(reason) => {
            Some(format!("it could not be checked ({reason}); stored anyway"))
        }
    };
    port.save(key)
        .await
        .map_err(|err| format!("could not store the key in the system keyring: {err}"))?;
    let mut message = "API key stored in the system keyring".to_owned();
    if let Some(warning) = warning {
        message = format!("{message}; {warning}");
    }
    if let Some(var) = port.env_override() {
        message.push_str(&format!(
            "; {var} is set and still wins: unset it to use the stored key"
        ));
    }
    Ok(message)
}

/// Deletes the stored key.
///
/// # Errors
///
/// When the keyring fails.
pub async fn logout(port: &dyn ApiKeyPort) -> Result<String, String> {
    let deleted = port
        .delete()
        .await
        .map_err(|err| format!("could not delete the key from the system keyring: {err}"))?;
    let mut message = if deleted {
        "API key deleted from the system keyring".to_owned()
    } else {
        "no API key was stored in the system keyring".to_owned()
    };
    if let Some(var) = port.env_override() {
        message.push_str(&format!("; {var} is still set, so crest keeps using it"));
    }
    Ok(message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::credentials::FakeKeys;

    fn block<T>(future: impl std::future::Future<Output = T>) -> T {
        futures::executor::block_on(future)
    }

    #[test]
    fn a_valid_key_is_stored() {
        let keys = FakeKeys::new(KeyCheck::Valid);
        let message = block(login(&keys, "  sk-ant-good  ")).unwrap();
        assert_eq!(keys.stored(), Some("sk-ant-good".to_owned()));
        assert_eq!(message, "API key stored in the system keyring");
    }

    #[test]
    fn a_rejected_or_malformed_key_is_not_stored() {
        for check in [
            KeyCheck::Rejected("Anthropic rejected this key".into()),
            KeyCheck::Malformed("it does not start with sk-ant-".into()),
        ] {
            let keys = FakeKeys::new(check);
            assert!(block(login(&keys, "sk-ant-bad")).is_err());
            assert_eq!(keys.stored(), None);
        }
        let keys = FakeKeys::new(KeyCheck::Valid);
        assert!(block(login(&keys, "   ")).is_err());
    }

    #[test]
    fn an_unchecked_key_is_stored_with_a_warning() {
        let keys = FakeKeys::new(KeyCheck::Unreachable("offline".into()));
        let message = block(login(&keys, "sk-ant-x")).unwrap();
        assert!(
            message.contains("could not be checked (offline)"),
            "{message}"
        );
        assert_eq!(keys.stored(), Some("sk-ant-x".to_owned()));
    }

    #[test]
    fn the_environment_variable_is_mentioned() {
        let keys = FakeKeys::new(KeyCheck::Valid).with_env("ANTHROPIC_API_KEY");
        let message = block(login(&keys, "sk-ant-x")).unwrap();
        assert!(message.contains("ANTHROPIC_API_KEY is set"), "{message}");
        let message = block(logout(&keys)).unwrap();
        assert!(message.starts_with("API key deleted"), "{message}");
        assert!(message.contains("still set"), "{message}");
    }

    #[test]
    fn logout_without_a_key_says_so() {
        let keys = FakeKeys::new(KeyCheck::Valid);
        assert_eq!(
            block(logout(&keys)).unwrap(),
            "no API key was stored in the system keyring"
        );
    }

    #[test]
    fn keyring_failures_are_reported() {
        let keys = FakeKeys::new(KeyCheck::Valid).failing();
        let err = block(login(&keys, "sk-ant-x")).unwrap_err();
        assert!(err.contains("system keyring"), "{err}");
        assert!(block(logout(&keys)).is_err());
    }
}
