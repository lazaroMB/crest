//! The file tools over a project's roots, and over none.

use domain::conversation::{SessionId, ToolUseId, TurnId};
use serde_json::{Value, json};

use super::*;
use crate::testing::fs::{InMemoryFs, standalone_fs, test_root};
use crate::tools::registry::{Tool, Typed};

fn ctx() -> ToolContext {
    ToolContext::new(SessionId::new(), TurnId::new(), ToolUseId::new("toolu_1"))
}

/// A chat without a directory has nothing to resolve a path against: each
/// file tool says so in words the model can relay, as a normal tool error.
#[tokio::test]
async fn a_chat_without_a_directory_asks_for_one() {
    let fs: Arc<dyn FileSystemPort> = Arc::new(standalone_fs());
    let tools: Vec<(Box<dyn Tool>, Value)> = vec![
        (
            Box::new(Typed(ReadFileTool::new(
                Arc::clone(&fs),
                ResultBudget::default(),
            ))),
            json!({ "path": "Cargo.toml" }),
        ),
        (
            Box::new(Typed(EditFileTool::new(Arc::clone(&fs)))),
            json!({ "path": "a.txt", "old": "a", "new": "b" }),
        ),
        (
            Box::new(Typed(WriteFileTool::new(Arc::clone(&fs)))),
            json!({ "path": "a.txt", "content": "x" }),
        ),
    ];
    for (tool, input) in tools {
        let said = match tool.prepare(input, &ctx()).await {
            Ok(prepared) => prepared.run.await.unwrap().text_content(),
            Err(err) => err.to_string(),
        };
        assert!(said.contains(domain::workspace::NO_DIRECTORY), "{said}");
    }
}

/// An absolute spelling of a path in the first root meets the same
/// permission rules as its relative one.
#[cfg(unix)]
#[tokio::test]
async fn the_rule_subject_of_an_absolute_path_is_its_relative_address() {
    let fs = InMemoryFs::new(test_root()).with_file("src/lib.rs", "x\n");
    let tool = Typed(ReadFileTool::new(Arc::new(fs), ResultBudget::default()));
    // Each path and the subject the rules see for it.
    let cases = "/ws/src/lib.rs src/lib.rs  /ws/src/../Cargo.toml Cargo.toml  \
         src/lib.rs src/lib.rs  /elsewhere/a.rs /elsewhere/a.rs";
    let words: Vec<&str> = cases.split_whitespace().collect();
    for pair in words.chunks(2) {
        let (path, subject) = (pair[0], pair[1]);
        let prepared = tool.prepare(json!({ "path": path }), &ctx()).await.unwrap();
        assert_eq!(prepared.subject, subject, "{path}");
    }
}

/// Notes (a directory that was not searched, the legend of the roots) follow
/// the answer, one `[note]` line each; a single directory has none.
#[test]
fn notes_follow_the_answer_one_line_each() {
    let notes = |items: &[&str]| items.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    for (text, given, expected) in [
        ("a\nb", notes(&[]), "a\nb"),
        (
            "a",
            notes(&["x: not searched (gone)"]),
            "a\n[x: not searched (gone)]",
        ),
        (
            "No files found.",
            notes(&["l", "m"]),
            "No files found.\n[l]\n[m]",
        ),
    ] {
        assert_eq!(super::with_notes(text.to_owned(), &given), expected);
    }
}

/// The search tools can name a directory under any root: an absolute path
/// parses (the adapter places it), `..` out of a root still does not.
#[test]
fn search_paths_may_be_absolute_but_not_climb_out() {
    for (path, ok) in [("/a/b", true), ("src", true), ("../x", false), ("", false)] {
        let tree = json!({ "path": path });
        let grep = json!({ "pattern": "p", "path": path });
        let parsed = (
            serde_json::from_value::<TreeRequest>(tree).is_ok(),
            serde_json::from_value::<GrepRequest>(grep).is_ok(),
        );
        assert_eq!(parsed, (ok, ok), "{path:?}");
    }
}
