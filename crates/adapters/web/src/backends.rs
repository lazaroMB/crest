//! The search backends: the request each one needs and how to read its
//! answer.
//!
//! - **DuckDuckGo**: the HTML page of `html.duckduckgo.com` (a POST with
//!   `q`), no key. Results are `a.result__a` links (sometimes through a
//!   `/l/?uddg=` redirect) followed by an `.result__snippet`; ads are
//!   skipped.
//! - **SearXNG**: `GET <instance>/search?format=json`, `results[]`.
//! - **Brave**: `GET api.search.brave.com/res/v1/web/search`, with the key
//!   in the `X-Subscription-Token` header, `web.results[]`.

use std::sync::LazyLock;

use application::ports::web::{SearchHit, WebError};
use application::web::SearchBackend;
use regex::Regex;
use serde_json::Value;

use crate::curl::{Request, Response, decode, encode};
use crate::html::{decode_entities, fragment_text};

/// The DuckDuckGo endpoint.
pub const DUCKDUCKGO_URL: &str = "https://html.duckduckgo.com/html/";
/// The Brave endpoint.
pub const BRAVE_URL: &str = "https://api.search.brave.com/res/v1/web/search";

#[allow(clippy::expect_used)] // static patterns, covered by the tests
static LINK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?s)<a\b([^>]*)>(.*?)</a>"#).expect("valid regex"));
#[allow(clippy::expect_used)]
static HREF: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"href="([^"]*)""#).expect("valid regex"));
#[allow(clippy::expect_used)]
static SNIPPET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?s)<(a|td|div)\b[^>]*class="result__snippet"[^>]*>(.*?)</(?:a|td|div)>"#)
        .expect("valid regex")
});

/// The request that searches `query` on `backend` (`key` for Brave).
pub fn request(backend: &SearchBackend, key: Option<&str>, query: &str, count: usize) -> Request {
    match backend {
        SearchBackend::DuckDuckGo => Request::get(DUCKDUCKGO_URL).form("q", query),
        SearchBackend::Searxng { url } => {
            Request::get(format!("{url}/search?q={}&format=json", encode(query)))
                .header("Accept: application/json")
        }
        SearchBackend::Brave => {
            let request = Request::get(format!("{BRAVE_URL}?q={}&count={count}", encode(query)))
                .header("Accept: application/json");
            match key {
                Some(key) => request.header(format!("X-Subscription-Token: {key}")),
                None => request,
            }
        }
    }
}

/// The results in `response` from `backend`.
///
/// # Errors
///
/// [`WebError::Status`] for an error status, [`WebError::Parse`] when the
/// answer is not what the backend sends (or DuckDuckGo refused the search).
pub fn parse(backend: &SearchBackend, response: &Response) -> Result<Vec<SearchHit>, WebError> {
    let name = backend.name();
    if response.status == 429 || (name == "duckduckgo" && response.status == 202) {
        return Err(refused(name));
    }
    if response.status >= 400 {
        return Err(WebError::Status {
            status: response.status,
            url: response.url.clone(),
        });
    }
    let body = String::from_utf8_lossy(&response.body);
    match backend {
        SearchBackend::DuckDuckGo => parse_duckduckgo(&body),
        SearchBackend::Searxng { .. } => parse_json(name, &body, &["results"], "content"),
        SearchBackend::Brave => parse_json(name, &body, &["web", "results"], "description"),
    }
}

fn refused(backend: &str) -> WebError {
    WebError::Parse {
        backend: backend.to_owned(),
        detail: "the engine refused the search (too many in a short time); wait a minute, or \
                 set app.opt.web.backend to \"searxng\" or \"brave\""
            .to_owned(),
    }
}

/// The results of a DuckDuckGo HTML page.
///
/// # Errors
///
/// [`WebError::Parse`] when the page is DuckDuckGo's bot check.
pub fn parse_duckduckgo(html: &str) -> Result<Vec<SearchHit>, WebError> {
    if html.contains("anomaly-modal") || html.contains("challenge-form") {
        return Err(refused("duckduckgo"));
    }
    // Title links and snippets, in page order.
    let mut marks: Vec<(usize, Mark)> = Vec::new();
    for link in LINK.captures_iter(html) {
        let (Some(whole), attrs, inner) = (link.get(0), &link[1], &link[2]) else {
            continue;
        };
        if !attrs.contains("result__a") {
            continue;
        }
        if let Some(url) = HREF
            .captures(attrs)
            .and_then(|href| result_url(&decode_entities(&href[1])))
        {
            marks.push((whole.start(), Mark::Title(fragment_text(inner), url)));
        }
    }
    for snippet in SNIPPET.captures_iter(html) {
        if let Some(whole) = snippet.get(0) {
            marks.push((whole.start(), Mark::Snippet(fragment_text(&snippet[2]))));
        }
    }
    marks.sort_by_key(|(at, _)| *at);
    let mut hits: Vec<SearchHit> = Vec::new();
    for (_, mark) in marks {
        match mark {
            Mark::Title(title, url) => hits.push(SearchHit {
                title,
                url,
                snippet: String::new(),
            }),
            Mark::Snippet(text) => {
                if let Some(hit) = hits.last_mut().filter(|hit| hit.snippet.is_empty()) {
                    hit.snippet = text;
                }
            }
        }
    }
    Ok(hits)
}

enum Mark {
    Title(String, String),
    Snippet(String),
}

/// The target of a result link: direct, or the `uddg` of a redirect. Ads
/// (`/y.js`) and internal links give `None`.
fn result_url(href: &str) -> Option<String> {
    let href = if href.starts_with("//") {
        format!("https:{href}")
    } else {
        href.to_owned()
    };
    if href.contains("duckduckgo.com/y.js") {
        return None;
    }
    if href.contains("duckduckgo.com/l/") {
        let query = href.split_once('?')?.1;
        let target = query
            .split('&')
            .find_map(|pair| pair.strip_prefix("uddg="))?;
        return Some(decode(target)).filter(|url| url.starts_with("http"));
    }
    (href.starts_with("http") && !href.contains("duckduckgo.com")).then_some(href)
}

/// Results of a JSON backend: the array at `path`, each with `title`,
/// `url` and the snippet field `snippet`.
fn parse_json(
    backend: &str,
    body: &str,
    path: &[&str],
    snippet: &str,
) -> Result<Vec<SearchHit>, WebError> {
    let bad = |detail: String| WebError::Parse {
        backend: backend.to_owned(),
        detail,
    };
    let json: Value = serde_json::from_str(body).map_err(|err| bad(format!("not JSON ({err})")))?;
    let mut node = &json;
    for key in path {
        match node.get(key) {
            Some(next) => node = next,
            // Brave leaves `web` out when nothing matched.
            None if backend == "brave" => return Ok(Vec::new()),
            None => return Err(bad(format!("no `{}` in the answer", path.join(".")))),
        }
    }
    let items = node
        .as_array()
        .ok_or_else(|| bad(format!("`{}` is not a list", path.join("."))))?;
    Ok(items
        .iter()
        .filter_map(|item| {
            let url = item.get("url")?.as_str()?.to_owned();
            let text = |key: &str| {
                item.get(key)
                    .and_then(Value::as_str)
                    .map(fragment_text)
                    .unwrap_or_default()
            };
            Some(SearchHit {
                title: text("title"),
                url,
                snippet: text(snippet),
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DDG: &str = include_str!("../tests/fixtures/ddg.html");
    const SEARXNG: &str = include_str!("../tests/fixtures/searxng.json");
    const BRAVE: &str = include_str!("../tests/fixtures/brave.json");

    fn response(status: u16, body: &str) -> Response {
        Response {
            status,
            content_type: "text/html".into(),
            url: "https://engine.example/search".into(),
            body: body.as_bytes().to_vec(),
        }
    }

    #[test]
    fn duckduckgo_results_from_a_real_page() {
        let hits = parse(&SearchBackend::DuckDuckGo, &response(200, DDG)).unwrap();
        assert_eq!(hits.len(), 10, "{hits:#?}");
        assert_eq!(
            hits[0].url,
            "https://blog.rust-lang.org/2025/09/18/Rust-1.90.0/"
        );
        assert!(hits[0].title.contains("Rust 1.90.0"), "{:?}", hits[0].title);
        assert!(
            hits[0]
                .snippet
                .starts_with("The Rust team is happy to announce"),
            "{:?}",
            hits[0].snippet
        );
        assert!(hits[0].snippet.contains("don't"), "entities decoded");
        assert!(hits.iter().all(|hit| hit.url.starts_with("https://")));
    }

    #[test]
    fn duckduckgo_redirects_ads_and_refusals() {
        let html = r#"
            <a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fdocs.rs%2Fserde&amp;rut=x">serde - <b>Rust</b></a>
            <a class="result__snippet" href="x">Serialization framework</a>
            <a rel="nofollow" class="result__a" href="https://duckduckgo.com/y.js?ad_provider=x">Ad</a>
            <a class="result__snippet" href="x">Buy now</a>
        "#;
        let hits = parse_duckduckgo(html).unwrap();
        assert_eq!(
            hits,
            [SearchHit {
                title: "serde - Rust".into(),
                url: "https://docs.rs/serde".into(),
                snippet: "Serialization framework".into(),
            }]
        );
        let refused = parse(
            &SearchBackend::DuckDuckGo,
            &response(202, "<div class=\"anomaly-modal\">"),
        );
        assert!(matches!(refused, Err(WebError::Parse { .. })));
    }

    #[test]
    fn searxng_and_brave_json() {
        let searx = SearchBackend::Searxng {
            url: "https://searx.example".into(),
        };
        let hits = parse(&searx, &response(200, SEARXNG)).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].title, "Announcing Rust 1.90.0");
        assert_eq!(
            hits[0].snippet,
            "The Rust team is happy to announce 1.90.0."
        );
        let hits = parse(&SearchBackend::Brave, &response(200, BRAVE)).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[1].url, "https://releases.rs/docs/1.90.0/");
        assert_eq!(
            hits[0].snippet,
            "The Rust team is happy to announce 1.90.0."
        );
        assert_eq!(
            parse(&SearchBackend::Brave, &response(200, "{}")).unwrap(),
            []
        );
        assert!(matches!(
            parse(&searx, &response(403, "forbidden")),
            Err(WebError::Status { status: 403, .. })
        ));
        assert!(matches!(
            parse(&searx, &response(200, "<html>")),
            Err(WebError::Parse { .. })
        ));
    }

    #[test]
    fn requests_per_backend() {
        let ddg = request(&SearchBackend::DuckDuckGo, None, "rust 1.90", 8);
        assert_eq!(ddg.url, DUCKDUCKGO_URL);
        assert_eq!(ddg.form, [("q".to_owned(), "rust 1.90".to_owned())]);
        let searx = request(
            &SearchBackend::Searxng {
                url: "https://searx.example".into(),
            },
            None,
            "rust 1.90",
            8,
        );
        assert_eq!(
            searx.url,
            "https://searx.example/search?q=rust+1.90&format=json"
        );
        let brave = request(&SearchBackend::Brave, Some("k3y"), "rust", 5);
        assert_eq!(brave.url, format!("{BRAVE_URL}?q=rust&count=5"));
        assert!(
            brave
                .headers
                .contains(&"X-Subscription-Token: k3y".to_owned())
        );
    }
}
