//! A turn that researches with `web_search` and `web_fetch`.

use std::sync::Arc;

use domain::block::{BlockKind, SourceRef};
use domain::permission::PermissionMode;
use serde_json::json;

use super::tests::Harness;
use super::*;
use crate::ports::web::{SearchHit, WebPage};
use crate::testing::llm::ScriptedResponse;
use crate::testing::web::ScriptedWeb;
use crate::tools::web::register_web_tools;

fn web() -> Arc<ScriptedWeb> {
    Arc::new(
        ScriptedWeb::new()
            .on_search(
                "rust 1.99",
                vec![SearchHit {
                    title: "Announcing Rust 1.99".into(),
                    url: "https://blog.rust-lang.org/1.99".into(),
                    snippet: "The Rust team is happy".into(),
                }],
            )
            .on_fetch(
                "https://blog.rust-lang.org/1.99",
                WebPage {
                    url: "https://blog.rust-lang.org/1.99".into(),
                    title: Some("Announcing Rust 1.99".into()),
                    text: "Rust 1.99 stabilises things.".into(),
                },
            ),
    )
}

fn sources(h: &Harness) -> Vec<Vec<SourceRef>> {
    h.sink
        .events()
        .into_iter()
        .filter_map(|event| match event {
            DomainEvent::BlockAdded { block } => match block.kind {
                BlockKind::Sources { sources } => Some(sources),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn searches_and_reads_without_approval_and_shows_what_it_read() {
    for mode in [PermissionMode::Ask, PermissionMode::Plan] {
        let web = web();
        let tools_web = Arc::clone(&web);
        let mut h = Harness::with(
            vec![
                ScriptedResponse::tool_use("t1", "web_search", json!({ "query": "rust 1.99" })),
                ScriptedResponse::tool_use(
                    "t2",
                    "web_fetch",
                    json!({ "url": "https://blog.rust-lang.org/1.99" }),
                ),
                ScriptedResponse::text("Rust 1.99 stabilises things [blog]."),
            ],
            mode,
            move |tools| register_web_tools(tools, tools_web, 5).unwrap(),
        );
        h.run("investiga rust 1.99").await.unwrap();

        assert!(h.approval.requests().is_empty(), "{mode:?} asked");
        let results = h.tool_results();
        assert_eq!(results.len(), 2, "{results:?}");
        assert!(results.iter().all(|(_, is_error)| !is_error), "{results:?}");
        assert_eq!(web.searches(), [("rust 1.99".to_owned(), 5)]);
        assert_eq!(web.fetches(), ["https://blog.rust-lang.org/1.99"]);
        assert_eq!(
            sources(&h),
            [vec![SourceRef::Url {
                url: "https://blog.rust-lang.org/1.99".into(),
                title: Some("Announcing Rust 1.99".into()),
            }]],
            "{mode:?}"
        );
    }
}

#[tokio::test]
async fn a_turn_that_only_searched_shows_no_sources() {
    let web = web();
    let mut h = Harness::with(
        vec![
            ScriptedResponse::tool_use("t1", "web_search", json!({ "query": "rust 1.99" })),
            ScriptedResponse::text("Parece que salió."),
        ],
        PermissionMode::Ask,
        move |tools| register_web_tools(tools, web, 5).unwrap(),
    );
    h.run("busca").await.unwrap();
    assert!(sources(&h).is_empty());
}
