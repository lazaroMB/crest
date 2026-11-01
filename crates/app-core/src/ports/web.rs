//! Web port: searching the web and reading pages from this machine.
//!
//! The adapter (`adapter-web`) runs `curl` against the configured search
//! backend (DuckDuckGo, SearXNG or Brave) and turns HTML pages into text;
//! the model reaches it through the `web_search` and `web_fetch` tools.
//! Nothing here goes through the model provider.
//!
//! Cancellation is done by dropping the returned future; adapters must kill
//! the request on drop.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Results of a search that does not say how many it wants, unless
/// `app.opt.web.max_results` says otherwise.
pub const DEFAULT_MAX_RESULTS: usize = 8;
/// Most results one search may ask for.
pub const MAX_RESULTS: usize = 10;

/// Errors of [`WebPort`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WebError {
    /// `curl` is not installed.
    #[error("curl is not installed; install it to search and read the web")]
    CurlMissing,
    /// The request took longer than the configured timeout.
    #[error("the request timed out after {seconds} s")]
    Timeout {
        /// The timeout that elapsed.
        seconds: u64,
    },
    /// The server answered with an error status.
    #[error("HTTP {status} from {url}")]
    Status {
        /// The status code.
        status: u16,
        /// The URL that answered.
        url: String,
    },
    /// The URL is malformed or not http(s).
    #[error("invalid URL `{0}`: only http and https URLs can be read")]
    InvalidUrl(String),
    /// The domain is not allowed by `app.opt.web`.
    #[error("{0} is not allowed by app.opt.web (allowed_domains / blocked_domains)")]
    DomainNotAllowed(String),
    /// The page is not text (a PDF, an image...).
    #[error("{url} is {content_type}, not a text page")]
    Unsupported {
        /// The URL read.
        url: String,
        /// Its content type.
        content_type: String,
    },
    /// The answer of the search backend could not be understood.
    #[error("cannot read the answer of {backend}: {detail}")]
    Parse {
        /// The backend.
        backend: String,
        /// What went wrong.
        detail: String,
    },
    /// curl failed for another reason (DNS, TLS, connection...).
    #[error("request failed: {0}")]
    Request(String),
}

/// Input of the `web_search` tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WebSearchRequest {
    /// What to search for, as you would type it in a search engine.
    pub query: String,
    /// How many results to return, 1 to 10; leave it out for the
    /// configured default.
    #[serde(default)]
    pub max_results: Option<usize>,
}

impl WebSearchRequest {
    /// A search for `query` with the default number of results.
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            max_results: None,
        }
    }
}

/// Input of the `web_fetch` tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WebFetchRequest {
    /// The http or https URL of the page to read, usually one a search
    /// returned.
    pub url: String,
}

/// One result of a search.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchHit {
    /// Title of the page.
    pub title: String,
    /// Its URL.
    pub url: String,
    /// The text the engine shows under it.
    pub snippet: String,
}

/// A page read as text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebPage {
    /// The URL after redirects.
    pub url: String,
    /// The `<title>` of an HTML page.
    pub title: Option<String>,
    /// The readable text (HTML converted, other text as is).
    pub text: String,
}

/// Searching and reading the web.
#[async_trait]
pub trait WebPort: Send + Sync {
    /// Up to `max_results` results for `query`.
    async fn search(&self, query: &str, max_results: usize) -> Result<Vec<SearchHit>, WebError>;

    /// The page at `url`, as text.
    async fn fetch(&self, url: &str) -> Result<WebPage, WebError>;

    /// Name of the search backend (`duckduckgo`, `searxng`, `brave`).
    fn backend(&self) -> &str;
}
