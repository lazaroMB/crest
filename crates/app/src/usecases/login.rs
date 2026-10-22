//! `/login` and `/logout`: the API key of each account (Anthropic,
//! OpenCode) stored in the system keyring.
//!
//! `/login` checks the key with the provider before storing it: a rejected
//! key is never stored, and one that cannot be checked (no network) is
//! stored with a warning. Both return the message the window shows, and
//! the window then reopens the workspace so the agent starts with the new
//! key.

use domain::model::Account;

use crate::ports::credentials::{ApiKeyPort, KeyCheck};

/// What to paste in the `/login` dialog of `account`.
#[must_use]
pub fn key_prompt(account: Account) -> &'static str {
    match account {
        Account::Anthropic => "paste your Anthropic API key (it starts with sk-ant-)",
        Account::OpenCode => "paste your OpenCode API key (opencode.ai, Go or Zen)",
    }
}

/// Checks and stores `key` for `account`; `Err` keeps the dialog open with
/// the reason.
///
/// # Errors
///
/// When the key is malformed or rejected, or the keyring fails.
pub async fn login(port: &dyn ApiKeyPort, account: Account, key: &str) -> Result<String, String> {
    let key = key.trim();
    if key.is_empty() {
        return Err(key_prompt(account).to_owned());
    }
    let warning = match port.check(account, key).await {
        KeyCheck::Valid => None,
        KeyCheck::Malformed(reason) | KeyCheck::Rejected(reason) => return Err(reason),
        KeyCheck::Unreachable(reason) => {
            Some(format!("it could not be checked ({reason}); stored anyway"))
        }
    };
    port.save(account, key)
        .await
        .map_err(|err| format!("could not store the key in the system keyring: {err}"))?;
    let name = account.display_name();
    let mut message = format!("{name} API key stored in the system keyring");
    if let Some(warning) = warning {
        message = format!("{message}; {warning}");
    }
    if let Some(var) = port.env_override(account) {
        message.push_str(&format!(
            "; {var} is set and still wins: unset it to use the stored key"
        ));
    }
    Ok(message)
}

/// Deletes the stored key of `account`.
///
/// # Errors
///
/// When the keyring fails.
pub async fn logout(port: &dyn ApiKeyPort, account: Account) -> Result<String, String> {
    let deleted = port
        .delete(account)
        .await
        .map_err(|err| format!("could not delete the key from the system keyring: {err}"))?;
    let name = account.display_name();
    let mut message = if deleted {
        format!("{name} API key deleted from the system keyring")
    } else {
        format!("no {name} API key was stored in the system keyring")
    };
    if let Some(var) = port.env_override(account) {
        message.push_str(&format!("; {var} is still set, so crest keeps using it"));
    }
    Ok(message)
}

/// `/logout` without an account: deletes the stored key of every account.
///
/// # Errors
///
/// When the keyring fails.
pub async fn logout_all(port: &dyn ApiKeyPort) -> Result<String, String> {
    let mut deleted = Vec::new();
    let mut env = Vec::new();
    for account in Account::ALL {
        if port
            .delete(account)
            .await
            .map_err(|err| format!("could not delete the key from the system keyring: {err}"))?
        {
            deleted.push(account.display_name());
        }
        env.extend(port.env_override(account));
    }
    let mut message = if deleted.is_empty() {
        "no API key was stored in the system keyring".to_owned()
    } else {
        format!(
            "API keys deleted from the system keyring: {}",
            deleted.join(", ")
        )
    };
    if !env.is_empty() {
        message.push_str(&format!(
            "; {} still set, so crest keeps using it",
            env.join(", ")
        ));
    }
    Ok(message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::credentials::FakeKeys;

    const ANTHROPIC: Account = Account::Anthropic;

    fn block<T>(future: impl std::future::Future<Output = T>) -> T {
        futures::executor::block_on(future)
    }

    #[test]
    fn a_valid_key_is_stored() {
        let keys = FakeKeys::new(KeyCheck::Valid);
        let message = block(login(&keys, ANTHROPIC, "  sk-ant-good  ")).unwrap();
        assert_eq!(keys.stored(), Some("sk-ant-good".to_owned()));
        assert_eq!(message, "Anthropic API key stored in the system keyring");
    }

    #[test]
    fn a_rejected_or_malformed_key_is_not_stored() {
        for check in [
            KeyCheck::Rejected("Anthropic rejected this key".into()),
            KeyCheck::Malformed("it does not start with sk-ant-".into()),
        ] {
            let keys = FakeKeys::new(check);
            assert!(block(login(&keys, ANTHROPIC, "sk-ant-bad")).is_err());
            assert_eq!(keys.stored(), None);
        }
        let keys = FakeKeys::new(KeyCheck::Valid);
        assert!(block(login(&keys, ANTHROPIC, "   ")).is_err());
    }

    #[test]
    fn an_unchecked_key_is_stored_with_a_warning() {
        let keys = FakeKeys::new(KeyCheck::Unreachable("offline".into()));
        let message = block(login(&keys, ANTHROPIC, "sk-ant-x")).unwrap();
        assert!(
            message.contains("could not be checked (offline)"),
            "{message}"
        );
        assert_eq!(keys.stored(), Some("sk-ant-x".to_owned()));
    }

    #[test]
    fn the_environment_variable_is_mentioned() {
        let keys = FakeKeys::new(KeyCheck::Valid).with_env("ANTHROPIC_API_KEY");
        let message = block(login(&keys, ANTHROPIC, "sk-ant-x")).unwrap();
        assert!(message.contains("ANTHROPIC_API_KEY is set"), "{message}");
        let message = block(logout(&keys, ANTHROPIC)).unwrap();
        assert!(
            message.starts_with("Anthropic API key deleted"),
            "{message}"
        );
        assert!(message.contains("still set"), "{message}");
    }

    #[test]
    fn logout_without_a_key_says_so() {
        let keys = FakeKeys::new(KeyCheck::Valid);
        assert_eq!(
            block(logout(&keys, ANTHROPIC)).unwrap(),
            "no Anthropic API key was stored in the system keyring"
        );
    }

    #[test]
    fn each_account_keeps_its_own_key() {
        let keys = FakeKeys::new(KeyCheck::Valid);
        block(login(&keys, ANTHROPIC, "sk-ant-x")).unwrap();
        let message = block(login(&keys, Account::OpenCode, "oc-key")).unwrap();
        assert_eq!(message, "OpenCode API key stored in the system keyring");
        assert_eq!(
            keys.stored_for(Account::OpenCode),
            Some("oc-key".to_owned())
        );
        block(logout(&keys, Account::OpenCode)).unwrap();
        assert_eq!(keys.stored_for(Account::OpenCode), None);
        assert_eq!(keys.stored(), Some("sk-ant-x".to_owned()));
    }

    #[test]
    fn logout_without_an_account_deletes_every_key() {
        let keys = FakeKeys::new(KeyCheck::Valid);
        assert_eq!(
            block(logout_all(&keys)).unwrap(),
            "no API key was stored in the system keyring"
        );
        block(login(&keys, ANTHROPIC, "sk-ant-x")).unwrap();
        block(login(&keys, Account::OpenCode, "oc-key")).unwrap();
        assert_eq!(
            block(logout_all(&keys)).unwrap(),
            "API keys deleted from the system keyring: Anthropic, OpenCode"
        );
        assert_eq!(keys.stored_for(Account::OpenCode), None);
        assert_eq!(keys.stored(), None);
    }

    #[test]
    fn keyring_failures_are_reported() {
        let keys = FakeKeys::new(KeyCheck::Valid).failing();
        let err = block(login(&keys, ANTHROPIC, "sk-ant-x")).unwrap_err();
        assert!(err.contains("system keyring"), "{err}");
        assert!(block(logout(&keys, ANTHROPIC)).is_err());
    }
}
