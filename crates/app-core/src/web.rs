//! `app.opt.web`: whether the model can search and read the web, with
//! which search backend, and on which domains.
//!
//! ```lua
//! app.opt.web = {
//!   enabled = true,
//!   backend = "duckduckgo",          -- or "searxng" (with searxng_url), "brave"
//!   searxng_url = "https://searx.example.org",
//!   max_results = 8,                 -- results of a search by default
//!   timeout = 20,                    -- seconds per request
//!   allowed_domains = { "docs.rs" }, -- or blocked_domains; never both
//! }
//! ```
//!
//! Every field is optional. The Brave key is not configured here: it is
//! read from `BRAVE_API_KEY` or the keyring by the composition root.

use serde_json::{Map, Value};

use crate::ports::plugin::AppOptions;
use crate::ports::web::{DEFAULT_MAX_RESULTS, MAX_RESULTS};

/// Key of the web options in `app.opt`.
pub const WEB_OPTION: &str = "web";

/// Default of [`WebOptions::timeout_s`].
pub const DEFAULT_TIMEOUT_S: u64 = 20;
/// Longest timeout accepted.
const MAX_TIMEOUT_S: u64 = 300;

const FIELDS: [&str; 7] = [
    "enabled",
    "backend",
    "searxng_url",
    "max_results",
    "timeout",
    "allowed_domains",
    "blocked_domains",
];

/// A malformed `app.opt.web`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WebConfigError {
    /// Not a table.
    #[error("app.opt.{WEB_OPTION} must be a table")]
    NotATable,
    /// A field crest does not know.
    #[error("app.opt.{WEB_OPTION}: unknown field `{0}` (known: {known})", known = FIELDS.join(", "))]
    UnknownField(String),
    /// A field of the wrong type or out of range.
    #[error("app.opt.{WEB_OPTION}.{field} {problem}")]
    BadField {
        /// The field.
        field: &'static str,
        /// What is wrong with it.
        problem: String,
    },
    /// An unknown backend.
    #[error(
        "app.opt.{WEB_OPTION}.backend must be \"duckduckgo\", \"searxng\" or \"brave\", not `{0}`"
    )]
    BadBackend(String),
    /// `searxng` without its URL.
    #[error("app.opt.{WEB_OPTION}.backend = \"searxng\" needs searxng_url")]
    SearxngWithoutUrl,
    /// Both domain lists.
    #[error("app.opt.{WEB_OPTION}: set allowed_domains or blocked_domains, not both")]
    BothDomainLists,
}

/// The search engine `web_search` asks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchBackend {
    /// `html.duckduckgo.com`, no key (the default).
    DuckDuckGo,
    /// A SearXNG instance's JSON API.
    Searxng {
        /// Base URL of the instance.
        url: String,
    },
    /// The Brave Search API; its key comes from `BRAVE_API_KEY` or the
    /// keyring.
    Brave,
}

impl SearchBackend {
    /// Name of the backend in the config and the help.
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Self::DuckDuckGo => "duckduckgo",
            Self::Searxng { .. } => "searxng",
            Self::Brave => "brave",
        }
    }
}

/// Which domains the web tools may reach.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum DomainRules {
    /// Any domain (the default).
    #[default]
    Any,
    /// Only these domains and their subdomains.
    Allow(Vec<String>),
    /// Any domain but these and their subdomains.
    Block(Vec<String>),
}

impl DomainRules {
    /// Whether `url` may be read (and listed in search results).
    #[must_use]
    pub fn allows(&self, url: &str) -> bool {
        let Some(host) = host_of(url) else {
            return false;
        };
        let matches = |domains: &[String]| {
            domains.iter().any(|domain| {
                host == *domain
                    || host
                        .strip_suffix(domain.as_str())
                        .is_some_and(|rest| rest.ends_with('.'))
            })
        };
        match self {
            Self::Any => true,
            Self::Allow(domains) => matches(domains),
            Self::Block(domains) => !matches(domains),
        }
    }
}

/// The lower-cased host of an http(s) `url`, without port or user info.
#[must_use]
pub fn host_of(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit('@').next()?;
    let host = if host.starts_with('[') {
        host.split_inclusive(']').next()?
    } else {
        host.split(':').next()?
    };
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

/// The web options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebOptions {
    /// Whether the web tools are registered at all.
    pub enabled: bool,
    /// The search engine.
    pub backend: SearchBackend,
    /// Results of a search that does not say how many it wants.
    pub max_results: usize,
    /// Seconds per request.
    pub timeout_s: u64,
    /// Which domains can be read.
    pub domains: DomainRules,
}

impl Default for WebOptions {
    fn default() -> Self {
        Self {
            enabled: true,
            backend: SearchBackend::DuckDuckGo,
            max_results: DEFAULT_MAX_RESULTS,
            timeout_s: DEFAULT_TIMEOUT_S,
            domains: DomainRules::Any,
        }
    }
}

impl WebOptions {
    /// The `web` option of `app.opt`, or the defaults when it is not set.
    ///
    /// # Errors
    ///
    /// [`WebConfigError`] when it is malformed.
    pub fn from_options(options: &AppOptions) -> Result<Self, WebConfigError> {
        match options.extra.get(WEB_OPTION) {
            None | Some(Value::Null) => Ok(Self::default()),
            Some(value) => Self::from_json(value),
        }
    }

    /// Parses the `app.opt.web` table.
    ///
    /// # Errors
    ///
    /// [`WebConfigError`] when it is malformed.
    pub fn from_json(value: &Value) -> Result<Self, WebConfigError> {
        let empty = Map::new();
        let table = match value {
            Value::Object(table) => table,
            // `{}` in Lua can come as an empty list.
            Value::Array(items) if items.is_empty() => &empty,
            _ => return Err(WebConfigError::NotATable),
        };
        if let Some(unknown) = table.keys().find(|key| !FIELDS.contains(&key.as_str())) {
            return Err(WebConfigError::UnknownField(unknown.clone()));
        }
        let mut out = Self::default();
        if let Some(enabled) = table.get("enabled") {
            out.enabled = enabled.as_bool().ok_or(WebConfigError::BadField {
                field: "enabled",
                problem: "must be true or false".into(),
            })?;
        }
        if let Some(n) = table.get("max_results") {
            out.max_results = whole(n, "max_results", MAX_RESULTS as u64)? as usize;
        }
        if let Some(n) = table.get("timeout") {
            out.timeout_s = whole(n, "timeout", MAX_TIMEOUT_S)?;
        }
        let url = match table.get("searxng_url") {
            None => None,
            Some(Value::String(url))
                if url.starts_with("http://") || url.starts_with("https://") =>
            {
                Some(url.trim_end_matches('/').to_owned())
            }
            Some(_) => {
                return Err(WebConfigError::BadField {
                    field: "searxng_url",
                    problem: "must be an http or https URL".into(),
                });
            }
        };
        out.backend = match table.get("backend") {
            None => SearchBackend::DuckDuckGo,
            Some(Value::String(name)) => match name.as_str() {
                "duckduckgo" => SearchBackend::DuckDuckGo,
                "brave" => SearchBackend::Brave,
                "searxng" => SearchBackend::Searxng {
                    url: url.ok_or(WebConfigError::SearxngWithoutUrl)?,
                },
                other => return Err(WebConfigError::BadBackend(other.to_owned())),
            },
            Some(other) => return Err(WebConfigError::BadBackend(other.to_string())),
        };
        let allowed = domains(table.get("allowed_domains"), "allowed_domains")?;
        let blocked = domains(table.get("blocked_domains"), "blocked_domains")?;
        out.domains = match (allowed.is_empty(), blocked.is_empty()) {
            (true, true) => DomainRules::Any,
            (false, true) => DomainRules::Allow(allowed),
            (true, false) => DomainRules::Block(blocked),
            (false, false) => return Err(WebConfigError::BothDomainLists),
        };
        Ok(out)
    }
}

/// A whole number from 1 to `max`.
fn whole(value: &Value, field: &'static str, max: u64) -> Result<u64, WebConfigError> {
    value
        .as_u64()
        .or_else(|| {
            value
                .as_f64()
                .filter(|f| f.fract() == 0.0 && *f >= 0.0)
                .map(|f| f as u64)
        })
        .filter(|n| (1..=max).contains(n))
        .ok_or(WebConfigError::BadField {
            field,
            problem: format!("must be a whole number from 1 to {max}"),
        })
}

/// A list of domains (`docs.rs`, `example.com`), lower-cased.
fn domains(value: Option<&Value>, field: &'static str) -> Result<Vec<String>, WebConfigError> {
    let bad = || WebConfigError::BadField {
        field,
        problem: "must be a list of domain names such as \"docs.rs\"".into(),
    };
    let items = match value {
        None => return Ok(Vec::new()),
        Some(Value::Array(items)) => items,
        Some(Value::Object(map)) if map.is_empty() => return Ok(Vec::new()),
        Some(_) => return Err(bad()),
    };
    items
        .iter()
        .map(|item| {
            let domain = item.as_str().ok_or_else(bad)?.trim().to_ascii_lowercase();
            let valid =
                !domain.is_empty() && !domain.contains(['/', ':', ' ']) && !domain.starts_with('.');
            valid.then_some(domain).ok_or_else(bad)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn options(web: Value) -> AppOptions {
        let mut options = AppOptions::default();
        options.extra.insert(WEB_OPTION.into(), web);
        options
    }

    #[test]
    fn defaults_when_unset_or_empty() {
        assert_eq!(
            WebOptions::from_options(&AppOptions::default()),
            Ok(WebOptions::default())
        );
        assert_eq!(WebOptions::from_json(&json!([])), Ok(WebOptions::default()));
        let default = WebOptions::default();
        assert!(default.enabled);
        assert_eq!(default.backend, SearchBackend::DuckDuckGo);
    }

    #[test]
    fn reads_every_field() {
        let web = WebOptions::from_options(&options(json!({
            "enabled": true,
            "backend": "searxng",
            "searxng_url": "https://searx.example.org/",
            "max_results": 5,
            "timeout": 30.0,
            "blocked_domains": ["Pinterest.com"],
        })))
        .unwrap();
        assert_eq!(
            web,
            WebOptions {
                enabled: true,
                backend: SearchBackend::Searxng {
                    url: "https://searx.example.org".into()
                },
                max_results: 5,
                timeout_s: 30,
                domains: DomainRules::Block(vec!["pinterest.com".into()]),
            }
        );
    }

    #[test]
    fn reports_what_is_wrong() {
        let cases = [
            (json!(3), "app.opt.web must be a table"),
            (json!({ "engine": "x" }), "unknown field `engine`"),
            (json!({ "backend": "google" }), "not `google`"),
            (json!({ "backend": "searxng" }), "needs searxng_url"),
            (
                json!({ "max_results": 0 }),
                "max_results must be a whole number from 1 to 10",
            ),
            (json!({ "timeout": 2.5 }), "timeout must be a whole number"),
            (json!({ "enabled": "yes" }), "enabled must be true or false"),
            (
                json!({ "allowed_domains": ["https://docs.rs"] }),
                "allowed_domains must be a list",
            ),
            (
                json!({ "allowed_domains": ["a.com"], "blocked_domains": ["b.com"] }),
                "not both",
            ),
        ];
        for (value, expected) in cases {
            let err = WebOptions::from_json(&value).unwrap_err().to_string();
            assert!(err.contains(expected), "{value}: {err}");
        }
    }

    #[test]
    fn domain_rules_match_subdomains_only() {
        let allow = DomainRules::Allow(vec!["rust-lang.org".into()]);
        assert!(allow.allows("https://blog.rust-lang.org/2026/10/1.99"));
        assert!(allow.allows("https://RUST-LANG.org"));
        assert!(!allow.allows("https://evilrust-lang.org/"));
        assert!(!allow.allows("https://example.com/?u=rust-lang.org"));
        let block = DomainRules::Block(vec!["pinterest.com".into()]);
        assert!(!block.allows("http://user@www.pinterest.com:8080/pin"));
        assert!(block.allows("https://docs.rs"));
        assert!(!DomainRules::Any.allows("ftp://example.com"));
    }
}
