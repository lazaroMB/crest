//! OAuth logins of remote MCP servers (`/mcp login <name>`), with rmcp's
//! `auth` support: discovery from the server's metadata, dynamic client
//! registration (or a pre-registered client from the config), PKCE and a
//! one-shot callback on a loopback port. Tokens live in the system keyring
//! (service `crest`, entry `mcp-oauth:<server>`) and rmcp refreshes them.
//! Windows' Credential Manager holds at most 2560 bytes per secret, less
//! than an access and a refresh token together, so a long entry is split
//! over `mcp-oauth:<server>#1`, `#2`… and the main entry says how many.

use std::time::Duration;

use application::ports::plugin::McpOAuthClient;
use async_trait::async_trait;
use rmcp::transport::auth::{
    AuthError, AuthorizationManager, AuthorizationRequest, CredentialStore, OAuthClientConfig,
    OAuthState, StoredCredentials,
};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Keyring service of every crest secret.
const KEYRING_SERVICE: &str = "crest";
/// Most characters of one keyring secret; a longer entry is split. Windows
/// stores secrets as UTF-16 in at most 2560 bytes.
const CHUNK_CHARS: usize = 1000;
/// Starts the main entry of a split one, followed by the number of parts.
const CHUNKED_PREFIX: &str = "crest-chunks:";
/// Longest wait for the browser to come back.
const LOGIN_TIMEOUT: Duration = Duration::from_secs(300);

/// What one keyring entry holds: the tokens and the URL they are for.
#[derive(Serialize, Deserialize)]
struct Entry {
    url: String,
    credentials: StoredCredentials,
}

/// [`CredentialStore`] over one keyring entry per server.
#[derive(Debug, Clone)]
pub(crate) struct KeyringStore {
    user: String,
    url: String,
}

impl KeyringStore {
    pub(crate) fn new(server: &str, url: &str) -> Self {
        Self {
            user: format!("mcp-oauth:{server}"),
            url: url.to_owned(),
        }
    }

    async fn blocking<T: Send + 'static>(
        &self,
        work: impl FnOnce(&Entries) -> Result<T, String> + Send + 'static,
    ) -> Result<T, AuthError> {
        let entries = Entries {
            user: self.user.clone(),
        };
        tokio::task::spawn_blocking(move || work(&entries))
            .await
            .map_err(|e| AuthError::CredentialStoreError(e.to_string()))?
            .map_err(AuthError::CredentialStoreError)
    }

    /// Whether tokens for this server and URL are stored.
    pub(crate) async fn has_tokens(&self) -> bool {
        matches!(self.load().await, Ok(Some(c)) if c.token_response.is_some())
    }

    /// Deletes the entry; `false` when there was none.
    pub(crate) async fn delete(&self) -> Result<bool, String> {
        self.blocking(Entries::delete)
            .await
            .map_err(|e| e.to_string())
    }
}

/// The keyring entries of one server: the main one and its parts.
struct Entries {
    user: String,
}

impl Entries {
    fn entry(&self, part: usize) -> Result<keyring::Entry, String> {
        let user = if part == 0 {
            self.user.clone()
        } else {
            format!("{}#{part}", self.user)
        };
        keyring::Entry::new(KEYRING_SERVICE, &user).map_err(|e| e.to_string())
    }

    /// The whole stored text, `None` when there is none.
    fn read(&self) -> Result<Option<String>, String> {
        let head = match self.entry(0)?.get_password() {
            Ok(head) => head,
            Err(keyring::Error::NoEntry) => return Ok(None),
            Err(err) => return Err(err.to_string()),
        };
        let Some(count) = head.strip_prefix(CHUNKED_PREFIX) else {
            return Ok(Some(head));
        };
        let count: usize = count
            .parse()
            .map_err(|_| "corrupt keyring entry".to_owned())?;
        let mut text = String::new();
        for part in 1..=count {
            match self.entry(part)?.get_password() {
                Ok(chunk) => text.push_str(&chunk),
                // A part went missing: the tokens are unusable.
                Err(keyring::Error::NoEntry) => return Ok(None),
                Err(err) => return Err(err.to_string()),
            }
        }
        Ok(Some(text))
    }

    /// Stores `text`, split when it is long, and drops parts left over by a
    /// longer earlier value.
    fn write(&self, text: &str) -> Result<(), String> {
        let chunks = split(text, CHUNK_CHARS);
        if chunks.len() == 1 {
            self.entry(0)?
                .set_password(text)
                .map_err(|e| e.to_string())?;
        } else {
            for (i, chunk) in chunks.iter().enumerate() {
                self.entry(i + 1)?
                    .set_password(chunk)
                    .map_err(|e| e.to_string())?;
            }
            self.entry(0)?
                .set_password(&format!("{CHUNKED_PREFIX}{}", chunks.len()))
                .map_err(|e| e.to_string())?;
        }
        let used = if chunks.len() == 1 { 0 } else { chunks.len() };
        self.delete_parts_after(used);
        Ok(())
    }

    /// Deletes the main entry and its parts; `false` when there was none.
    fn delete(&self) -> Result<bool, String> {
        self.delete_parts_after(0);
        match self.entry(0)?.delete_credential() {
            Ok(()) => Ok(true),
            Err(keyring::Error::NoEntry) => Ok(false),
            Err(err) => Err(err.to_string()),
        }
    }

    /// Best effort: deletes parts `last + 1`, `last + 2`… until one is missing.
    fn delete_parts_after(&self, last: usize) {
        for part in last + 1.. {
            match self.entry(part).map(|e| e.delete_credential()) {
                Ok(Ok(())) => {}
                _ => break,
            }
        }
    }
}

/// `text` in pieces of at most `max` characters (one piece when it fits).
fn split(text: &str, max: usize) -> Vec<&str> {
    let mut chunks = Vec::new();
    let mut rest = text;
    while rest.chars().count() > max {
        let cut = rest.char_indices().nth(max).map_or(rest.len(), |(i, _)| i);
        let (chunk, tail) = rest.split_at(cut);
        chunks.push(chunk);
        rest = tail;
    }
    chunks.push(rest);
    chunks
}

#[async_trait]
impl CredentialStore for KeyringStore {
    async fn load(&self) -> Result<Option<StoredCredentials>, AuthError> {
        let url = self.url.clone();
        self.blocking(move |entries| {
            Ok(entries.read()?.and_then(|json| {
                serde_json::from_str::<Entry>(&json)
                    .ok()
                    // Tokens of another URL are not for this server.
                    .filter(|stored| stored.url == url)
                    .map(|stored| stored.credentials)
            }))
        })
        .await
    }

    async fn save(&self, credentials: StoredCredentials) -> Result<(), AuthError> {
        let json = serde_json::to_string(&Entry {
            url: self.url.clone(),
            credentials,
        })
        .map_err(|e| AuthError::CredentialStoreError(e.to_string()))?;
        self.blocking(move |entries| entries.write(&json)).await
    }

    async fn clear(&self) -> Result<(), AuthError> {
        self.delete()
            .await
            .map(|_| ())
            .map_err(AuthError::CredentialStoreError)
    }
}

/// The redirect URI on `port`.
fn redirect_uri(port: u16) -> String {
    format!("http://127.0.0.1:{port}/callback")
}

/// An authorization manager with the stored tokens of `server`, if there
/// are some for `url`; `None` connects without OAuth.
pub(crate) async fn stored_manager(
    server: &str,
    url: &str,
    client: Option<&McpOAuthClient>,
) -> Option<AuthorizationManager> {
    let store = KeyringStore::new(server, url);
    if !store.has_tokens().await {
        return None;
    }
    let mut manager = AuthorizationManager::new(url)
        .await
        .inspect_err(|err| tracing::warn!(target: "crest::mcp", server, "OAuth: {err}"))
        .ok()?;
    manager.set_credential_store(store);
    match manager.initialize_from_store().await {
        Ok(true) => {}
        Ok(false) => return None,
        Err(err) => {
            tracing::warn!(target: "crest::mcp", server, "OAuth tokens unusable: {err}");
            return None;
        }
    }
    // A confidential pre-registered client needs its secret to refresh.
    if let Some(client) = client
        && let Some(secret) = &client.client_secret
    {
        let config = OAuthClientConfig::new(
            client.client_id.clone(),
            redirect_uri(client.callback_port.unwrap_or(0)),
        )
        .with_client_secret(secret.clone())
        .with_scopes(client.scopes.clone());
        if let Err(err) = manager.configure_client(config) {
            tracing::warn!(target: "crest::mcp", server, "OAuth client: {err}");
        }
    }
    Some(manager)
}

/// Runs the login of `server` at `url`: `show_url` gets the page to open
/// in the browser, and the tokens are stored once the browser comes back
/// to the loopback callback.
///
/// # Errors
///
/// A message for the user when discovery, registration, the callback or
/// the token exchange fail, or the browser does not come back in time.
pub(crate) async fn login(
    server: &str,
    url: &str,
    client: Option<&McpOAuthClient>,
    show_url: &(dyn Fn(&str) + Send + Sync),
) -> Result<(), String> {
    let port = client.and_then(|c| c.callback_port).unwrap_or(0);
    let listener = TcpListener::bind(("127.0.0.1", port))
        .await
        .map_err(|err| format!("cannot listen on 127.0.0.1:{port} for the login: {err}"))?;
    let port = listener.local_addr().map_err(|err| err.to_string())?.port();
    let redirect = redirect_uri(port);

    let mut manager = AuthorizationManager::new(url)
        .await
        .map_err(|err| format!("OAuth discovery for {url} failed: {err}"))?;
    let store = KeyringStore::new(server, url);
    manager.set_credential_store(store);
    let mut state = OAuthState::Unauthorized(manager);
    let mut request = AuthorizationRequest::new(redirect.clone())
        .with_client_name("crest")
        .with_application_type("native");
    if let Some(client) = client {
        request = request
            .with_preregistered_client(client.client_id.clone())
            .with_scopes(client.scopes.iter().cloned());
        if let Some(secret) = &client.client_secret {
            request = request.with_client_secret(secret.clone());
        }
    }
    state.start_authorization(request).await.map_err(|err| {
        format!(
            "{url} cannot start an OAuth login: {err}{}",
            if client.is_none() {
                " (if it has no dynamic client registration, set `oauth = { client_id = … }`)"
            } else {
                ""
            }
        )
    })?;
    let auth_url = state
        .get_authorization_url()
        .await
        .map_err(|err| err.to_string())?;
    show_url(&auth_url);

    let callback = tokio::time::timeout(LOGIN_TIMEOUT, wait_callback(&listener, port))
        .await
        .map_err(|_| "the browser did not come back within 5 minutes".to_owned())??;
    state
        .handle_callback_url(&callback)
        .await
        .map_err(|err| format!("the login was refused: {err}"))?;
    Ok(())
}

/// Waits for `GET /callback?…` on `listener`, answers it and returns its
/// full URL. Other requests (a favicon) get a 404.
async fn wait_callback(listener: &TcpListener, port: u16) -> Result<String, String> {
    loop {
        let (mut stream, _) = listener.accept().await.map_err(|e| e.to_string())?;
        let mut buf = vec![0u8; 8192];
        let n = stream.read(&mut buf).await.map_err(|e| e.to_string())?;
        let request = String::from_utf8_lossy(&buf[..n]);
        let target = request
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .unwrap_or("/");
        if !target.starts_with("/callback") {
            let _ = stream
                .write_all(b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\n\r\n")
                .await;
            continue;
        }
        let body = "<!doctype html><meta charset=utf-8><title>crest</title>\
                    <p>Login received. You can close this tab and go back to crest.</p>";
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/html; charset=utf-8\r\ncontent-length: {}\r\n\
             connection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes()).await;
        return Ok(format!("http://127.0.0.1:{port}{target}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_entries_split_on_character_boundaries() {
        assert_eq!(split("short", 10), ["short"]);
        assert_eq!(split("", 10), [""]);
        assert_eq!(split("abcdefg", 3), ["abc", "def", "g"]);
        assert_eq!(split("ñññññ", 2), ["ññ", "ññ", "ñ"]);
        let long = "x".repeat(2500);
        let parts = split(&long, CHUNK_CHARS);
        assert_eq!(parts.len(), 3);
        assert_eq!(parts.concat(), long);
    }

    #[tokio::test]
    async fn the_callback_is_answered_and_returned() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let browser = tokio::spawn(async move {
            let mut favicon = tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .unwrap();
            favicon
                .write_all(b"GET /favicon.ico HTTP/1.1\r\n\r\n")
                .await
                .unwrap();
            let mut answer = String::new();
            favicon.read_to_string(&mut answer).await.ok();
            let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .unwrap();
            stream
                .write_all(b"GET /callback?code=abc&state=xyz HTTP/1.1\r\nHost: x\r\n\r\n")
                .await
                .unwrap();
            let mut answer = String::new();
            stream.read_to_string(&mut answer).await.unwrap();
            answer
        });
        let url = wait_callback(&listener, port).await.unwrap();
        assert_eq!(
            url,
            format!("http://127.0.0.1:{port}/callback?code=abc&state=xyz")
        );
        assert!(browser.await.unwrap().starts_with("HTTP/1.1 200 OK"));
    }
}
