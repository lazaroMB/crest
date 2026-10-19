//! The web tools of the model: `web_search` and `web_fetch`, thin
//! [`TypedTool`]s over the [`WebPort`].
//!
//! Both only read (a search, a GET), so they declare [`ToolEffect::Read`]:
//! they never ask for approval, run in every permission mode and in the
//! read-only agents (`explore`, `task`). A `deny` rule on their subject
//! (the query or the URL) still blocks them.
//!
//! Outputs are plain text in a fixed shape that
//! [`web_sources`](crate::usecases::web_sources) reads back:
//!
//! ```text
//! [1] Title
//! URL: https://example.com/page
//! The snippet the engine shows.
//! ```
//!
//! and, for a page, `Source: <url>` and `Title: <title>` lines before the
//! text.

use std::sync::Arc;

use async_trait::async_trait;
use domain::permission::ToolEffect;
use futures::FutureExt;

use super::budget::ResultBudget;
use super::registry::{
    PreparedCall, RegisterError, ToolContext, ToolError, ToolOutput, ToolRegistry, TypedTool,
};
use crate::ports::web::{MAX_RESULTS, SearchHit, WebFetchRequest, WebPort, WebSearchRequest};

/// Name of the search tool.
pub const WEB_SEARCH_TOOL: &str = "web_search";
/// Name of the page tool.
pub const WEB_FETCH_TOOL: &str = "web_fetch";

/// Prefix of the URL line of a search result.
pub const URL_PREFIX: &str = "URL: ";
/// Prefix of the URL line of a fetched page.
pub const SOURCE_PREFIX: &str = "Source: ";
/// Prefix of the title line of a fetched page.
pub const TITLE_PREFIX: &str = "Title: ";

const WEB_SEARCH_DESCRIPTION: &str = "\
Searches the web and returns the results: title, URL and snippet of each.
- Use it for anything outside the workspace: documentation, releases, error messages, \
libraries, news, comparisons.
- To research a question, search, then read the 2-3 most relevant results with web_fetch \
before answering; search again with other words when the results are poor.
- Snippets are short and can be outdated: read the page before relying on it.
- Cite the URL of every source you use in your answer.";

const WEB_FETCH_DESCRIPTION: &str = "\
Reads a web page and returns its text (HTML converted to plain text; navigation, scripts \
and styles removed), with its final URL and title.
- Use it on URLs from web_search or given by the user; only http and https.
- Long pages are cut; PDFs and other binary files cannot be read.
- Cite the URL of the page when you use what it says.";

/// The search results as the model reads them.
#[must_use]
pub fn render_hits(hits: &[SearchHit]) -> Vec<String> {
    hits.iter()
        .enumerate()
        .map(|(ix, hit)| {
            let mut item = format!("[{}] {}\n{URL_PREFIX}{}", ix + 1, hit.title, hit.url);
            if !hit.snippet.is_empty() {
                item.push('\n');
                item.push_str(&hit.snippet);
            }
            item.push('\n');
            item
        })
        .collect()
}

/// `web_search`.
pub struct WebSearchTool {
    web: Arc<dyn WebPort>,
    budget: ResultBudget,
    default_results: usize,
}

impl WebSearchTool {
    /// The tool over `web`, cutting results to `budget`; a search that
    /// does not say how many results it wants gets `default_results`.
    #[must_use]
    pub fn new(web: Arc<dyn WebPort>, budget: ResultBudget, default_results: usize) -> Self {
        Self {
            web,
            budget,
            default_results: default_results.clamp(1, MAX_RESULTS),
        }
    }
}

#[async_trait]
impl TypedTool for WebSearchTool {
    type Input = WebSearchRequest;

    fn name(&self) -> &str {
        WEB_SEARCH_TOOL
    }

    fn description(&self) -> &str {
        WEB_SEARCH_DESCRIPTION
    }

    fn effect(&self, _input: &WebSearchRequest) -> ToolEffect {
        ToolEffect::Read
    }

    async fn prepare(
        &self,
        input: WebSearchRequest,
        _ctx: &ToolContext,
    ) -> Result<PreparedCall, ToolError> {
        let query = input.query.trim().to_owned();
        if query.is_empty() {
            return Err(ToolError::InvalidInput {
                tool: WEB_SEARCH_TOOL.to_owned(),
                message: "`query` is empty".to_owned(),
            });
        }
        let wanted = input
            .max_results
            .map_or(self.default_results, |n| n.clamp(1, MAX_RESULTS));
        let web = Arc::clone(&self.web);
        let budget = self.budget;
        let subject = format!("\"{query}\"");
        let run = async move {
            let hits = match web.search(&query, wanted).await {
                Ok(hits) => hits,
                Err(err) => return Ok(ToolOutput::error(format!("web search failed: {err}"))),
            };
            if hits.is_empty() {
                return Ok(ToolOutput::text(format!(
                    "No results for \"{query}\". Try other words or a broader query."
                )));
            }
            Ok(ToolOutput::text(budget.truncate_list(&render_hits(&hits))))
        };
        Ok(PreparedCall::new(subject, run.boxed()))
    }
}

/// `web_fetch`.
pub struct WebFetchTool {
    web: Arc<dyn WebPort>,
    budget: ResultBudget,
}

impl WebFetchTool {
    /// The tool over `web`, cutting pages to `budget`.
    #[must_use]
    pub fn new(web: Arc<dyn WebPort>, budget: ResultBudget) -> Self {
        Self { web, budget }
    }
}

#[async_trait]
impl TypedTool for WebFetchTool {
    type Input = WebFetchRequest;

    fn name(&self) -> &str {
        WEB_FETCH_TOOL
    }

    fn description(&self) -> &str {
        WEB_FETCH_DESCRIPTION
    }

    fn effect(&self, _input: &WebFetchRequest) -> ToolEffect {
        ToolEffect::Read
    }

    async fn prepare(
        &self,
        input: WebFetchRequest,
        _ctx: &ToolContext,
    ) -> Result<PreparedCall, ToolError> {
        let url = input.url.trim().to_owned();
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Err(ToolError::InvalidInput {
                tool: WEB_FETCH_TOOL.to_owned(),
                message: format!("`{url}` is not an http or https URL"),
            });
        }
        let web = Arc::clone(&self.web);
        let budget = self.budget;
        let subject = url.clone();
        let run = async move {
            let page = match web.fetch(&url).await {
                Ok(page) => page,
                Err(err) => return Ok(ToolOutput::error(format!("cannot read {url}: {err}"))),
            };
            let mut head = format!("{SOURCE_PREFIX}{}\n", page.url);
            if let Some(title) = page.title.as_deref().filter(|t| !t.trim().is_empty()) {
                head.push_str(&format!("{TITLE_PREFIX}{}\n", title.trim()));
            }
            head.push('\n');
            let room = ResultBudget::new(
                budget
                    .max_tokens
                    .saturating_sub(head.len().div_ceil(4) + 64),
            );
            let text = if page.text.trim().is_empty() {
                "(the page has no readable text)".to_owned()
            } else {
                room.truncate_text(page.text.trim())
            };
            Ok(ToolOutput::text(format!("{head}{text}")))
        };
        Ok(PreparedCall::new(subject, run.boxed()))
    }
}

/// Registers `web_search` and `web_fetch` over `web` in `registry`, cut to
/// the registry's budget; a search returns `default_results` results
/// unless the model asks for another number.
///
/// # Errors
///
/// [`RegisterError::Duplicate`] when a tool with one of their names is
/// already registered.
pub fn register_web_tools(
    registry: &mut ToolRegistry,
    web: Arc<dyn WebPort>,
    default_results: usize,
) -> Result<(), RegisterError> {
    let budget = registry.budget();
    registry.register_typed(WebSearchTool::new(
        Arc::clone(&web),
        budget,
        default_results,
    ))?;
    registry.register_typed(WebFetchTool::new(web, budget))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use domain::conversation::{SessionId, ToolUseId, TurnId};
    use serde_json::{Value, json};

    use super::*;
    use crate::ports::web::{WebError, WebPage};
    use crate::testing::web::ScriptedWeb;
    use crate::tools::registry::{Tool, Typed};

    fn ctx() -> ToolContext {
        ToolContext::new(SessionId::new(), TurnId::new(), ToolUseId::new("toolu_1"))
    }

    async fn call(tool: &dyn Tool, input: Value) -> (String, ToolOutput) {
        let prepared = tool.prepare(input, &ctx()).await.unwrap();
        let subject = prepared.subject.clone();
        (subject, prepared.run.await.unwrap())
    }

    fn hit(n: u32) -> SearchHit {
        SearchHit {
            title: format!("Result {n}"),
            url: format!("https://example.com/{n}"),
            snippet: format!("snippet {n}"),
        }
    }

    #[tokio::test]
    async fn search_lists_title_url_and_snippet() {
        let web = Arc::new(ScriptedWeb::new().on_search("rust 1.99", vec![hit(1), hit(2)]));
        let tool = Typed(WebSearchTool::new(web.clone(), ResultBudget::default(), 5));
        let (subject, out) = call(&tool, json!({ "query": " rust 1.99 " })).await;
        assert_eq!(subject, "\"rust 1.99\"");
        assert!(!out.is_error);
        assert_eq!(
            out.text_content(),
            "[1] Result 1\nURL: https://example.com/1\nsnippet 1\n\n\
             [2] Result 2\nURL: https://example.com/2\nsnippet 2\n"
        );
        assert_eq!(web.searches(), [("rust 1.99".to_owned(), 5)]);
        assert_eq!(tool.effect(&json!({ "query": "x" })), ToolEffect::Read);
    }

    #[tokio::test]
    async fn search_errors_and_empty_results_are_outputs() {
        let web = Arc::new(
            ScriptedWeb::new()
                .on_search_error("down", WebError::Request("could not resolve host".into())),
        );
        let tool = Typed(WebSearchTool::new(web, ResultBudget::default(), 8));
        let (_, out) = call(&tool, json!({ "query": "down" })).await;
        assert!(out.is_error);
        assert!(out.text_content().contains("could not resolve host"));
        let (_, out) = call(&tool, json!({ "query": "nothing" })).await;
        assert!(!out.is_error);
        assert!(out.text_content().starts_with("No results"));
        let empty = tool.prepare(json!({ "query": "  " }), &ctx()).await;
        assert!(empty.is_err());
    }

    #[tokio::test]
    async fn fetch_shows_source_title_and_text_cut_to_the_budget() {
        let long: String = (0..2000).map(|i| format!("line {i}\n")).collect();
        let web = Arc::new(ScriptedWeb::new().on_fetch(
            "https://example.com/a",
            WebPage {
                url: "https://example.com/b".into(),
                title: Some("A page".into()),
                text: long,
            },
        ));
        let tool = Typed(WebFetchTool::new(web, ResultBudget::new(500)));
        let (subject, out) = call(&tool, json!({ "url": "https://example.com/a" })).await;
        assert_eq!(subject, "https://example.com/a");
        let text = out.text_content();
        assert!(
            text.starts_with("Source: https://example.com/b\nTitle: A page\n\nline 0\n"),
            "{text}"
        );
        assert!(text.len() <= 500 * 4, "{}", text.len());
        assert!(
            text.contains("more lines omitted"),
            "the cut is noted: {text}"
        );
    }

    #[tokio::test]
    async fn fetch_rejects_other_schemes_and_reports_errors() {
        let web = Arc::new(ScriptedWeb::new());
        let tool = Typed(WebFetchTool::new(web, ResultBudget::default()));
        assert!(
            tool.prepare(json!({ "url": "file:///etc/passwd" }), &ctx())
                .await
                .is_err()
        );
        let (_, out) = call(&tool, json!({ "url": "https://missing.example" })).await;
        assert!(out.is_error);
    }
}
