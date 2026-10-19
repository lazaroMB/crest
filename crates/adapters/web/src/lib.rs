//! WebPort adapter: web search and page reading over `curl`.
//!
//! [`CurlWeb`] runs one `curl` process per request (see [`curl`] for how
//! it is invoked: no shell, no `.curlrc`, a minimal environment, headers
//! and keys on stdin). Searches go to the backend of `app.opt.web`
//! ([`backends`]); pages are converted to text ([`html`]). The domain rules
//! of `app.opt.web` apply to the URL read, to where it redirected and to
//! the search results.

pub mod backends;
pub mod curl;
pub mod html;

use std::ffi::OsString;

use application::ports::web::{SearchHit, WebError, WebPage, WebPort};
use application::web::{SearchBackend, WebOptions};
use async_trait::async_trait;

/// Environment variable and keyring entry (service `crest`) of the Brave
/// Search API key.
pub const BRAVE_KEY_NAME: &str = "BRAVE_API_KEY";
/// Keyring service of the key.
const KEYRING_SERVICE: &str = "crest";

/// The Brave key from `BRAVE_API_KEY`, else from the keyring (service
/// `crest`, entry `BRAVE_API_KEY`). The keyring call may block (D-Bus,
/// Keychain): call this through `spawn_blocking`.
#[must_use]
pub fn brave_key_from_env_or_keyring() -> Option<String> {
    if let Ok(key) = std::env::var(BRAVE_KEY_NAME)
        && !key.trim().is_empty()
    {
        return Some(key.trim().to_owned());
    }
    let entry = keyring::Entry::new(KEYRING_SERVICE, BRAVE_KEY_NAME)
        .inspect_err(|err| tracing::debug!(error = %err, "keyring unavailable"))
        .ok()?;
    entry
        .get_password()
        .inspect_err(|err| tracing::debug!(error = %err, "no Brave key in the keyring"))
        .ok()
        .map(|key| key.trim().to_owned())
        .filter(|key| !key.is_empty())
}

/// [`WebPort`] over the `curl` on `PATH`.
#[derive(Clone)]
pub struct CurlWeb {
    options: WebOptions,
    brave_key: Option<String>,
    program: OsString,
    env: Vec<(OsString, OsString)>,
}

impl std::fmt::Debug for CurlWeb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CurlWeb")
            .field("options", &self.options)
            .field("brave_key", &self.brave_key.as_ref().map(|_| "<redacted>"))
            .finish_non_exhaustive()
    }
}

impl CurlWeb {
    /// A web with `options`; `brave_key` is the key of the Brave backend.
    #[must_use]
    pub fn new(options: WebOptions, brave_key: Option<String>) -> Self {
        Self {
            options,
            brave_key,
            program: OsString::from("curl"),
            env: curl::environment(std::env::vars_os()),
        }
    }

    /// Uses the curl at `program` instead of the one on `PATH`.
    #[must_use]
    pub fn with_program(mut self, program: impl Into<OsString>) -> Self {
        self.program = program.into();
        self
    }

    /// The options in use.
    #[must_use]
    pub fn options(&self) -> &WebOptions {
        &self.options
    }

    /// Whether curl can be started (`curl --version` succeeds).
    pub async fn curl_available(&self) -> bool {
        tokio::process::Command::new(&self.program)
            .arg("--version")
            .env_clear()
            .envs(self.env.iter().cloned())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await
            .is_ok_and(|status| status.success())
    }

    async fn run(&self, request: &curl::Request) -> Result<curl::Response, WebError> {
        tracing::debug!(url = %request.url, "web request");
        curl::run(&self.program, &self.env, request, self.options.timeout_s).await
    }
}

#[async_trait]
impl WebPort for CurlWeb {
    async fn search(&self, query: &str, max_results: usize) -> Result<Vec<SearchHit>, WebError> {
        let backend = &self.options.backend;
        if *backend == SearchBackend::Brave && self.brave_key.is_none() {
            return Err(WebError::Parse {
                backend: "brave".into(),
                detail: "no key: set BRAVE_API_KEY or store it in the keyring (crest / \
                         BRAVE_API_KEY)"
                    .into(),
            });
        }
        let request = backends::request(backend, self.brave_key.as_deref(), query, max_results);
        let response = self.run(&request).await?;
        let hits = backends::parse(backend, &response)?;
        Ok(hits
            .into_iter()
            .filter(|hit| self.options.domains.allows(&hit.url))
            .take(max_results)
            .collect())
    }

    async fn fetch(&self, url: &str) -> Result<WebPage, WebError> {
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Err(WebError::InvalidUrl(url.to_owned()));
        }
        if !self.options.domains.allows(url) {
            return Err(WebError::DomainNotAllowed(url.to_owned()));
        }
        let response = self.run(&curl::Request::get(url)).await?;
        let final_url = if response.url.is_empty() {
            url.to_owned()
        } else {
            response.url.clone()
        };
        if !self.options.domains.allows(&final_url) {
            return Err(WebError::DomainNotAllowed(final_url));
        }
        if response.status >= 400 {
            return Err(WebError::Status {
                status: response.status,
                url: final_url,
            });
        }
        let (title, text) = html::page_text(&final_url, &response.content_type, &response.body)?;
        Ok(WebPage {
            url: final_url,
            title,
            text,
        })
    }

    fn backend(&self) -> &str {
        self.options.backend.name()
    }
}

#[cfg(test)]
mod tests {
    use application::web::DomainRules;

    use super::*;

    #[tokio::test]
    async fn a_missing_curl_is_reported() {
        let web = CurlWeb::new(WebOptions::default(), None).with_program("/nonexistent/curl");
        assert!(!web.curl_available().await);
        assert_eq!(
            web.fetch("https://example.com").await,
            Err(WebError::CurlMissing)
        );
    }

    #[tokio::test]
    async fn domains_and_schemes_are_checked_before_curl_runs() {
        let options = WebOptions {
            domains: DomainRules::Allow(vec!["docs.rs".into()]),
            ..WebOptions::default()
        };
        let web = CurlWeb::new(options, None).with_program("/nonexistent/curl");
        assert_eq!(
            web.fetch("https://example.com/").await,
            Err(WebError::DomainNotAllowed("https://example.com/".into()))
        );
        assert!(matches!(
            web.fetch("file:///etc/passwd").await,
            Err(WebError::InvalidUrl(_))
        ));
        assert_eq!(
            web.fetch("https://docs.rs/serde").await,
            Err(WebError::CurlMissing),
            "an allowed domain gets to curl"
        );
    }

    #[tokio::test]
    async fn brave_without_a_key_says_how_to_set_it() {
        let options = WebOptions {
            backend: SearchBackend::Brave,
            ..WebOptions::default()
        };
        let err = CurlWeb::new(options, None)
            .search("x", 5)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("BRAVE_API_KEY"), "{err}");
        let debug = format!(
            "{:?}",
            CurlWeb::new(WebOptions::default(), Some("s3cret".into()))
        );
        assert!(!debug.contains("s3cret"), "{debug}");
    }

    /// Real network: `cargo test -p adapter-web -- --ignored`.
    #[tokio::test]
    #[ignore = "needs curl and the network"]
    async fn live_search_and_fetch() {
        let web = CurlWeb::new(WebOptions::default(), None);
        let hits = web.search("rust programming language", 5).await.unwrap();
        assert!(!hits.is_empty());
        let page = web.fetch("https://www.rust-lang.org/").await.unwrap();
        assert!(page.text.contains("Rust"), "{}", page.text);
    }
}
