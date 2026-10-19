//! The main window with every panel wired to fakes: timeline, prompt input,
//! status bar, the workspace tree (`Ctrl+G e`), the pickers
//! (`Ctrl+G ff`, `Ctrl+G fg`) and the approval dialog.
//!
//! `cargo run -p ui --example panels`
//!
//! The workspace is an in-memory file system.
//!
//! - `Ctrl+G e` shows or hides the tree; click a file to "open" it.
//! - `Ctrl+G ff` finds a file, `Ctrl+G fg` greps; type, move with
//!   `Up`/`Down`, `Enter` opens, `Esc` closes.
//! - `/edit` proposes a change to `src/lib.rs` (Diff block + dialog) and
//!   `/run` asks to run a command; answer with `y`, `a`, `n` or `Esc`.
//! - `/touch <path>` adds a file to the workspace and reloads the tree.
//!
//! Quit with `/q` or Cmd/Ctrl+Q. With `--demo` the example types into its
//! own window, logs what the views show after each step and quits.

#[path = "support/mod.rs"]
mod support;

use std::sync::Arc;
use std::time::Duration;

use application::keymap::default_keymap;
use application::ports::approval::{
    ApprovalPort as _, ApprovalPreview, ApprovalRequest, ApprovalRequestId,
};
use application::ports::events::{DomainEvent, EventSink, NoticeLevel};
use application::testing::fs::{InMemoryFs, unified_diff};
use application::testing::system::RecordingSink;
use domain::block::{ApprovalState, Block, BlockKind, DiffSpec};
use domain::conversation::ToolUseId;
use domain::permission::ToolEffect;
use domain::workspace::{WorkspacePath, WorkspaceRoot};
use futures::StreamExt as _;
use gpui::{
    AnyWindowHandle, App, AppContext as _, Context, Entity, IntoElement, ParentElement as _,
    Render, Styled as _, Window, div, px,
};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};
use ui::approval::{ApprovalView, UiApproval};
use ui::input::{InputEvent, InputView};
use ui::picker::{PickerEvent, PickerView};
use ui::status::StatusView;
use ui::timeline::TimelineView;
use ui::tree::{TreePanel, TreePanelEvent};

const LIB_RS: &str = "pub fn run() {\n    println!(\"hello\");\n}\n";

fn workspace() -> Arc<InMemoryFs> {
    let root = WorkspaceRoot::new("/workspace").unwrap_or_else(|err| panic!("{err}"));
    Arc::new(
        InMemoryFs::new(root)
            .with_file("Cargo.toml", "[workspace]\nmembers = [\"crates/*\"]\n")
            .with_file("README.md", "# demo\n\nrun `cargo run`\n")
            .with_file("src/main.rs", "fn main() {\n    demo::run();\n}\n")
            .with_file("src/lib.rs", LIB_RS)
            .with_file("crates/ui/src/lib.rs", "pub mod tree;\n")
            .with_file("crates/ui/src/tree.rs", "// the side panel\n"),
    )
}

/// Main window layout: the tree on the left, the session on the right and
/// the popups over everything.
struct Panels {
    tree: Entity<TreePanel>,
    timeline: Entity<TimelineView>,
    input: Entity<InputView>,
    status: Entity<StatusView>,
    picker: Entity<PickerView>,
    approval: Entity<ApprovalView>,
}

impl Render for Panels {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let session = v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(div().flex_1().min_h_0().p_2().child(self.timeline.clone()))
            .child(
                div()
                    .min_h(px(64.))
                    .p_2()
                    .border_t_1()
                    .border_color(theme.border)
                    .child(self.input.clone()),
            )
            .child(
                div()
                    .px_2()
                    .py_1()
                    .border_t_1()
                    .border_color(theme.border)
                    .bg(theme.title_bar)
                    .child(self.status.clone()),
            );
        div()
            .relative()
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(h_flex().size_full().child(self.tree.clone()).child(session))
            .child(self.picker.clone())
            .child(self.approval.clone())
    }
}

/// Fake agent pieces: the event sink, the workspace and the approval port.
#[derive(Clone)]
struct Fakes {
    sink: Arc<RecordingSink>,
    fs: Arc<InMemoryFs>,
    approval: UiApproval,
}

impl Fakes {
    fn notice(&self, message: String) {
        self.sink.emit(DomainEvent::Notice {
            level: NoticeLevel::Info,
            message,
        });
    }

    /// `:edit`: a Diff block awaiting approval, then the dialog.
    fn propose_edit(&self, id: u64) {
        let path = "src/lib.rs";
        let old = self.fs.file(path).unwrap_or_default();
        let new = old.replace("hello", "hello, crest");
        let Ok(workspace_path) = WorkspacePath::parse(path) else {
            return;
        };
        let mut spec = DiffSpec {
            path: path.to_owned(),
            old_text: Some(old.clone()),
            new_text: new.clone(),
            unified: unified_diff(&workspace_path, Some(&old), &new),
            approval: Some(ApprovalState::Pending),
        };
        let mut block = Block::new(BlockKind::Diff(spec.clone()));
        self.sink.emit(DomainEvent::BlockAdded {
            block: block.clone(),
        });
        let request = ApprovalRequest {
            id: ApprovalRequestId(id),
            tool_use_id: ToolUseId::new(format!("toolu_{id}")),
            tool: "edit_file".into(),
            effect: ToolEffect::Write,
            subject: path.into(),
            preview: ApprovalPreview::Diff(spec.clone()),
        };
        let fakes = self.clone();
        std::thread::spawn(move || {
            let answer = futures::executor::block_on(fakes.approval.request(request));
            let approved = matches!(
                answer,
                Ok(application::ports::approval::ApprovalResponse::Approve
                    | application::ports::approval::ApprovalResponse::ApproveAlways { .. })
            );
            if approved {
                fakes.fs.set_file(path, new);
            }
            spec.approval = Some(if approved {
                ApprovalState::Approved
            } else {
                ApprovalState::Rejected
            });
            block.kind = BlockKind::Diff(spec);
            fakes.sink.emit(DomainEvent::BlockUpdated { block });
            fakes.notice(format!("edit_file: {answer:?}"));
        });
    }

    /// `:run`: asks to run a command.
    fn propose_command(&self, id: u64) {
        let request = ApprovalRequest {
            id: ApprovalRequestId(id),
            tool_use_id: ToolUseId::new(format!("toolu_{id}")),
            tool: "run_command".into(),
            effect: ToolEffect::Execute,
            subject: "cargo test -p ui".into(),
            preview: ApprovalPreview::Command {
                command: "cargo".into(),
                args: vec!["test".into(), "-p".into(), "ui".into()],
                cwd: ".".into(),
            },
        };
        let fakes = self.clone();
        std::thread::spawn(move || {
            let answer = futures::executor::block_on(fakes.approval.request(request));
            fakes.notice(format!("run_command: {answer:?}"));
        });
    }
}

fn main() {
    let demo = std::env::args().any(|arg| arg == "--demo");
    let (sink, mut rx) = RecordingSink::with_channel();
    let (approval, inbox) = UiApproval::channel();
    let fakes = Fakes {
        sink: Arc::new(sink),
        fs: workspace(),
        approval,
    };

    support::run("crest — panels", move |window, cx| {
        let fs = fakes.fs.clone();
        let timeline = cx.new(TimelineView::new);
        let status = cx.new(|_| StatusView::new("panels"));
        let keymap = default_keymap();
        let input = cx.new(|cx| InputView::with_keymap(keymap, window, cx));
        let tree = cx.new(|cx| TreePanel::new(fs.clone(), cx));
        let picker = cx.new(|cx| PickerView::new(fs.clone(), fs.clone(), window, cx));
        let approval = cx.new(|cx| ApprovalView::new(inbox, window, cx));

        let (to_timeline, to_status) = (timeline.clone(), status.clone());
        cx.spawn(async move |cx| {
            while let Some(event) = rx.next().await {
                to_timeline.update(cx, |view, cx| view.apply(&event, cx));
                to_status.update(cx, |view, cx| view.apply(&event, cx));
            }
        })
        .detach();

        wire(&input, &timeline, &tree, &picker, fakes, window, cx);

        let panels = cx.new(|_| Panels {
            tree: tree.clone(),
            timeline,
            input: input.clone(),
            status,
            picker: picker.clone(),
            approval: approval.clone(),
        });
        input.update(cx, |input, cx| input.focus(window, cx));
        if demo {
            run_demo(window.window_handle(), tree, picker, approval, cx);
        }
        panels.into()
    });
}

#[allow(clippy::too_many_arguments)]
fn wire(
    input: &Entity<InputView>,
    timeline: &Entity<TimelineView>,
    tree: &Entity<TreePanel>,
    picker: &Entity<PickerView>,
    fakes: Fakes,
    window: &mut Window,
    cx: &mut App,
) {
    let opened = {
        let fakes = fakes.clone();
        move |path: &WorkspacePath, line: Option<u64>| match line {
            Some(line) => fakes.notice(format!("open {path}:{line}")),
            None => fakes.notice(format!("open {path}")),
        }
    };
    let on_tree = opened.clone();
    cx.subscribe(tree, move |_, event: &TreePanelEvent, _| match event {
        TreePanelEvent::Open(path) => on_tree(path, None),
    })
    .detach();
    let (input_back, on_picker) = (input.clone(), opened);
    let window_handle = window.window_handle();
    cx.subscribe(picker, move |_, event: &PickerEvent, cx| {
        if let PickerEvent::Open { path, line } = event {
            on_picker(path, *line);
        }
        // The picker gave focus back to the input; keep its mode in sync.
        let input = input_back.clone();
        window_handle
            .update(cx, |_, window, cx| {
                input.update(cx, |input, cx| input.focus(window, cx));
            })
            .ok();
    })
    .detach();

    let (timeline, tree, picker) = (timeline.clone(), tree.clone(), picker.clone());
    let mut next_id = 0;
    cx.subscribe(input, move |_, event: &InputEvent, cx| {
        match event {
            InputEvent::Submit(prompt) | InputEvent::SubmitImages { text: prompt, .. } => {
                fakes.notice(format!("no agent here: {prompt}"));
            }
            InputEvent::Notice(message) => fakes.notice(message.clone()),
            InputEvent::Command(command) => {
                next_id += 1;
                let (name, arg) = command.split_once(' ').unwrap_or((command, ""));
                match name {
                    "q" | "quit" => cx.quit(),
                    "edit" => fakes.propose_edit(next_id),
                    "run" => fakes.propose_command(next_id),
                    "touch" if !arg.is_empty() => {
                        fakes.fs.set_file(arg, "");
                        // The in-memory workspace has no directory to watch.
                        tree.update(cx, |tree, cx| tree.reload(cx));
                    }
                    _ => fakes.notice(format!("unknown command /{command}")),
                }
            }
            InputEvent::Action(action) => {
                if timeline.update(cx, |view, cx| view.handle_action(action, cx))
                    || window_handle
                        .update(cx, |_, window, cx| {
                            tree.update(cx, |tree, cx| tree.handle_action(action, window, cx))
                        })
                        .unwrap_or(false)
                    || window_handle
                        .update(cx, |_, window, cx| {
                            picker.update(cx, |picker, cx| picker.handle_action(action, window, cx))
                        })
                        .unwrap_or(false)
                {
                    return;
                }
                fakes.notice(format!("{action:?} is not wired in this example"));
            }
            // The example keeps no sessions.
            InputEvent::Recorded(_) => {}
        }
    })
    .detach();
}

/// Keys typed by `--demo`, each followed by a pause and a log line.
const DEMO_STEPS: &[(&str, u64)] = &[
    ("ctrl-g e", 300),
    ("ctrl-g f f", 200),
    ("m a i n", 500),
    ("down enter", 300),
    ("ctrl-g f g", 200),
    ("r u n", 500),
    ("enter", 300),
    ("/ e d i t enter", 400),
    ("y", 400),
    ("/ r u n enter", 400),
    ("n n o space t h a n k s enter", 400),
];

fn run_demo(
    window: AnyWindowHandle,
    tree: Entity<TreePanel>,
    picker: Entity<PickerView>,
    approval: Entity<ApprovalView>,
    cx: &mut App,
) {
    cx.spawn(async move |cx| {
        cx.background_executor()
            .timer(Duration::from_millis(500))
            .await;
        for (keys, pause) in DEMO_STEPS {
            for key in keys.split(' ') {
                let Ok(keystroke) = gpui::Keystroke::parse(key) else {
                    continue;
                };
                window
                    .update(cx, |_, window, cx| window.dispatch_keystroke(keystroke, cx))
                    .ok();
            }
            cx.background_executor()
                .timer(Duration::from_millis(*pause))
                .await;
            cx.update(|cx| {
                let picker = picker.read(cx);
                eprintln!(
                    "demo [{keys}]: tree={} picker={:?} query={:?} results={} dialog={:?}",
                    tree.read(cx).is_visible(),
                    picker.mode(),
                    picker.query(cx),
                    picker.items().len(),
                    approval.read(cx).current().map(|r| r.tool.clone()),
                );
            });
        }
        eprintln!("demo: done");
        cx.update(|cx| cx.quit());
    })
    .detach();
}
