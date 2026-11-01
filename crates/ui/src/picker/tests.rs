use std::sync::atomic::{AtomicUsize, Ordering};

use application_core::ports::search::{
    ColumnRange, FileList, FindFileRequest, GrepRequest, ListFilesRequest, TreeNode, TreeRequest,
};
use application_testing::fs::{InMemoryFs, test_root};

use futures::executor::block_on;
use gpui::{ParentElement as _, Styled as _, TestAppContext, VisualTestContext, div, px};

use super::preview::RichPreview;
use super::*;

fn fake_fs() -> Arc<InMemoryFs> {
    Arc::new(
        InMemoryFs::new(test_root())
            .with_file("Cargo.toml", "[workspace]\nmembers = [\"crates/*\"]\n")
            .with_file("src/main.rs", "fn main() {\n    run();\n}\n")
            .with_file("src/lib.rs", "pub fn run() {}\n")
            .with_file("docs/main.md", "# main\nrun it\n")
            .with_file("crates/ui/src/main_view.rs", "// run\n"),
    )
}

fn picker(
    fs: Arc<InMemoryFs>,
    cx: &mut TestAppContext,
) -> (Entity<PickerView>, &mut VisualTestContext) {
    cx.update(gpui_component::init);
    let (picker, cx) =
        cx.add_window_view(|window, cx| PickerView::new(fs.clone(), fs.clone(), window, cx));
    (picker, cx)
}

fn open(picker: &Entity<PickerView>, action: &str, cx: &mut VisualTestContext) {
    cx.update(|window, cx| {
        picker.update(cx, |picker, cx| {
            assert!(picker.handle_action(&Action::builtin(action), window, cx));
        });
    });
}

fn settle(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.executor().advance_clock(SEARCH_DEBOUNCE);
    cx.run_until_parked();
}

fn events(
    picker: &Entity<PickerView>,
    cx: &mut VisualTestContext,
) -> std::rc::Rc<std::cell::RefCell<Vec<PickerEvent>>> {
    let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let seen = events.clone();
    cx.update(|_, cx| {
        cx.subscribe(picker, move |_, event: &PickerEvent, _| {
            seen.borrow_mut().push(event.clone());
        })
        .detach();
    });
    events
}

#[test]
fn actions_pick_the_mode() {
    assert_eq!(
        PickerMode::for_action(&Action::builtin("picker.files")),
        Some(PickerMode::Files)
    );
    assert_eq!(
        PickerMode::for_action(&Action::builtin("picker.grep")),
        Some(PickerMode::Grep)
    );
    assert_eq!(PickerMode::for_action(&Action::builtin("block.next")), None);
}

#[test]
fn requests_are_the_tool_defaults() {
    // What the model gets when it calls the tool with only the query.
    let find: FindFileRequest = serde_json::from_str(r#"{"query":"main"}"#).unwrap();
    assert_eq!(files_request("main"), find);
    let grep: GrepRequest =
        serde_json::from_str(r#"{"pattern":"run","fixed":true,"ignore_case":true}"#).unwrap();
    assert_eq!(grep_request("run", GrepOptions::default()), grep);
    let all = GrepOptions {
        match_case: true,
        regex: true,
        whole_word: true,
    };
    let grep: GrepRequest = serde_json::from_str(r#"{"pattern":"run","word":true}"#).unwrap();
    assert_eq!(grep_request("run", all), grep);
}

#[gpui::test]
fn file_results_match_the_port(cx: &mut TestAppContext) {
    let fs = fake_fs();
    let (picker, cx) = picker(fs.clone(), cx);
    open(&picker, FILES_ACTION, cx);
    cx.simulate_input("main");
    settle(cx);
    let expected = block_on(fs.find_file(files_request("main"))).unwrap();
    assert_eq!(expected.paths.len(), 3);
    picker.read_with(cx, |picker, cx| {
        assert_eq!(picker.query(cx), "main");
        let paths: Vec<_> = picker
            .items()
            .iter()
            .map(|item| item.path.clone())
            .collect();
        assert_eq!(paths, expected.paths);
        assert!(!picker.is_searching());
    });
}

#[gpui::test]
fn enter_opens_the_highlighted_file(cx: &mut TestAppContext) {
    let fs = fake_fs();
    let (picker, cx) = picker(fs.clone(), cx);
    let events = events(&picker, cx);
    open(&picker, FILES_ACTION, cx);
    cx.simulate_input("main");
    settle(cx);
    let expected = block_on(fs.find_file(files_request("main"))).unwrap();
    cx.simulate_keystrokes("down down up down enter");
    assert_eq!(
        *events.borrow(),
        [PickerEvent::Open {
            path: expected.paths[2].clone(),
            line: None,
        }]
    );
    picker.read_with(cx, |picker, _| assert_eq!(picker.mode(), None));
}

#[gpui::test]
fn escape_dismisses_and_gives_focus_back(cx: &mut TestAppContext) {
    let (picker, cx) = picker(fake_fs(), cx);
    let events = events(&picker, cx);
    let before = cx.update(|window, cx| {
        let handle = cx.focus_handle();
        window.focus(&handle, cx);
        handle
    });
    open(&picker, GREP_ACTION, cx);
    cx.simulate_input("run");
    cx.simulate_keystrokes("escape");
    assert_eq!(*events.borrow(), [PickerEvent::Dismissed]);
    cx.update(|window, _| assert!(before.is_focused(window)));
    picker.read_with(cx, |picker, _| {
        assert_eq!(picker.mode(), None);
        assert!(!picker.is_searching(), "the pending search was dropped");
    });
}

/// A port whose `find_file` never finishes for `slow` and counts the
/// searches that were dropped before finishing.
struct Slow {
    inner: Arc<InMemoryFs>,
    dropped: Arc<AtomicUsize>,
}

struct CountDrop(Arc<AtomicUsize>);

impl Drop for CountDrop {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[async_trait::async_trait]
impl FileSearchPort for Slow {
    async fn list_files(&self, request: ListFilesRequest) -> Result<FileList, SearchError> {
        self.inner.list_files(request).await
    }
    async fn find_file(&self, request: FindFileRequest) -> Result<FileList, SearchError> {
        if request.query == "slow" {
            let _guard = CountDrop(self.dropped.clone());
            futures::future::pending::<()>().await;
        }
        self.inner.find_file(request).await
    }
    async fn tree(&self, request: TreeRequest) -> Result<TreeNode, SearchError> {
        self.inner.tree(request).await
    }
}

#[gpui::test]
fn typing_cancels_the_search_in_flight(cx: &mut TestAppContext) {
    let fs = fake_fs();
    let dropped = Arc::new(AtomicUsize::new(0));
    let slow = Arc::new(Slow {
        inner: fs.clone(),
        dropped: dropped.clone(),
    });
    cx.update(gpui_component::init);
    let (picker, cx) =
        cx.add_window_view(|window, cx| PickerView::new(slow.clone(), fs.clone(), window, cx));
    open(&picker, FILES_ACTION, cx);
    cx.simulate_input("slow");
    settle(cx);
    picker.read_with(cx, |picker, _| assert!(picker.is_searching()));
    assert_eq!(dropped.load(Ordering::SeqCst), 0, "still running");

    cx.update(|window, cx| {
        picker.update(cx, |picker, cx| picker.set_query("lib", window, cx));
    });
    settle(cx);
    assert_eq!(
        dropped.load(Ordering::SeqCst),
        1,
        "the slow search was dropped"
    );
    picker.read_with(cx, |picker, _| {
        let paths: Vec<_> = picker.items().iter().map(|i| i.path.to_string()).collect();
        assert_eq!(paths, ["src/lib.rs"]);
    });
}

#[gpui::test]
fn search_waits_for_the_debounce(cx: &mut TestAppContext) {
    let (picker, cx) = picker(fake_fs(), cx);
    open(&picker, GREP_ACTION, cx);
    cx.simulate_input("pub");
    cx.run_until_parked();
    picker.read_with(cx, |picker, _| {
        assert!(picker.items().is_empty());
        assert!(picker.is_searching());
    });
    settle(cx);
    picker.read_with(cx, |picker, _| assert_eq!(picker.items().len(), 1));
    // The old results stay, marked stale, while the next search waits.
    cx.simulate_input("x");
    cx.run_until_parked();
    picker.read_with(cx, |picker, cx| {
        assert_eq!(picker.items().len(), 1);
        assert!(picker.is_searching());
        assert_eq!(
            picker.status_text(cx),
            "searching… (showing the previous results)"
        );
    });
    // Clearing the query clears the results without searching.
    cx.simulate_keystrokes("backspace backspace backspace backspace");
    cx.run_until_parked();
    picker.read_with(cx, |picker, cx| {
        assert!(picker.items().is_empty());
        assert!(!picker.is_searching());
        assert_eq!(
            picker.status_text(cx),
            "Type to search the text of the workspace"
        );
    });
}

#[gpui::test]
fn draws_results(cx: &mut TestAppContext) {
    let (picker, cx) = picker(fake_fs(), cx);
    open(&picker, FILES_ACTION, cx);
    cx.simulate_input("main");
    settle(cx);
    cx.draw(
        gpui::point(px(0.), px(0.)),
        gpui::size(px(1000.), px(700.)),
        |_, _| div().relative().size_full().child(picker.clone()),
    );
}

#[gpui::test]
fn grep_results_match_the_port(cx: &mut TestAppContext) {
    let fs = fake_fs();
    let (picker, cx) = picker(fs.clone(), cx);
    open(&picker, GREP_ACTION, cx);
    cx.simulate_input("run");
    settle(cx);
    let expected = block_on(fs.grep(grep_request("run", GrepOptions::default()))).unwrap();
    assert_eq!(expected.hits.len(), 4);
    picker.read_with(cx, |picker, _| {
        let rows: Vec<_> = picker
            .items()
            .iter()
            .map(|item| (item.path.clone(), item.line, item.text.clone()))
            .collect();
        let hits: Vec<_> = expected
            .hits
            .iter()
            .map(|hit| {
                (
                    hit.path.clone(),
                    Some(hit.line_number),
                    Some(hit.line.clone()),
                )
            })
            .collect();
        assert_eq!(rows, hits);
    });
}

#[gpui::test]
fn enter_on_a_match_opens_its_line(cx: &mut TestAppContext) {
    let fs = fake_fs();
    let (picker, cx) = picker(fs.clone(), cx);
    let events = events(&picker, cx);
    open(&picker, GREP_ACTION, cx);
    cx.simulate_input("run()");
    settle(cx);
    // Literal by default: `run()` is the text, not a regex.
    picker.read_with(cx, |picker, _| {
        let mut rows: Vec<_> = picker.items().iter().map(|i| i.path.to_string()).collect();
        rows.sort_unstable();
        assert_eq!(rows, ["src/lib.rs", "src/main.rs"]);
    });
    // As a regex `(` is a metacharacter, so the request fails like
    // the tool's.
    cx.simulate_keystrokes("alt-r");
    settle(cx);
    picker.read_with(cx, |picker, _| {
        assert!(picker.options().regex);
        assert!(picker.items().is_empty());
        assert!(
            picker
                .error()
                .is_some_and(|e| e.contains("invalid pattern"))
        );
    });
    cx.simulate_keystrokes("alt-r backspace backspace");
    settle(cx);
    cx.simulate_keystrokes("ctrl-n enter");
    let expected = block_on(fs.grep(grep_request("run", GrepOptions::default()))).unwrap();
    assert_eq!(
        *events.borrow(),
        [PickerEvent::Open {
            path: expected.hits[1].path.clone(),
            line: Some(expected.hits[1].line_number),
        }]
    );
}

#[test]
fn counts_paths_and_rows_read_well() {
    assert_eq!(count_text(1, 1, false), "1 result");
    assert_eq!(count_text(4, 4, false), "4 results");
    assert_eq!(count_text(100, 2345, true), "100 of 2345 (truncated)");
    assert_eq!(truncate_left("src/main.rs", 20), "src/main.rs");
    assert_eq!(
        truncate_left("crates/ui/src/picker/preview.rs", 22),
        "…/picker/preview.rs"
    );
    assert_eq!(truncate_left("abcdefghij", 5), "…ghij");
    // The indentation goes and the match keeps its place.
    let range = |start, end| Some(ColumnRange { start, end });
    assert_eq!(
        row_text("    run();", range(4, 7)),
        ("run();".to_owned(), Some(0..3))
    );
    // A match far to the right stays in view.
    let long = format!("{}needle end", "x".repeat(80));
    let (shown, at) = row_text(&long, range(80, 86));
    assert!(shown.starts_with('…'), "{shown}");
    let at = at.unwrap();
    assert_eq!(&shown[at], "needle");
    assert_eq!(
        footer_keys(PickerMode::Files, true),
        "↑↓ navigate · Enter insert · Esc close · Alt+P preview"
    );
    assert!(
        footer_keys(PickerMode::Grep, false).ends_with("Alt+C case · Alt+R regex · Alt+W word")
    );
}

#[gpui::test]
fn an_empty_query_lists_recent_files_first(cx: &mut TestAppContext) {
    let fs = fake_fs();
    let (picker, cx) = picker(fs.clone(), cx);
    open(&picker, FILES_ACTION, cx);
    settle(cx);
    let listed = block_on(fs.list_files(ListFilesRequest::default())).unwrap();
    picker.read_with(cx, |picker, cx| {
        let paths: Vec<_> = picker.items().iter().map(|i| i.path.clone()).collect();
        assert_eq!(paths, listed.paths);
        assert_eq!(picker.status_text(cx), "workspace files; type to search");
    });
    cx.simulate_input("lib");
    settle(cx);
    cx.simulate_keystrokes("enter");
    open(&picker, FILES_ACTION, cx);
    settle(cx);
    picker.read_with(cx, |picker, cx| {
        let first = &picker.items()[0];
        assert_eq!((first.path.as_str(), first.recent), ("src/lib.rs", true));
        assert_eq!(picker.items().len(), listed.paths.len(), "listed once");
        assert!(picker.status_text(cx).starts_with("recent files"));
    });
}

#[gpui::test]
fn the_preview_reads_around_the_match(cx: &mut TestAppContext) {
    let fs = fake_fs();
    let (picker, cx) = picker(fs.clone(), cx);
    cx.update(|_, cx| {
        picker.update(cx, |picker, _| picker.set_file_system(Some(fs.clone())));
    });
    open(&picker, GREP_ACTION, cx);
    cx.simulate_input("run();");
    settle(cx);
    // At once from the context grep returned, then from the file.
    picker.read_with(cx, |picker, _| {
        let preview = picker.preview().expect("previewed");
        assert!(preview.loading);
        assert_eq!(preview.match_index(), Some(1));
    });
    cx.executor().advance_clock(preview::PREVIEW_DEBOUNCE);
    cx.run_until_parked();
    picker.read_with(cx, |picker, _| {
        let preview = picker.preview().expect("previewed");
        assert!(!preview.loading);
        assert_eq!(preview.lines, ["fn main() {", "    run();", "}"]);
        assert_eq!(preview.start_line, 1);
        assert_eq!(preview.match_index(), Some(1));
        assert_eq!(
            preview.column_range,
            Some(ColumnRange { start: 4, end: 10 })
        );
    });
    cx.draw(
        gpui::point(px(0.), px(0.)),
        gpui::size(px(1200.), px(800.)),
        |_, _| div().relative().size_full().child(picker.clone()),
    );
    // `Alt+P` hides it; moving no longer reads files.
    cx.simulate_keystrokes("alt-p");
    picker.read_with(cx, |picker, _| assert!(!picker.preview_enabled()));
}

#[gpui::test]
fn pictures_models_and_binaries_are_previewed_as_such(cx: &mut TestAppContext) {
    const STL: &str = "solid t\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\n\
                       vertex 0 1 0\nendloop\nendfacet\nendsolid t\n";
    let fs = Arc::new(
        InMemoryFs::new(test_root())
            .with_bytes("shots/logo.png", vec![0x89, b'P', b'N', b'G', 0, 1])
            .with_file("parts/tetra.stl", STL)
            .with_bytes("build/app.bin", vec![0, 1, 2, 3])
            .with_file("parts/bracket.step", "ISO-10303-21;\n"),
    );
    let (picker, cx) = picker(fs.clone(), cx);
    cx.update(|_, cx| {
        picker.update(cx, |picker, _| picker.set_file_system(Some(fs.clone())));
    });
    let preview_of = |query: &str, cx: &mut VisualTestContext| {
        open(&picker, FILES_ACTION, cx);
        cx.simulate_input(query);
        settle(cx);
        cx.executor().advance_clock(preview::PREVIEW_DEBOUNCE);
        cx.run_until_parked();
        let preview = picker.read_with(cx, |picker, _| {
            assert_eq!(picker.items()[0].path.as_str(), query, "first result");
            picker.preview().cloned().expect("previewed")
        });
        cx.update(|window, cx| picker.update(cx, |picker, cx| picker.dismiss(window, cx)));
        preview
    };

    let picture = preview_of("shots/logo.png", cx);
    assert!(!picture.loading && picture.lines.is_empty());
    match picture.rich {
        Some(RichPreview::Image { path }) => {
            assert!(
                path.ends_with("logo.png") && path.len() > "shots/logo.png".len(),
                "{path}"
            )
        }
        other => panic!("a picture: {other:?}"),
    }

    let model = preview_of("parts/tetra.stl", cx);
    match model.rich {
        Some(RichPreview::Model(spec)) => {
            assert_eq!(
                (spec.name.as_str(), spec.mesh.triangles.len()),
                ("tetra.stl", 1)
            );
        }
        other => panic!("a model: {other:?}"),
    }

    let binary = preview_of("build/app.bin", cx);
    assert_eq!(
        binary.rich,
        Some(RichPreview::Unsupported {
            reason: "binary file".into()
        })
    );
    assert_eq!(binary.error, None, "not an error");

    let step = preview_of("parts/bracket.step", cx);
    assert!(matches!(
        step.rich,
        Some(RichPreview::Unsupported { ref reason }) if reason.contains("/open")
    ));

    // The stills draw under the shield; the card names itself.
    open(&picker, FILES_ACTION, cx);
    cx.simulate_input("build/app.bin");
    settle(cx);
    cx.executor().advance_clock(preview::PREVIEW_DEBOUNCE);
    cx.run_until_parked();
    cx.draw(
        gpui::point(px(0.), px(0.)),
        gpui::size(px(1200.), px(800.)),
        |_, _| div().relative().size_full().child(picker.clone()),
    );
    assert!(cx.debug_bounds("picker-preview-unsupported").is_some());
}

#[gpui::test]
fn the_chips_flip_the_options(cx: &mut TestAppContext) {
    let (picker, cx) = picker(fake_fs(), cx);
    open(&picker, GREP_ACTION, cx);
    cx.simulate_input("RUN");
    settle(cx);
    picker.read_with(cx, |picker, _| assert_eq!(picker.items().len(), 4));
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    let case = cx.debug_bounds("picker-case").expect("chip drawn");
    cx.simulate_click(case.center(), gpui::Modifiers::none());
    settle(cx);
    picker.read_with(cx, |picker, _| {
        assert!(picker.options().match_case);
        assert!(picker.items().is_empty(), "no upper-case RUN");
    });
    cx.update(|window, cx| {
        let input = picker.read(cx).input.focus_handle(cx);
        assert!(input.is_focused(window), "the input keeps the focus");
    });
}

#[gpui::test]
fn the_highlighted_row_stays_in_view(cx: &mut TestAppContext) {
    let mut fs = InMemoryFs::new(test_root());
    for n in 0..40 {
        fs = fs.with_file(&format!("src/file_{n:02}.rs"), "\n");
    }
    let (picker, cx) = picker(Arc::new(fs), cx);
    open(&picker, FILES_ACTION, cx);
    settle(cx);
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    for _ in 0..30 {
        cx.simulate_keystrokes("down");
    }
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    picker.read_with(cx, |picker, _| {
        assert_eq!(picker.selected(), 30);
        let scroll = picker.scroll_handle().0.borrow();
        let offset = -scroll.base_handle.offset().y;
        assert!(offset > px(0.), "the list scrolled");
        // The last layout's content is the 40 rows.
        let row = scroll
            .last_item_size
            .map(|size| size.contents.height / 40.)
            .unwrap_or_else(|| panic!("the rows were laid out"));
        let view = scroll.base_handle.bounds().size.height;
        let top = row * 30.;
        assert!(
            top >= offset - px(1.) && top + row <= offset + view + px(1.),
            "row 30 in view: top {top:?}, offset {offset:?}, view {view:?}"
        );
    });
}

// -- parity with the agent's tools --------------------------------------

/// Output of the registered file tool `name` for `input`, as the model
/// would get it.
fn tool_output(fs: &Arc<InMemoryFs>, name: &str, input: serde_json::Value) -> String {
    use application::tools::files::{FileToolPorts, register_file_tools};
    use application_core::tools::registry::{ToolContext, ToolRegistry};
    use application_testing::fs::ScriptedShell;
    use domain::conversation::{SessionId, ToolUseId, TurnId};

    let mut registry = ToolRegistry::new();
    register_file_tools(
        &mut registry,
        FileToolPorts {
            file_search: fs.clone(),
            content_search: fs.clone(),
            fs: fs.clone(),
            shell: Arc::new(ScriptedShell::new()),
            rewriter: None,
        },
    )
    .unwrap();
    let tool = registry.get(name).unwrap();
    let ctx = ToolContext::new(SessionId::new(), TurnId::new(), ToolUseId::new("toolu_1"));
    block_on(async {
        let prepared = tool.prepare(input, &ctx).await.unwrap();
        prepared.run.await.unwrap()
    })
    .text_content()
}

#[gpui::test]
fn file_picker_lists_what_find_file_returns(cx: &mut TestAppContext) {
    let fs = fake_fs();
    let (picker, cx) = picker(fs.clone(), cx);
    open(&picker, FILES_ACTION, cx);
    cx.simulate_input("main");
    settle(cx);
    let output = tool_output(&fs, "find_file", serde_json::json!({"query": "main"}));
    let tool_paths: Vec<&str> = output.lines().collect();
    picker.read_with(cx, |picker, _| {
        let paths: Vec<String> = picker.items().iter().map(|i| i.path.to_string()).collect();
        assert_eq!(paths, tool_paths);
    });
}

#[gpui::test]
fn text_picker_lists_what_grep_returns(cx: &mut TestAppContext) {
    let fs = fake_fs();
    let (picker, cx) = picker(fs.clone(), cx);
    open(&picker, GREP_ACTION, cx);
    cx.simulate_input("run");
    settle(cx);
    let output = tool_output(
        &fs,
        "grep",
        serde_json::json!({"pattern": "run", "fixed": true, "ignore_case": true}),
    );
    // Match lines are `path:line: text`; context lines use `-` and
    // groups are separated by `--`.
    let tool_matches: Vec<&str> = output
        .lines()
        .filter(|line| {
            line.split_once(':').is_some_and(|(_, rest)| {
                rest.split_once(": ")
                    .is_some_and(|(n, _)| n.chars().all(|c| c.is_ascii_digit()))
            })
        })
        .collect();
    assert_eq!(tool_matches.len(), 4);
    picker.read_with(cx, |picker, _| {
        let rows: Vec<String> = picker
            .items()
            .iter()
            .map(|i| {
                let (line, text) = (
                    i.line.unwrap_or_default(),
                    i.text.clone().unwrap_or_default(),
                );
                format!("{}:{line}: {text}", i.path)
            })
            .collect();
        assert_eq!(rows, tool_matches);
    });
}
