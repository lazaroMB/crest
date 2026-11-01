//! [`ScriptedWeb`]: a [`WebPort`] that answers from a script.

use std::collections::BTreeMap;
use std::sync::Mutex;

use async_trait::async_trait;

use crate::ports::web::{SearchHit, WebError, WebPage, WebPort};

/// A [`WebPort`] that answers searches by their exact query and fetches by
/// their exact URL. An unscripted search finds nothing; an unscripted
/// fetch is an HTTP 404. Calls are recorded.
#[derive(Debug, Default)]
pub struct ScriptedWeb {
    searches: BTreeMap<String, Result<Vec<SearchHit>, WebError>>,
    pages: BTreeMap<String, Result<WebPage, WebError>>,
    search_calls: Mutex<Vec<(String, usize)>>,
    fetch_calls: Mutex<Vec<String>>,
}

impl ScriptedWeb {
    /// A web with an empty script.
    pub fn new() -> Self {
        Self::default()
    }

    /// Answers the search `query` with `hits`.
    #[must_use]
    pub fn on_search(mut self, query: &str, hits: Vec<SearchHit>) -> Self {
        self.searches.insert(query.to_owned(), Ok(hits));
        self
    }

    /// Fails the search `query` with `error`.
    #[must_use]
    pub fn on_search_error(mut self, query: &str, error: WebError) -> Self {
        self.searches.insert(query.to_owned(), Err(error));
        self
    }

    /// Answers a fetch of `url` with `page`.
    #[must_use]
    pub fn on_fetch(mut self, url: &str, page: WebPage) -> Self {
        self.pages.insert(url.to_owned(), Ok(page));
        self
    }

    /// Fails a fetch of `url` with `error`.
    #[must_use]
    pub fn on_fetch_error(mut self, url: &str, error: WebError) -> Self {
        self.pages.insert(url.to_owned(), Err(error));
        self
    }

    /// The searches made, with the number of results asked.
    pub fn searches(&self) -> Vec<(String, usize)> {
        self.search_calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// The URLs fetched.
    pub fn fetches(&self) -> Vec<String> {
        self.fetch_calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

#[async_trait]
impl WebPort for ScriptedWeb {
    async fn search(&self, query: &str, max_results: usize) -> Result<Vec<SearchHit>, WebError> {
        self.search_calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((query.to_owned(), max_results));
        match self.searches.get(query) {
            Some(Ok(hits)) => Ok(hits.iter().take(max_results).cloned().collect()),
            Some(Err(err)) => Err(err.clone()),
            None => Ok(Vec::new()),
        }
    }

    async fn fetch(&self, url: &str) -> Result<WebPage, WebError> {
        self.fetch_calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(url.to_owned());
        self.pages.get(url).cloned().unwrap_or_else(|| {
            Err(WebError::Status {
                status: 404,
                url: url.to_owned(),
            })
        })
    }

    fn backend(&self) -> &str {
        "scripted"
    }
}
