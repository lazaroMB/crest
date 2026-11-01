use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use application::ports::events::EventSink;
use application::ports::form::FormPort;
use application::tools::files::{FileToolPorts, register_file_tools};
use application::tools::registry::ToolRegistry;
use application::usecases::approval::SharedPermissions;
use application::usecases::permissions::ProjectRules;
use application::usecases::run_turn::{AgentDeps, AgentService, TurnOptions, UserInput};
use application_testing::fs::{InMemoryFs, ScriptedShell, test_root};
use application_testing::llm::{ScriptedLlm, ScriptedResponse};
use application_testing::permissions::MemoryPermissionStore;
use application_testing::system::{RecordingSink, RecordingTelemetry};
use domain::block::{BlockKind, ToolStatus};
use domain::conversation::ContentPart;
use domain::conversation::Session as Conversation;
use domain::model::{ModelPolicy, ModelRole};
use domain::permission::PermissionPolicy;

use futures::channel::oneshot;
use gpui::{Focusable as _, ParentElement as _, Styled as _, TestAppContext, VisualTestContext};
use serde_json::json;

use super::*;
use crate::approval::UiApproval;
use crate::commands::unknown_command_hint;
use crate::help::{HelpView, Section, help_modal_id};
use crate::input::ModelPopup;
use crate::input::attachments::ImageMode;
use crate::permissions::PermissionsView;

/// A controller over the real `AgentService`, running each turn on its
/// own thread as the binary runs it on tokio.
struct AgentController {
    service: Arc<AgentService>,
    conversation: Arc<futures::lock::Mutex<Conversation>>,
    events: Arc<dyn EventSink>,
    /// Keys given to `login`, in order.
    logins: Arc<Mutex<Vec<String>>>,
}

impl SessionController for AgentController {
    fn run_turn(&self, request: TurnRequest) -> BoxFuture<'static, TurnEnd> {
        let (tx, rx) = oneshot::channel();
        let service = Arc::clone(&self.service);
        let conversation = Arc::clone(&self.conversation);
        let events = Arc::clone(&self.events);
        std::thread::spawn(move || {
            let end = futures::executor::block_on(async move {
                events.emit(DomainEvent::BlockAdded {
                    block: prompt_block(&request.prompt),
                });
                let role = request.role.unwrap_or(ModelRole::Main);
                let mut opts = TurnOptions::new(role).with_cancel(request.cancel);
                if let Some(model) = request.model_override {
                    opts = opts.with_override(model);
                }
                let mut conversation = conversation.lock().await;
                let input = UserInput::new(request.prompt, request.images);
                match service.run_turn(&mut conversation, input, opts).await {
                    Ok(outcome) => TurnEnd::Finished(outcome.stop_reason),
                    Err(err) => TurnEnd::Failed(err.to_string()),
                }
            });
            tx.send(end).ok();
        });
        rx.map(|end| end.unwrap_or(TurnEnd::Failed("turn lost".into())))
            .boxed()
    }

    fn permission_mode(&self) -> PermissionMode {
        self.service
            .permissions()
            .lock()
            .map(|policy| policy.mode)
            .unwrap_or_default()
    }

    fn set_permission_mode(&self, mode: PermissionMode) {
        if let Ok(mut policy) = self.service.permissions().lock() {
            policy.mode = mode;
        }
    }

    fn models(&self) -> Vec<ModelChoice> {
        ModelPolicy::default()
            .aliases()
            .into_iter()
            .map(|(alias, model)| ModelChoice { alias, model })
            .collect()
    }

    fn permission_rules(&self) -> BoxFuture<'static, RuleListing> {
        let service = Arc::clone(&self.service);
        async move {
            application::usecases::permissions::load_rules(
                service.permissions(),
                service.project_rules(),
            )
            .await
        }
        .boxed()
    }

    fn remove_permission_rule(
        &self,
        rule: PermissionRule,
    ) -> BoxFuture<'static, Result<(), String>> {
        let service = Arc::clone(&self.service);
        async move {
            application::usecases::permissions::remove_rule(
                service.permissions(),
                service.project_rules(),
                &rule,
            )
            .await
            .map_err(|err| err.to_string())
        }
        .boxed()
    }

    fn promote_permission_rule(
        &self,
        rule: PermissionRule,
    ) -> BoxFuture<'static, Result<(), String>> {
        let service = Arc::clone(&self.service);
        async move {
            application::usecases::permissions::promote_rule(
                service.permissions(),
                service.project_rules(),
                &rule,
            )
            .await
            .map_err(|err| err.to_string())
        }
        .boxed()
    }

    /// Two OpenCode models, besides the aliases.
    fn catalog_models(&self) -> BoxFuture<'static, Result<Vec<ModelChoice>, String>> {
        let models = ["opencode-go:glm-5.3", "opencode-go:kimi-k3"]
            .map(|id| ModelChoice {
                alias: id.to_owned(),
                model: ModelId::new(id),
            })
            .to_vec();
        futures::future::ready(Ok(models)).boxed()
    }

    /// Records the pick, as saving it would.
    fn select_model(&self, model: ModelId) -> BoxFuture<'static, Result<String, String>> {
        self.logins.lock().unwrap().push(format!("model {model}"));
        futures::future::ready(Ok(format!("model `{model}` selected"))).boxed()
    }

    fn new_session(&self) -> BoxFuture<'static, ()> {
        let conversation = Arc::clone(&self.conversation);
        async move {
            *conversation.lock().await = Conversation::new("/ws", 0);
        }
        .boxed()
    }

    fn resume_pending(&self, cancel: CancelToken) -> BoxFuture<'static, TurnEnd> {
        let (tx, rx) = oneshot::channel();
        let service = Arc::clone(&self.service);
        let conversation = Arc::clone(&self.conversation);
        std::thread::spawn(move || {
            let end = futures::executor::block_on(async move {
                let opts = TurnOptions::new(ModelRole::Main).with_cancel(cancel);
                let mut conversation = conversation.lock().await;
                match service.resume_pending_tools(&mut conversation, opts).await {
                    Ok(Some(outcome)) => TurnEnd::Finished(outcome.stop_reason),
                    Ok(None) => TurnEnd::Finished(StopReason::EndTurn),
                    Err(err) => TurnEnd::Failed(err.to_string()),
                }
            });
            tx.send(end).ok();
        });
        rx.map(|end| end.unwrap_or(TurnEnd::Failed("turn lost".into())))
            .boxed()
    }

    /// Adds the images as blocks, as the binary does.
    fn post_images(
        &self,
        caption: String,
        images: Vec<ImageAttachment>,
    ) -> BoxFuture<'static, Reply> {
        for block in application::usecases::user_blocks::user_blocks(&caption, &images) {
            self.events.emit(DomainEvent::BlockAdded { block });
        }
        futures::future::ready(Reply::Done).boxed()
    }

    /// Callback 7 is `function() app.cmd("clear") end`.
    fn run_callback(&self, id: u64, _args: serde_json::Value) -> BoxFuture<'static, Reply> {
        let reply = match id {
            7 => Reply::Actions(vec![PluginAction::Cmd {
                command: "clear".into(),
            }]),
            _ => Reply::Notice(NoticeLevel::Error, format!("callback {id}?")),
        };
        futures::future::ready(reply).boxed()
    }

    /// Only `sk-ant-good` is accepted; `sk-ant-slow` never answers.
    fn login(&self, _account: Account, key: String) -> BoxFuture<'static, Result<String, String>> {
        self.logins.lock().unwrap().push(key.clone());
        if key == "sk-ant-slow" {
            return futures::future::pending().boxed();
        }
        let result = if key == "sk-ant-good" {
            Ok("stored".to_owned())
        } else {
            Err("Anthropic rejected this key".to_owned())
        };
        futures::future::ready(result).boxed()
    }

    fn logout(&self, _account: Option<Account>) -> BoxFuture<'static, Result<String, String>> {
        futures::future::ready(Ok("deleted".to_owned())).boxed()
    }

    /// `/review` sends a prompt with the plan role; `/loop` calls itself;
    /// `/slow` answers after 2 s, like a flow running to its gate.
    fn run_command(&self, name: &str, args: &str) -> Option<BoxFuture<'static, Reply>> {
        let reply = match name {
            "slow" => {
                let (tx, rx) = oneshot::channel();
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_secs(2));
                    tx.send(Reply::Notice(NoticeLevel::Info, "slow done".into()))
                        .ok();
                });
                return Some(rx.map(|reply| reply.unwrap_or(Reply::Done)).boxed());
            }
            "review" => Reply::Actions(vec![PluginAction::Send {
                text: format!("revisa {args}"),
                role: Some(ModelRole::Plan),
            }]),
            "loop" => Reply::Actions(vec![PluginAction::Cmd {
                command: "loop".into(),
            }]),
            "!" => Reply::Notice(NoticeLevel::Info, format!("ran [{args}]")),
            _ => return None,
        };
        Some(futures::future::ready(reply).boxed())
    }
}

struct Harness {
    session: Entity<Session>,
    /// The agent's policy.
    permissions: SharedPermissions,
    /// Holds the project's `permissions.local.toml` (workspace `/ws`).
    rules_file: Arc<MemoryPermissionStore>,
    llm: Arc<ScriptedLlm>,
    sink: Arc<RecordingSink>,
    logins: Arc<Mutex<Vec<String>>>,
    reopens: Rc<RefCell<Vec<SessionEvent>>>,
}

/// Hosts the root view as the window's root.
struct Host(Entity<RootView>);

impl Render for Host {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.0.clone())
    }
}

fn harness(
    script: Vec<ScriptedResponse>,
    cx: &mut TestAppContext,
) -> (Harness, &mut VisualTestContext) {
    harness_with(script, Vec::new(), cx)
}

fn harness_with(
    script: Vec<ScriptedResponse>,
    history: Vec<Block>,
    cx: &mut TestAppContext,
) -> (Harness, &mut VisualTestContext) {
    harness_full(script, history, |_, _| {}, cx)
}

/// A harness whose agent also has the tools `register` adds; the
/// `FormPort` given answers in the session's form blocks.
fn harness_full(
    script: Vec<ScriptedResponse>,
    history: Vec<Block>,
    register: impl FnOnce(&mut ToolRegistry, Arc<dyn FormPort>),
    cx: &mut TestAppContext,
) -> (Harness, &mut VisualTestContext) {
    harness_resuming(script, Conversation::new("/ws", 0), history, register, cx)
}

/// [`harness_full`] over a saved `conversation`; a turn it leaves
/// pending is resumed as the session opens.
fn harness_resuming(
    script: Vec<ScriptedResponse>,
    conversation: Conversation,
    history: Vec<Block>,
    register: impl FnOnce(&mut ToolRegistry, Arc<dyn FormPort>),
    cx: &mut TestAppContext,
) -> (Harness, &mut VisualTestContext) {
    harness_parts(script, conversation, history, register, |_| {}, cx)
}

/// A harness whose session runs `keymap` (no `<leader>x` callback).
fn harness_keymap(
    keymap: Keymap<Action>,
    cx: &mut TestAppContext,
) -> (Harness, &mut VisualTestContext) {
    harness_parts(
        Vec::new(),
        Conversation::new("/ws", 0),
        Vec::new(),
        |_, _| {},
        move |parts| parts.keymap = keymap,
        cx,
    )
}

/// [`harness_resuming`], with `tweak` changing the parts last.
fn harness_parts(
    script: Vec<ScriptedResponse>,
    conversation: Conversation,
    history: Vec<Block>,
    register: impl FnOnce(&mut ToolRegistry, Arc<dyn FormPort>),
    tweak: impl FnOnce(&mut SessionParts),
    cx: &mut TestAppContext,
) -> (Harness, &mut VisualTestContext) {
    cx.executor().allow_parking();
    cx.update(gpui_component::init);
    let fs = Arc::new(InMemoryFs::new(test_root()).with_file("src/lib.rs", "pub fn add() {}\n"));
    let (sink, events) = RecordingSink::with_channel();
    let sink = Arc::new(sink);
    let (approval, approvals) = UiApproval::channel();
    let (form, forms) = crate::form::UiForm::channel();
    let llm = Arc::new(ScriptedLlm::new(script));
    let mut tools = ToolRegistry::new();
    register(&mut tools, Arc::new(form));
    register_file_tools(
        &mut tools,
        FileToolPorts {
            file_search: fs.clone(),
            content_search: fs.clone(),
            fs: fs.clone(),
            shell: Arc::new(ScriptedShell::new()),
            rewriter: None,
        },
    )
    .unwrap();
    let service = AgentService::new(
        AgentDeps {
            llm: llm.clone(),
            events: sink.clone(),
            approval: Arc::new(approval),
            telemetry: Arc::new(RecordingTelemetry::new()),
        },
        tools,
        ModelPolicy::default(),
        Arc::new(Mutex::new(PermissionPolicy::new(PermissionMode::Ask))),
    );
    let rules_file = Arc::new(MemoryPermissionStore::new());
    let service = service.with_project_rules(ProjectRules::new(rules_file.clone(), "/ws"));
    let permissions = Arc::clone(service.permissions());
    let logins = Arc::new(Mutex::new(Vec::new()));
    let resume_pending = !conversation.pending_tool_uses().is_empty();
    let controller = Arc::new(AgentController {
        service: Arc::new(service),
        conversation: Arc::new(futures::lock::Mutex::new(conversation)),
        events: sink.clone(),
        logins: Arc::clone(&logins),
    });
    let mut parts = SessionParts::new(controller, events, approvals, fs.clone(), fs.clone());
    parts.fs = Some(fs);
    parts.forms = Some(forms);
    parts.resume_pending = resume_pending;
    parts.keymap.set("<leader>x", Action::Callback(7)).unwrap();
    parts.history = history;
    tweak(&mut parts);
    let slot = Rc::new(RefCell::new(None));
    let out = slot.clone();
    let (_host, cx) = cx.add_window_view(move |window, cx| {
        let (root, session) = open(parts, window, cx);
        *out.borrow_mut() = Some(session);
        Host(root)
    });
    cx.run_until_parked();
    let session = slot.borrow_mut().take().unwrap();
    let reopens = Rc::new(RefCell::new(Vec::new()));
    let seen = reopens.clone();
    cx.update(|_, cx| {
        cx.subscribe(&session, move |_, event: &SessionEvent, _| {
            seen.borrow_mut().push(event.clone());
        })
        .detach();
    });
    (
        Harness {
            session,
            permissions,
            rules_file,
            llm,
            sink,
            logins,
            reopens,
        },
        cx,
    )
}

/// Runs the UI until `done` holds; the agent runs on other threads.
fn wait_until(
    h: &Harness,
    cx: &mut VisualTestContext,
    what: &str,
    done: impl Fn(&Session, &App) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        cx.run_until_parked();
        if h.session.read_with(cx, |s, cx| done(s, cx)) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn blocks(h: &Harness, cx: &mut VisualTestContext) -> Vec<BlockKind> {
    h.session.read_with(cx, |s, cx| {
        s.timeline()
            .read(cx)
            .state()
            .entries()
            .iter()
            .map(|e| e.block.kind.clone())
            .collect()
    })
}

fn text_of(kind: &BlockKind) -> Option<&str> {
    match kind {
        BlockKind::Text { markdown } => Some(markdown),
        _ => None,
    }
}

fn send(text: &str, cx: &mut VisualTestContext) {
    cx.simulate_input(text);
    cx.simulate_keystrokes("enter");
}

fn finished(h: &Harness) -> Vec<StopReason> {
    h.sink
        .events()
        .into_iter()
        .filter_map(|event| match event {
            DomainEvent::TurnFinished { stop_reason, .. } => Some(stop_reason),
            _ => None,
        })
        .collect()
}

fn login_view(h: &Harness, cx: &mut VisualTestContext) -> Entity<LoginView> {
    h.session.read_with(cx, |s, _| s.login_view().clone())
}

fn submit_key(h: &Harness, key: &str, cx: &mut VisualTestContext) {
    let login = login_view(h, cx);
    cx.update(|window, cx| {
        login.update(cx, |login, cx| {
            login.set_key(key, window, cx);
            login.submit(cx);
        });
    });
}

#[gpui::test]
fn login_checks_the_key_out_of_the_prompt_and_reopens(cx: &mut TestAppContext) {
    let (h, cx) = harness(Vec::new(), cx);
    send("/login", cx);
    cx.run_until_parked();
    let login = login_view(&h, cx);
    assert!(login.read_with(cx, |l, _| l.is_open()));

    submit_key(&h, "sk-ant-bad", cx);
    wait_until(&h, cx, "the rejection", |s, cx| {
        s.login_view().read(cx).error().is_some()
    });
    assert!(
        login.read_with(cx, |l, _| l.is_open()),
        "an error keeps it open"
    );
    assert!(h.reopens.borrow().is_empty());

    submit_key(&h, "sk-ant-good", cx);
    wait_until(&h, cx, "the dialog to close", |s, cx| {
        !s.login_view().read(cx).is_open()
    });
    assert_eq!(*h.logins.lock().unwrap(), ["sk-ant-bad", "sk-ant-good"]);
    assert_eq!(
        *h.reopens.borrow(),
        [SessionEvent::Reopen {
            notice: Some((NoticeLevel::Info, "stored".into()))
        }]
    );
    let history = h
        .session
        .read_with(cx, |s, cx| s.input().read(cx).history().to_vec());
    assert!(
        history.iter().all(|line| !line.contains("sk-ant")),
        "{history:?}"
    );
    assert!(h.llm.requests().is_empty(), "no turn ran");
}

#[gpui::test]
fn login_names_the_account_and_tab_switches_it(cx: &mut TestAppContext) {
    let (h, cx) = harness(Vec::new(), cx);
    send("/login opencode", cx);
    cx.run_until_parked();
    let login = login_view(&h, cx);
    assert_eq!(
        login.read_with(cx, |l, _| (l.is_open(), l.account())),
        (true, Account::OpenCode)
    );
    cx.simulate_keystrokes("tab");
    cx.run_until_parked();
    assert_eq!(login.read_with(cx, |l, _| l.account()), Account::Anthropic);
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();

    send("/login openai", cx);
    cx.run_until_parked();
    assert!(!login.read_with(cx, |l, _| l.is_open()), "unknown account");
}

#[gpui::test]
fn a_login_check_can_be_cancelled_and_times_out(cx: &mut TestAppContext) {
    let (h, cx) = harness(Vec::new(), cx);
    send("/login", cx);
    cx.run_until_parked();
    let login = login_view(&h, cx);
    login.update_in(cx, |l, window, cx| {
        l.set_key("sk-ant-slow", window, cx);
        l.submit(cx);
    });
    cx.run_until_parked();
    assert!(login.read_with(cx, |l, _| l.is_busy()));
    cx.simulate_keystrokes("escape");
    assert!(
        !login.read_with(cx, |l, _| l.is_open()),
        "Esc cancels the check"
    );
    h.session.read_with(cx, |s, _| {
        assert!(s.login_check.is_none(), "the check is dropped")
    });
    cx.update(|window, cx| {
        let input = h.session.read(cx).input().focus_handle(cx);
        assert!(input.contains_focused(window, cx), "back in the prompt");
    });

    // The Cancel button works as well, and an unanswered check times out.
    send("/login", cx);
    cx.run_until_parked();
    login.update_in(cx, |l, window, cx| {
        l.set_key("sk-ant-slow", window, cx);
        l.submit(cx);
    });
    cx.run_until_parked();
    cx.executor().advance_clock(LOGIN_TIMEOUT);
    cx.run_until_parked();
    login.read_with(cx, |l, _| {
        assert!(l.is_open() && !l.is_busy());
        assert!(l.error().is_some_and(|e| e.contains("did not answer")));
    });
    let cancel = cx.debug_bounds("login-cancel").expect("Cancel drawn");
    cx.simulate_click(cancel.center(), gpui::Modifiers::none());
    assert!(!login.read_with(cx, |l, _| l.is_open()), "Cancel closes it");
}

#[gpui::test]
fn login_tabs_switch_the_account_on_click(cx: &mut TestAppContext) {
    let (h, cx) = harness(Vec::new(), cx);
    send("/login", cx);
    cx.run_until_parked();
    let login = login_view(&h, cx);
    assert_eq!(login.read_with(cx, |l, _| l.account()), Account::Anthropic);

    let tab = cx
        .debug_bounds("login-tab-opencode")
        .expect("OpenCode tab drawn");
    cx.simulate_click(tab.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    assert_eq!(login.read_with(cx, |l, _| l.account()), Account::OpenCode);

    cx.simulate_input("oc-typed");
    cx.run_until_parked();
    login.update(cx, |l, cx| l.submit(cx));
    cx.run_until_parked();
    assert_eq!(
        *h.logins.lock().unwrap(),
        ["oc-typed"],
        "the field kept the focus"
    );
}

#[gpui::test]
fn logout_reopens_and_esc_closes_the_dialog(cx: &mut TestAppContext) {
    let (h, cx) = harness(Vec::new(), cx);
    send("/logout", cx);
    cx.run_until_parked();
    assert_eq!(
        *h.reopens.borrow(),
        [SessionEvent::Reopen {
            notice: Some((NoticeLevel::Info, "deleted".into()))
        }]
    );
    let login = login_view(&h, cx);
    assert!(!login.read_with(cx, |l, _| l.is_open()));
    cx.update(|window, cx| {
        login.update(cx, |login, cx| login.show(Account::Anthropic, window, cx));
    });
    cx.run_until_parked();
    cx.simulate_input("sk-ant-typed");
    cx.run_until_parked();
    let prompt = h.session.read_with(cx, |s, cx| s.input().read(cx).text(cx));
    assert!(
        !prompt.contains("sk-ant"),
        "the key reached the prompt: {prompt}"
    );
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(!login.read_with(cx, |l, _| l.is_open()), "Esc closes it");
}

#[gpui::test]
fn a_prompt_runs_a_full_turn_with_streaming_and_a_tool(cx: &mut TestAppContext) {
    let script = vec![
        ScriptedResponse::text("Busco la función.").then_tool_use(
            "toolu_1",
            "find_file",
            json!({ "query": "lib" }),
        ),
        ScriptedResponse::text("Está en src/lib.rs."),
    ];
    let (h, cx) = harness(script, cx);
    send("dónde está add", cx);
    wait_until(&h, cx, "the turn to end", |s, _| !s.is_running());

    let kinds = blocks(&h, cx);
    let texts: Vec<_> = kinds.iter().filter_map(text_of).collect();
    assert_eq!(
        texts,
        [
            "> dónde está add\n",
            "Busco la función.",
            "Está en src/lib.rs."
        ]
    );
    assert!(
        kinds.iter().any(|kind| matches!(
            kind,
            BlockKind::ToolCall { name, status: ToolStatus::Succeeded, .. } if name == "find_file"
        )),
        "{kinds:?}"
    );
    let deltas = h
        .sink
        .events()
        .iter()
        .filter(|e| matches!(e, DomainEvent::TextDelta { .. }))
        .count();
    assert!(deltas >= 2, "the text arrived as deltas");
    assert_eq!(finished(&h), [StopReason::EndTurn]);
    assert_eq!(h.llm.remaining(), 0);
    h.session.read_with(cx, |s, cx| {
        assert!(!s.status().read(cx).state().is_busy());
    });
}

#[gpui::test]
fn ctrl_c_interrupts_the_turn_and_the_input_accepts_the_next(cx: &mut TestAppContext) {
    let script = vec![ScriptedResponse::text("hola mundo cruel").hang_after(4)];
    let (h, cx) = harness(script, cx);
    send("saluda", cx);
    wait_until(&h, cx, "the partial answer", |s, cx| {
        s.timeline()
            .read(cx)
            .state()
            .entries()
            .iter()
            .filter_map(|e| text_of(&e.block.kind))
            .any(|text| text.starts_with("hola"))
    });
    h.session
        .read_with(cx, |s, _| assert!(s.is_running(), "the model hangs"));

    cx.simulate_input("borrador");
    cx.simulate_keystrokes("ctrl-c");
    cx.run_until_parked();
    h.session.read_with(cx, |s, cx| {
        assert_eq!(
            s.input().read(cx).text(cx),
            "",
            "the first clears the prompt"
        );
        assert!(s.is_running(), "and leaves the turn alone");
    });

    cx.simulate_keystrokes("ctrl-c");
    wait_until(&h, cx, "the interrupted turn", |s, _| !s.is_running());
    assert_eq!(finished(&h), [StopReason::Interrupted]);

    h.llm.push(ScriptedResponse::text("de nuevo"));
    send("otra vez", cx);
    wait_until(&h, cx, "the second turn", |s, _| !s.is_running());
    assert_eq!(finished(&h), [StopReason::Interrupted, StopReason::EndTurn]);
    let kinds = blocks(&h, cx);
    assert_eq!(
        kinds.iter().filter_map(text_of).next_back(),
        Some("de nuevo")
    );
}

#[gpui::test]
fn a_running_command_keeps_the_session_busy_and_queues_prompts(cx: &mut TestAppContext) {
    let (h, cx) = harness(vec![ScriptedResponse::text("hecho")], cx);
    cx.simulate_input("/slow");
    cx.simulate_keystrokes("enter");
    h.session.read_with(cx, |s, cx| {
        assert!(s.is_busy() && !s.is_running());
        assert!(s.activity().read(cx).state().is_working());
    });
    send("mientras", cx);
    h.session.read_with(cx, |s, cx| {
        assert_eq!(s.queued(), 1);
        assert_eq!(s.activity().read(cx).state().queue(), ["mientras"]);
    });
    assert_eq!(h.llm.remaining(), 1, "nothing was sent yet");

    // Up in the empty prompt takes it back; Enter queues it again.
    cx.simulate_keystrokes("up");
    h.session.read_with(cx, |s, cx| {
        assert_eq!(s.queued(), 0);
        assert_eq!(s.input().read(cx).text(cx), "mientras");
    });
    cx.simulate_input(" tanto");
    cx.simulate_keystrokes("enter");
    h.session.read_with(cx, |s, _| assert_eq!(s.queued(), 1));

    wait_until(&h, cx, "the command and the queued turn", |s, _| {
        !s.is_busy() && s.queued() == 0
    });
    assert_eq!(h.llm.remaining(), 0, "the queued prompt was sent");
    let kinds = blocks(&h, cx);
    assert!(
        kinds
            .iter()
            .filter_map(text_of)
            .any(|text| text == "> mientras tanto\n"),
        "{kinds:?}"
    );
    h.session.read_with(cx, |s, cx| {
        let activity = s.activity().read(cx).state();
        assert!(!activity.is_working() && activity.queue().is_empty());
    });
}

#[gpui::test]
fn a_prompt_sent_while_a_form_waits_answers_it_in_prose(cx: &mut TestAppContext) {
    use application::tools::form::AskUserTool;
    use domain::form::FormState;

    let script = vec![
        ScriptedResponse::tool_use(
            "toolu_1",
            "ask_user",
            json!({"title": "Which?", "schema": {"properties": {
                "lang": {"type": "string", "enum": ["rust", "lua"]}}}}),
        ),
        ScriptedResponse::text("vale"),
    ];
    let register = |tools: &mut ToolRegistry, port: Arc<dyn FormPort>| {
        tools.register_typed(AskUserTool::new(port)).unwrap();
    };
    let (h, cx) = harness_full(script, Vec::new(), register, cx);
    send("hola", cx);
    wait_until(&h, cx, "the form", |s, cx| s.waiting_form(cx).is_some());
    h.session.read_with(cx, |s, _| assert!(s.is_running()));

    // Back to the prompt, and the answer is typed there.
    h.session
        .update_in(cx, |s, window, cx| s.focus_input(window, cx));
    send("mejor en python", cx);
    wait_until(&h, cx, "the turn", |s, _| !s.is_running());
    h.session.read_with(cx, |s, _| assert_eq!(s.queued(), 0));
    assert_eq!(
        h.llm.remaining(),
        0,
        "the tool returned and the model answered"
    );
    let replied: Vec<Option<String>> = h
        .sink
        .events()
        .into_iter()
        .filter_map(|event| match event {
            DomainEvent::BlockUpdated { block } => match block.kind {
                BlockKind::Form(spec) if spec.state == FormState::Replied => Some(spec.reply),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(replied, [Some("mejor en python".to_owned())]);
    let kinds = blocks(&h, cx);
    assert_eq!(
        kinds.iter().filter_map(text_of).next_back(),
        Some("vale"),
        "{kinds:?}"
    );
}

#[gpui::test]
fn a_form_left_waiting_by_a_restart_is_asked_again_on_open(cx: &mut TestAppContext) {
    use application::tools::form::AskUserTool;
    use domain::block::BlockId;
    use domain::conversation::{Message, Role, ToolUseId, Turn};
    use domain::form::{FormSpec, FormState};

    // The saved session: a turn cut while `ask_user` waited, and the
    // blocks the timeline showed then.
    let input = json!({"title": "Which?", "schema": {"properties": {
        "lang": {"type": "string", "enum": ["rust", "lua"]}}}});
    let mut conversation = Conversation::new("/ws", 0);
    let mut turn = Turn::new(Message::user_text("hola"));
    turn.messages.push(Message::new(
        Role::Assistant,
        vec![ContentPart::ToolUse {
            id: ToolUseId::new("toolu_1"),
            name: "ask_user".into(),
            input: input.clone(),
        }],
    ));
    conversation.push_turn(turn, 1);
    let spec: FormSpec = serde_json::from_value(json!({
        "title": "Which?",
        "fields": [{"name": "lang", "type": "enum",
                    "options": [{"value": "rust"}, {"value": "lua"}]}]
    }))
    .unwrap();
    let history = vec![
        prompt_block("hola"),
        Block {
            id: BlockId::for_tool_use("toolu_1", "tool_call"),
            kind: BlockKind::ToolCall {
                tool_use_id: "toolu_1".into(),
                name: "ask_user".into(),
                summary: "Which?".into(),
                input,
                status: ToolStatus::Running,
                output: None,
            },
        },
        Block {
            id: BlockId::for_tool_use("toolu_1", "form"),
            kind: BlockKind::Form(spec),
        },
    ];
    let register = |tools: &mut ToolRegistry, port: Arc<dyn FormPort>| {
        tools.register_typed(AskUserTool::new(port)).unwrap();
    };
    let (h, cx) = harness_resuming(
        vec![ScriptedResponse::text("vale")],
        conversation,
        history,
        register,
        cx,
    );

    // Opened: the turn resumes and the form waits again, in its block.
    wait_until(&h, cx, "the form", |s, cx| s.waiting_form(cx).is_some());
    h.session.read_with(cx, |s, _| assert!(s.is_running()));
    let forms = blocks(&h, cx)
        .iter()
        .filter(|kind| matches!(kind, BlockKind::Form(_)))
        .count();
    assert_eq!(forms, 1, "the same block, not a second form");

    // Answered in the chat: the model goes on.
    h.session
        .update_in(cx, |s, window, cx| s.focus_input(window, cx));
    send("rust", cx);
    wait_until(&h, cx, "the turn", |s, _| !s.is_running());
    let kinds = blocks(&h, cx);
    assert!(
        kinds.iter().any(|kind| matches!(
            kind,
            BlockKind::Form(spec) if spec.state == FormState::Replied
        )),
        "{kinds:?}"
    );
    assert_eq!(
        kinds.iter().filter_map(text_of).next_back(),
        Some("vale"),
        "{kinds:?}"
    );
    assert_eq!(finished(&h), [StopReason::EndTurn]);
}

#[gpui::test]
fn prompts_sent_during_a_turn_are_queued(cx: &mut TestAppContext) {
    let script = vec![
        ScriptedResponse::text("uno").hang_after(2),
        ScriptedResponse::text("dos"),
    ];
    let (h, cx) = harness(script, cx);
    send("primero", cx);
    // The turn runs on another thread: wait until it has taken its
    // answer, or Ctrl+C could land before the model is ever called.
    let llm = h.llm.clone();
    wait_until(&h, cx, "the first turn to reach the model", move |_, _| {
        llm.remaining() == 1
    });
    send("segundo", cx);
    h.session.read_with(cx, |s, _| assert_eq!(s.queued(), 1));
    // Ctrl+C drops the queue with the turn.
    cx.simulate_keystrokes("ctrl-c");
    wait_until(&h, cx, "the interrupted turn", |s, _| !s.is_running());
    h.session.read_with(cx, |s, _| assert_eq!(s.queued(), 0));
    assert_eq!(h.llm.remaining(), 1, "the queued prompt was not sent");
}

#[gpui::test]
fn leader_u_takes_the_queued_prompt_back_and_leader_ctrl_c_interrupts(cx: &mut TestAppContext) {
    let script = vec![
        ScriptedResponse::text("uno").hang_after(2),
        ScriptedResponse::text("dos"),
    ];
    let (h, cx) = harness(script, cx);
    send("primero", cx);
    let llm = h.llm.clone();
    wait_until(&h, cx, "the first turn to reach the model", move |_, _| {
        llm.remaining() == 1
    });
    send("segundo", cx);
    h.session.read_with(cx, |s, _| assert_eq!(s.queued(), 1));
    cx.simulate_keystrokes("ctrl-g u");
    h.session.read_with(cx, |s, cx| {
        assert_eq!(s.queued(), 0);
        assert_eq!(s.input().read(cx).text(cx), "segundo");
    });
    cx.simulate_keystrokes("ctrl-g ctrl-c");
    wait_until(&h, cx, "the interrupted turn", |s, _| !s.is_running());
    h.session.read_with(cx, |s, cx| {
        assert_eq!(s.input().read(cx).text(cx), "segundo")
    });
}

#[gpui::test]
fn leader_p_cycles_the_permission_mode(cx: &mut TestAppContext) {
    let (h, cx) = harness(Vec::new(), cx);
    cx.simulate_keystrokes("ctrl-g p");
    h.session.read_with(cx, |s, cx| {
        assert_eq!(s.controller.permission_mode(), PermissionMode::Ask.next());
        assert_eq!(
            s.status().read(cx).state().permission_mode(),
            PermissionMode::Ask.next()
        );
    });
}

#[gpui::test]
fn leader_m_puts_model_in_the_prompt_and_esc_keeps_it_for_one_turn(cx: &mut TestAppContext) {
    let (h, cx) = harness(vec![ScriptedResponse::text("ok")], cx);
    cx.simulate_keystrokes("ctrl-g m");
    h.session.read_with(cx, |s, cx| {
        assert_eq!(s.input().read(cx).text(cx), "/model ");
    });
    cx.simulate_input("opus");
    cx.simulate_keystrokes("escape enter");
    cx.run_until_parked();
    h.session
        .read_with(cx, |s, _| assert_eq!(s.model_override(), Some("opus")));

    send("hola", cx);
    wait_until(&h, cx, "the turn", |s, _| !s.is_running());
    h.session.read_with(cx, |s, _| {
        assert_eq!(s.model_override(), None, "one turn only")
    });
    let model = h.llm.last_request().map(|r| r.model.spec.id);
    assert_eq!(model, Some(ModelPolicy::default().expand("opus")));
}

#[gpui::test]
fn the_model_popup_lists_the_catalog_filters_and_saves(cx: &mut TestAppContext) {
    let (h, cx) = harness(Vec::new(), cx);
    send("/model", cx);
    cx.run_until_parked();
    h.session.read_with(cx, |s, cx| {
        let input = s.input().read(cx);
        let Some(ModelPopup::Entries(entries)) = input.model_popup(cx) else {
            panic!("the popup lists the catalog");
        };
        assert!(
            entries
                .iter()
                .any(|c| c.model.as_str() == "opencode-go:kimi-k3")
        );
        // Claude aliases: no provider here serves them.
        assert!(
            entries.iter().all(|c| c.model.provider().is_some()),
            "{entries:?}"
        );
    });
    cx.simulate_input("kimi");
    h.session.read_with(cx, |s, cx| {
        let Some(ModelPopup::Entries(entries)) = s.input().read(cx).model_popup(cx) else {
            panic!("still entries");
        };
        assert_eq!(entries.len(), 1, "{entries:?}");
    });
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(*h.logins.lock().unwrap(), ["model opencode-go:kimi-k3"]);
    assert_eq!(
        *h.reopens.borrow(),
        [SessionEvent::Reopen {
            notice: Some((
                NoticeLevel::Info,
                "model `opencode-go:kimi-k3` selected".into()
            )),
        }]
    );
    h.session.read_with(cx, |s, _| {
        assert_eq!(s.model_override(), None, "a saved pick is not an override");
    });
}

#[gpui::test]
fn model_set_saves_and_model_clear_drops_the_override(cx: &mut TestAppContext) {
    let (h, cx) = harness(Vec::new(), cx);
    send("/model opus", cx);
    cx.run_until_parked();
    h.session
        .read_with(cx, |s, _| assert_eq!(s.model_override(), Some("opus")));
    send("/model clear", cx);
    cx.run_until_parked();
    h.session
        .read_with(cx, |s, _| assert_eq!(s.model_override(), None));
    send("/model set opencode-go:glm-5.3", cx);
    cx.run_until_parked();
    assert_eq!(*h.logins.lock().unwrap(), ["model opencode-go:glm-5.3"]);
    assert_eq!(h.reopens.borrow().len(), 1);
}

#[gpui::test]
fn plugin_modals_open_over_the_window_and_close(cx: &mut TestAppContext) {
    use domain::block::CustomNode;
    use domain::modal::{ModalId, ModalSpec};
    let (h, cx) = harness(Vec::new(), cx);
    let id = ModalId::new("cov/report");
    h.sink.emit(DomainEvent::ModalOpened {
        spec: ModalSpec {
            id: id.clone(),
            plugin: "cov".into(),
            title: "Report".into(),
            width: domain::modal::Percent::FULL,
            height: domain::modal::Percent::FULL,
            position: domain::modal::ModalPosition::CENTER,
            content: CustomNode::Markdown {
                source: "# Coverage".into(),
            },
        },
    });
    cx.run_until_parked();
    let modal = h.session.read_with(cx, |s, _| s.modal().clone());
    modal.read_with(cx, |m, _| assert_eq!(m.ids(), std::slice::from_ref(&id)));
    assert!(blocks(&h, cx).is_empty(), "modals are not timeline blocks");
    // Esc closes it and the input gets the focus back.
    cx.simulate_keystrokes("escape");
    modal.read_with(cx, |m, _| assert!(!m.is_open()));
    cx.update(|window, cx| {
        let input = h.session.read(cx).input().focus_handle(cx);
        assert!(input.contains_focused(window, cx));
    });
    h.sink.emit(DomainEvent::ModalOpened {
        spec: ModalSpec {
            id: id.clone(),
            plugin: "cov".into(),
            title: "Report".into(),
            width: domain::modal::Percent::FULL,
            height: domain::modal::Percent::FULL,
            position: domain::modal::ModalPosition::CENTER,
            content: CustomNode::Text { text: "x".into() },
        },
    });
    cx.run_until_parked();
    h.session
        .update_in(cx, |s, window, cx| s.command("modal close", window, cx));
    modal.read_with(cx, |m, _| assert!(!m.is_open()));
}

#[gpui::test]
fn commands_set_mode_and_model_and_clear(cx: &mut TestAppContext) {
    let (h, cx) = harness(vec![ScriptedResponse::text("ok")], cx);
    send("hola", cx);
    wait_until(&h, cx, "the turn", |s, _| !s.is_running());
    assert!(!blocks(&h, cx).is_empty());

    send("/mode plan", cx);
    send("/model opus", cx);
    send("/clear", cx);
    cx.run_until_parked();
    h.session.read_with(cx, |s, _| {
        assert_eq!(s.controller.permission_mode(), PermissionMode::Plan);
        assert_eq!(s.model_override(), Some("opus"));
    });
    assert!(blocks(&h, cx).is_empty(), "/clear empties the timeline");
}

#[gpui::test]
fn clear_during_a_turn_interrupts_it_and_resets_once_it_ends(cx: &mut TestAppContext) {
    let script = vec![
        ScriptedResponse::text("hola mundo cruel").hang_after(4),
        ScriptedResponse::text("nuevo"),
    ];
    let (h, cx) = harness(script, cx);
    send("saluda", cx);
    wait_until(&h, cx, "the partial answer", |s, cx| {
        s.timeline()
            .read(cx)
            .state()
            .entries()
            .iter()
            .filter_map(|e| text_of(&e.block.kind))
            .any(|text| text.starts_with("hola"))
    });

    send("/clear", cx);
    wait_until(&h, cx, "the interrupted turn", |s, _| !s.is_running());
    cx.run_until_parked();
    assert_eq!(finished(&h), [StopReason::Interrupted]);
    assert!(blocks(&h, cx).is_empty(), "the timeline was cleared");
    let session = h.session.clone();
    assert!(
        cx.update(|window, cx| session.read(cx).input().read(cx).owns_focus(window, cx)),
        "the prompt keeps the focus"
    );

    // The session starts over: the model sees only the new prompt.
    send("otra", cx);
    wait_until(&h, cx, "the new turn", |s, _| !s.is_running());
    let kinds = blocks(&h, cx);
    assert_eq!(
        kinds.iter().filter_map(text_of).collect::<Vec<_>>(),
        ["> otra\n", "nuevo"]
    );
    let last = h.llm.last_request().expect("a request");
    assert_eq!(last.messages.len(), 1, "{last:?}");
}

#[gpui::test]
fn a_resumed_history_is_shown_before_new_blocks(cx: &mut TestAppContext) {
    let (h, cx) = harness_with(
        vec![ScriptedResponse::text("sigo")],
        vec![
            prompt_block("antes"),
            Block::new(BlockKind::Text {
                markdown: "respuesta".into(),
            }),
        ],
        cx,
    );
    send("después", cx);
    wait_until(&h, cx, "the turn", |s, _| !s.is_running());
    let kinds = blocks(&h, cx);
    assert_eq!(
        kinds.iter().filter_map(text_of).collect::<Vec<_>>(),
        ["> antes\n", "respuesta", "> después\n", "sigo"]
    );
}

#[gpui::test]
fn a_lua_mapping_runs_the_command_it_asks_for(cx: &mut TestAppContext) {
    let (h, cx) = harness(vec![ScriptedResponse::text("ok")], cx);
    send("hola", cx);
    wait_until(&h, cx, "the turn", |s, _| !s.is_running());
    assert!(!blocks(&h, cx).is_empty());
    cx.simulate_keystrokes("ctrl-g x");
    cx.run_until_parked();
    assert!(blocks(&h, cx).is_empty(), "app.cmd(\"clear\") ran");
}

#[gpui::test]
fn a_controller_command_sends_a_prompt_with_its_role(cx: &mut TestAppContext) {
    let (h, cx) = harness(vec![ScriptedResponse::text("sin riesgos")], cx);
    cx.simulate_input("/review todo");
    cx.simulate_keystrokes("enter");
    wait_until(&h, cx, "the turn", |s, _| {
        !s.is_running() && h.llm.remaining() == 0
    });
    let request = h.llm.last_request().unwrap();
    assert_eq!(request.model.role, ModelRole::Plan);
    let kinds = blocks(&h, cx);
    assert_eq!(
        kinds.iter().filter_map(text_of).next(),
        Some("> revisa todo\n")
    );
}

#[gpui::test]
fn a_command_calling_itself_stops(cx: &mut TestAppContext) {
    let (h, cx) = harness(Vec::new(), cx);
    cx.simulate_input("/loop");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    h.session.read_with(cx, |s, _| assert!(!s.is_running()));
}

#[gpui::test]
fn a_file_cited_by_a_block_is_referenced_in_the_prompt(cx: &mut TestAppContext) {
    let (h, cx) = harness(Vec::new(), cx);
    cx.update(|_, cx| {
        crate::blocks::sources::emit(
            BlockEvent::OpenFile {
                path: WorkspacePath::parse("src/lib.rs").unwrap(),
                line: Some(7),
            },
            cx,
        );
    });
    cx.run_until_parked();
    let text = h.session.read_with(cx, |s, cx| s.input().read(cx).text(cx));
    assert_eq!(text, "@src/lib.rs:7 ");
}

fn help_view(h: &Harness, cx: &mut VisualTestContext) -> Option<Entity<HelpView>> {
    let modal = h.session.read_with(cx, |s, _| s.modal().clone());
    modal.read_with(cx, |m, _| {
        m.view(&help_modal_id())
            .and_then(|view| view.clone().downcast::<HelpView>().ok())
    })
}

#[gpui::test]
fn f1_opens_the_help_and_esc_closes_it(cx: &mut TestAppContext) {
    let (h, cx) = harness(Vec::new(), cx);
    cx.simulate_keystrokes("f1");
    cx.run_until_parked();
    let view = help_view(&h, cx).expect("the help is open");
    view.read_with(cx, |v, _| {
        assert_eq!(v.section(), Section::Commands);
        let names: Vec<_> = v
            .snapshot()
            .commands
            .iter()
            .map(|c| c.name.clone())
            .collect();
        assert!(names.contains(&"help".to_owned()), "{names:?}");
        assert!(v.snapshot().keys.iter().any(|k| k.keys == "<F1>"));
        assert!(
            v.snapshot()
                .models
                .iter()
                .any(|m| m.name == "permission mode" && m.value == "ask")
        );
    });
    cx.update(|window, cx| {
        assert!(
            view.focus_handle(cx).contains_focused(window, cx),
            "the help has the focus"
        );
    });
    // Number keys switch tabs; Tab goes on.
    cx.simulate_keystrokes("2");
    view.read_with(cx, |v, _| assert_eq!(v.section(), Section::Keys));
    cx.simulate_keystrokes("8 tab");
    view.read_with(cx, |v, _| assert_eq!(v.section(), Section::Commands));
    cx.simulate_keystrokes("j j");
    view.read_with(cx, |v, _| assert_eq!(v.selected(), 2));
    cx.simulate_keystrokes("k y");
    let copied = cx.read_from_clipboard().and_then(|item| item.text());
    assert_eq!(copied.as_deref(), Some("/model"));

    cx.simulate_keystrokes("escape");
    assert!(help_view(&h, cx).is_none(), "Esc closes it");
    cx.update(|window, cx| {
        let input = h.session.read(cx).input().focus_handle(cx);
        assert!(input.contains_focused(window, cx), "the focus is back");
    });
}

#[gpui::test]
fn leader_h_opens_the_help_and_typing_filters(cx: &mut TestAppContext) {
    let (h, cx) = harness(Vec::new(), cx);
    cx.simulate_keystrokes("ctrl-g h");
    cx.run_until_parked();
    let view = help_view(&h, cx).expect("F1 opens the help");
    cx.simulate_keystrokes("/");
    cx.update(|window, cx| assert!(view.read(cx).is_searching(window, cx)));
    cx.simulate_input("block.toggle");
    cx.run_until_parked();
    view.read_with(cx, |v, cx| {
        assert_eq!(v.query(cx), "block.toggle");
        let f = v.filtered();
        assert_eq!(f.commands.len(), 0);
        assert_eq!(f.keys.len(), 2, "block.toggle and block.toggle_view");
        // The search moved to the section with matches.
        assert_eq!(v.section(), Section::Keys);
    });
    // Down moves while typing; Enter goes back to the list.
    cx.simulate_keystrokes("down enter");
    cx.update(|window, cx| {
        let v = view.read(cx);
        assert_eq!(v.selected(), 1);
        assert!(!v.is_searching(window, cx));
    });
    cx.simulate_keystrokes("escape");
    assert!(help_view(&h, cx).is_none());
    h.session
        .read_with(cx, |s, cx| assert!(!s.modal().read(cx).is_open()));
    cx.update(|window, cx| {
        let input = h.session.read(cx).input().focus_handle(cx);
        assert!(input.contains_focused(window, cx), "back in the prompt");
    });
}

#[gpui::test]
fn the_focus_cycles_and_typing_or_esc_brings_it_back(cx: &mut TestAppContext) {
    let (h, cx) = harness(vec![ScriptedResponse::text("uno")], cx);
    send("hola", cx);
    wait_until(&h, cx, "the turn", |s, _| !s.is_running());
    let in_input = |cx: &mut VisualTestContext| {
        cx.update(|window, cx| {
            let input = h.session.read(cx).input().focus_handle(cx);
            input.contains_focused(window, cx)
        })
    };
    let in_timeline = |cx: &mut VisualTestContext| {
        cx.update(|window, cx| {
            let timeline = h.session.read(cx).timeline().focus_handle(cx);
            timeline.contains_focused(window, cx)
        })
    };
    cx.simulate_keystrokes("ctrl-tab");
    assert!(in_timeline(cx), "Ctrl+Tab moves to the timeline");
    let selected = |cx: &mut VisualTestContext| {
        h.session
            .read_with(cx, |s, cx| s.timeline().read(cx).state().selected())
    };
    let last = selected(cx);
    assert!(last.is_some(), "a block is selected on entering");
    cx.simulate_keystrokes("up");
    assert_ne!(selected(cx), last, "arrows move the selection");
    // Esc goes back to the prompt.
    cx.simulate_keystrokes("escape");
    assert!(in_input(cx));
    // Typing in the timeline types into the prompt.
    cx.simulate_keystrokes("ctrl-tab");
    assert!(in_timeline(cx));
    cx.simulate_input("x");
    assert!(in_input(cx));
    h.session
        .read_with(cx, |s, cx| assert_eq!(s.input().read(cx).text(cx), "x"));
    // Shortcuts work from the timeline too.
    cx.simulate_keystrokes("ctrl-tab ctrl-g p");
    h.session.read_with(cx, |s, _| {
        assert_eq!(s.controller.permission_mode(), PermissionMode::Ask.next());
    });
}

#[gpui::test]
fn esc_closes_the_outline_before_focusing_the_prompt(cx: &mut TestAppContext) {
    let (h, cx) = harness(vec![ScriptedResponse::text("uno")], cx);
    send("hola", cx);
    wait_until(&h, cx, "the turn", |s, _| !s.is_running());
    let timeline = h.session.read_with(cx, |s, _| s.timeline().clone());
    timeline.update_in(cx, |t, window, cx| t.toggle_outline(window, cx));
    cx.run_until_parked();
    let in_input = |cx: &mut VisualTestContext| {
        cx.update(|window, cx| {
            let input = h.session.read(cx).input().focus_handle(cx);
            input.contains_focused(window, cx)
        })
    };
    assert!(timeline.read_with(cx, |t, _| t.outline_open()));
    assert!(!in_input(cx), "the open menu takes the focus");
    cx.simulate_keystrokes("escape");
    assert!(
        !timeline.read_with(cx, |t, _| t.outline_open()),
        "Esc closes the menu"
    );
    assert!(!in_input(cx), "the first Esc only closes the menu");
    cx.simulate_keystrokes("escape");
    assert!(in_input(cx), "a second Esc goes back to the prompt");
}

#[gpui::test]
fn a_focused_block_widget_gets_esc_and_its_keys(cx: &mut TestAppContext) {
    use domain::devops::flame::{FlameTree, ProfileFormat};
    let mut tree = FlameTree::new();
    tree.add(["main", "parse"], 30);
    tree.add(["idle"], 10);
    let spec = tree.finish(ProfileFormat::Folded, "samples");
    let block = Block::new(BlockKind::FlameGraph(Box::new(spec.clone())));
    let id = block.id;
    let (h, cx) = harness_with(Vec::new(), vec![block], cx);
    cx.run_until_parked();
    let flame = cx.update(|window, cx| {
        crate::blocks::flame::view(id, &spec, crate::timeline::NO_REV, window, cx)
    });
    flame.update_in(cx, |f, window, cx| {
        f.zoom_to(1, cx);
        window.focus(&f.focus_handle(cx), cx);
    });
    cx.run_until_parked();
    let prompt =
        |cx: &mut VisualTestContext| h.session.read_with(cx, |s, cx| s.input().read(cx).text(cx));
    let in_input = |cx: &mut VisualTestContext| {
        cx.update(|window, cx| {
            let input = h.session.read(cx).input().focus_handle(cx);
            input.contains_focused(window, cx)
        })
    };
    cx.simulate_keystrokes("escape");
    assert_eq!(flame.read_with(cx, |f, _| f.zoom()), 0, "Esc zooms out");
    assert!(!in_input(cx), "the widget took the Esc");
    // A key the widget does not take still lands in the prompt.
    cx.simulate_keystrokes("x");
    assert_eq!(prompt(cx), "x");
    assert!(in_input(cx));
}

#[gpui::test]
fn a_focused_list_widget_gets_the_arrows_and_enter(cx: &mut TestAppContext) {
    use domain::devops::logs::{LogFormat, LogSpec};
    let list: domain::widget::list::ListSpec = serde_json::from_value(serde_json::json!({
        "items": [
            { "text": "one", "fields": { "k": "v" } },
            { "text": "two", "fields": { "k": "v" } },
            { "text": "three" },
        ]
    }))
    .unwrap_or_else(|e| panic!("{e}"));
    let spec = LogSpec {
        path: None,
        format: LogFormat::Json,
        total_lines: 3,
        truncated: false,
        list: list.clone(),
        histogram: None,
    };
    let block = Block::new(BlockKind::Logs(Box::new(spec)));
    let id = block.id;
    let (h, cx) = harness_with(Vec::new(), vec![block], cx);
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    let list = cx.update(|window, cx| {
        crate::widgets::list::view(
            crate::blocks::custom::node_id(id, 0),
            &list,
            crate::timeline::NO_REV,
            window,
            cx,
        )
    });
    list.update_in(cx, |l, window, cx| {
        window.focus(&l.focus_handle().clone(), cx);
    });
    cx.run_until_parked();
    cx.simulate_keystrokes("down down enter");
    list.read_with(cx, |l, _| {
        assert_eq!(l.selected(), Some(1), "the arrows reach the list");
        assert!(l.is_open(1), "and so does Enter");
    });
    h.session.read_with(cx, |s, cx| {
        assert_eq!(
            s.input().read(cx).text(cx),
            "",
            "nothing reached the prompt"
        );
    });
}

#[gpui::test]
fn the_picker_closes_by_shortcut_click_outside_and_esc(cx: &mut TestAppContext) {
    let (h, cx) = harness(Vec::new(), cx);
    let mode =
        |cx: &mut VisualTestContext| h.session.read_with(cx, |s, cx| s.picker.read(cx).mode());
    let in_input = |cx: &mut VisualTestContext| {
        cx.update(|window, cx| {
            let input = h.session.read(cx).input().focus_handle(cx);
            input.contains_focused(window, cx)
        })
    };
    // The shortcut toggles it; the other one switches the mode.
    cx.simulate_keystrokes("ctrl-g f f");
    assert_eq!(mode(cx), Some(crate::picker::PickerMode::Files));
    cx.simulate_keystrokes("ctrl-g f g");
    assert_eq!(mode(cx), Some(crate::picker::PickerMode::Grep));
    cx.simulate_keystrokes("ctrl-g f g");
    assert_eq!(mode(cx), None, "the same shortcut closes it");
    assert!(in_input(cx));
    assert_eq!(
        h.session.read_with(cx, |s, cx| s.input().read(cx).text(cx)),
        "",
        "the keys after the leader are not typed"
    );
    // A click on the backdrop closes it.
    cx.simulate_keystrokes("ctrl-g f f");
    cx.run_until_parked();
    let backdrop = cx.debug_bounds("picker-backdrop").expect("backdrop drawn");
    let corner = backdrop.origin + gpui::point(gpui::px(4.), gpui::px(4.));
    cx.simulate_click(corner, gpui::Modifiers::none());
    assert_eq!(mode(cx), None, "a click outside closes it");
    // So does its close button.
    cx.simulate_keystrokes("ctrl-g f f");
    cx.run_until_parked();
    let close = cx.debug_bounds("picker-close").expect("close drawn");
    cx.simulate_click(close.center(), gpui::Modifiers::none());
    assert_eq!(mode(cx), None, "the close button closes it");
    // Esc closes it with the focus outside its input.
    cx.simulate_keystrokes("ctrl-g f f");
    h.session.update_in(cx, |s, window, cx| {
        window.focus(&s.timeline().focus_handle(cx), cx);
    });
    cx.simulate_keystrokes("escape");
    assert_eq!(mode(cx), None, "Esc closes it from anywhere");
}

#[gpui::test]
fn the_palette_opens_the_pickers_and_their_alt_keys_reach_them(cx: &mut TestAppContext) {
    use crate::picker::PickerMode;
    let (h, cx) = harness(Vec::new(), cx);
    let picker = h.session.read_with(cx, |s, _| s.picker.clone());
    // The `/` palette lists them with their keys.
    cx.simulate_input("/picker");
    cx.run_until_parked();
    h.session.read_with(cx, |s, cx| {
        let rows = s.input().read(cx).palette(cx);
        let files = rows
            .iter()
            .find(|row| row.name == "picker.files")
            .expect("listed");
        assert!(
            files.description.ends_with("(Ctrl+G ff)"),
            "{}",
            files.description
        );
    });
    // Running it from the palette opens the picker.
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(
        picker.read_with(cx, |p, _| p.mode()),
        Some(PickerMode::Files)
    );
    h.session
        .update_in(cx, |s, window, cx| s.command("picker.grep", window, cx));
    assert_eq!(
        picker.read_with(cx, |p, _| p.mode()),
        Some(PickerMode::Grep)
    );
    // `Alt+…` flips the options without going to the keymap or being typed.
    cx.simulate_keystrokes("alt-c alt-w alt-p");
    picker.read_with(cx, |p, cx| {
        assert!(p.options().match_case && p.options().whole_word);
        assert!(!p.options().regex);
        assert!(!p.preview_enabled());
        assert_eq!(p.query(cx), "");
    });
    assert_eq!(
        h.session.read_with(cx, |s, cx| s.input().read(cx).text(cx)),
        ""
    );
    // A match is previewed from the file, through the file system.
    cx.simulate_keystrokes("alt-p");
    cx.simulate_input("add");
    cx.run_until_parked();
    cx.executor().advance_clock(crate::picker::SEARCH_DEBOUNCE);
    cx.run_until_parked();
    cx.executor()
        .advance_clock(crate::picker::preview::PREVIEW_DEBOUNCE);
    cx.run_until_parked();
    picker.read_with(cx, |p, _| {
        let preview = p.preview().expect("previewed");
        assert_eq!(preview.path.as_str(), "src/lib.rs");
        assert_eq!(preview.lines, ["pub fn add() {}"]);
        assert!(!preview.loading);
        assert_eq!(preview.match_index(), Some(0));
    });
}

#[gpui::test]
fn vim_keys_move_in_the_focused_timeline(cx: &mut TestAppContext) {
    let (h, cx) = harness(vec![ScriptedResponse::text("uno")], cx);
    send("hola", cx);
    wait_until(&h, cx, "the turn", |s, _| !s.is_running());
    let selected = |cx: &mut VisualTestContext| {
        h.session
            .read_with(cx, |s, cx| s.timeline().read(cx).state().selected())
    };
    let prompt =
        |cx: &mut VisualTestContext| h.session.read_with(cx, |s, cx| s.input().read(cx).text(cx));
    cx.simulate_keystrokes("ctrl-g w");
    let last = selected(cx);
    assert!(last.is_some());
    cx.simulate_keystrokes("k");
    assert_ne!(selected(cx), last, "k moves up");
    cx.simulate_keystrokes("shift-g");
    assert_eq!(selected(cx), last, "G goes to the last block");
    cx.simulate_keystrokes("g");
    assert_eq!(selected(cx), Some(0), "g goes to the first block");
    cx.simulate_keystrokes("j y");
    assert!(cx.read_from_clipboard().is_some(), "y copies the block");
    assert_eq!(prompt(cx), "", "vim keys are not typed");
    // Any other letter is still typed into the prompt.
    cx.simulate_keystrokes("x");
    assert_eq!(prompt(cx), "x");
}

#[gpui::test]
fn a_bang_line_is_the_shell_command(cx: &mut TestAppContext) {
    let (h, cx) = harness(Vec::new(), cx);
    let notice = |cx: &mut VisualTestContext| {
        h.session.read_with(cx, |s, cx| {
            let now = cx.background_executor().now();
            s.status()
                .read(cx)
                .state()
                .notice(now)
                .map(|n| n.message.clone())
        })
    };
    send("/!git status -s", cx);
    cx.run_until_parked();
    assert_eq!(notice(cx).as_deref(), Some("ran [git status -s]"));
    send("/! ls", cx);
    cx.run_until_parked();
    assert_eq!(notice(cx).as_deref(), Some("ran [ls]"));
}

#[gpui::test]
fn the_palette_lists_the_commands(cx: &mut TestAppContext) {
    let (h, cx) = harness(Vec::new(), cx);
    cx.simulate_input("/mo");
    h.session.read_with(cx, |s, cx| {
        let input = s.input().read(cx);
        let names: Vec<_> = input.palette(cx).iter().map(|r| r.name.clone()).collect();
        assert_eq!(names, ["model", "mode", "modal"]);
    });
}

#[gpui::test]
fn the_palette_hides_on_blur_and_on_a_click_outside(cx: &mut TestAppContext) {
    let (h, cx) = harness(Vec::new(), cx);
    // Focus events are only sent to the active window.
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();
    let shown = |cx: &mut VisualTestContext| {
        h.session
            .read_with(cx, |s, cx| !s.input().read(cx).palette(cx).is_empty())
    };
    cx.simulate_input("/mo");
    assert!(shown(cx));
    h.session.update_in(cx, |s, window, cx| {
        window.focus(&s.timeline().focus_handle(cx), cx);
    });
    cx.run_until_parked();
    assert!(!shown(cx), "the prompt lost the focus");
    cx.simulate_keystrokes("escape");
    cx.simulate_input("d");
    assert!(shown(cx), "typing shows it again");
    // A click on the timeline.
    cx.simulate_click(
        gpui::point(gpui::px(20.), gpui::px(20.)),
        gpui::Modifiers::none(),
    );
    assert!(!shown(cx), "a click outside hides it");
}

#[gpui::test]
fn help_command_opens_a_section_or_a_search(cx: &mut TestAppContext) {
    let (h, cx) = harness(Vec::new(), cx);
    cx.simulate_input("/help models");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let view = help_view(&h, cx).expect("/help opens the help");
    view.read_with(cx, |v, _| assert_eq!(v.section(), Section::Models));
    cx.simulate_keystrokes("escape");
    h.session
        .update_in(cx, |s, window, cx| s.command("help clear", window, cx));
    cx.run_until_parked();
    let view = help_view(&h, cx).expect("a search opens it too");
    view.read_with(cx, |v, cx| {
        assert_eq!(v.query(cx), "clear");
        assert_eq!(v.section(), Section::Commands);
        let rows = v.rows();
        assert!(rows.iter().any(|r| r.copy == "/clear"), "{rows:?}");
    });
}

#[gpui::test]
fn an_unknown_command_points_to_the_help(cx: &mut TestAppContext) {
    let (h, cx) = harness(Vec::new(), cx);
    cx.simulate_input("/frobnicate");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    h.session.read_with(cx, |s, cx| {
        let state = s.status().read(cx).state();
        let notice = state.notice(Instant::now()).map(|n| n.message.clone());
        assert_eq!(notice, Some(unknown_command_hint("frobnicate")));
    });
}

#[test]
fn file_references_name_the_line_of_a_match() {
    let path = WorkspacePath::parse("src/lib.rs").unwrap();
    assert_eq!(file_reference(&path, None), "@src/lib.rs ");
    assert_eq!(file_reference(&path, Some(3)), "@src/lib.rs:3 ");
    let spaced = WorkspacePath::parse("My Docs/a b.rs").unwrap();
    assert_eq!(file_reference(&spaced, Some(3)), "@\"My Docs/a b.rs\":3 ");
}

fn paste_png(h: &Harness, cx: &mut VisualTestContext) {
    let png = crate::input::attachments::tests::encoded(2, 2, image::ImageFormat::Png);
    let input = h.session.read_with(cx, |s, _| s.input().clone());
    input.update(cx, |input, cx| {
        input.attach_images(
            vec![gpui::Image::from_bytes(gpui::ImageFormat::Png, png)],
            cx,
        );
    });
    cx.run_until_parked();
}

fn image_blocks(h: &Harness, cx: &mut VisualTestContext) -> usize {
    blocks(h, cx)
        .iter()
        .filter(|kind| matches!(kind, BlockKind::Image { .. }))
        .count()
}

#[gpui::test]
fn chat_only_images_become_blocks_without_calling_the_model(cx: &mut TestAppContext) {
    let (h, cx) = harness(vec![], cx);
    paste_png(&h, cx);
    cx.simulate_keystrokes("ctrl-g i");
    let input = h.session.read_with(cx, |s, _| s.input().clone());
    input.read_with(cx, |input, _| {
        assert_eq!(input.image_mode(), ImageMode::ChatOnly);
    });
    send("the bug", cx);
    wait_until(&h, cx, "the image block", |s, cx| {
        s.timeline()
            .read(cx)
            .state()
            .entries()
            .iter()
            .any(|e| matches!(e.block.kind, BlockKind::Image { .. }))
    });
    assert!(h.llm.requests().is_empty(), "the model was called");
    assert_eq!(image_blocks(&h, cx), 1);
    assert!(
        blocks(&h, cx)
            .iter()
            .filter_map(text_of)
            .any(|t| t == "> the bug\n")
    );
    input.read_with(cx, |input, _| {
        assert!(input.images().is_empty());
        assert_eq!(input.image_mode(), ImageMode::Model);
    });
}

#[gpui::test]
fn model_images_go_with_the_prompt(cx: &mut TestAppContext) {
    let (h, cx) = harness(vec![ScriptedResponse::text("a red square")], cx);
    paste_png(&h, cx);
    send("what is it?", cx);
    wait_until(&h, cx, "the answer", |s, _| !s.is_busy());
    let request = h.llm.last_request().unwrap();
    let content = &request.messages[0].content;
    assert!(
        matches!(&content[0], ContentPart::Text { text, .. } if text == "what is it?"),
        "{content:?}"
    );
    assert!(matches!(
        &content[1],
        ContentPart::Image { media_type, .. } if media_type == "image/png"
    ));
}

fn prompt_of(h: &Harness, cx: &mut VisualTestContext) -> String {
    h.session.read_with(cx, |s, cx| s.input().read(cx).text(cx))
}

#[gpui::test]
fn the_leader_opens_the_theme_mode_and_outline_pickers(cx: &mut TestAppContext) {
    let (h, cx) = harness(vec![ScriptedResponse::text("uno")], cx);
    send("hola", cx);
    wait_until(&h, cx, "the turn", |s, _| !s.is_running());
    cx.simulate_keystrokes("ctrl-g shift-t");
    assert_eq!(prompt_of(&h, cx), "/theme ");
    cx.simulate_keystrokes("escape");
    h.session.update_in(cx, |s, window, cx| {
        s.input()
            .update(cx, |input, cx| input.set_text("", window, cx));
    });
    cx.simulate_keystrokes("ctrl-g shift-p");
    assert_eq!(prompt_of(&h, cx), "/mode ");
    let rows: Vec<String> = h.session.read_with(cx, |s, cx| {
        s.input()
            .read(cx)
            .palette(cx)
            .iter()
            .map(|row| row.name.clone())
            .collect()
    });
    assert_eq!(rows, ["mode ask", "mode auto-edit", "mode plan"]);
    // Enter on the chosen row sets that mode.
    cx.simulate_keystrokes("down enter");
    h.session.read_with(cx, |s, _| {
        assert_eq!(s.controller.permission_mode(), PermissionMode::AutoEdit);
    });
    let timeline = h.session.read_with(cx, |s, _| s.timeline().clone());
    cx.simulate_keystrokes("ctrl-g shift-o");
    assert!(timeline.read_with(cx, |t, _| t.outline_open()), "opened");
    cx.simulate_keystrokes("ctrl-g shift-o");
    assert!(!timeline.read_with(cx, |t, _| t.outline_open()), "closed");
}

#[gpui::test]
fn chat_keys_reach_the_window_and_focus_next_goes_through_the_chats(cx: &mut TestAppContext) {
    let (h, cx) = harness(Vec::new(), cx);
    let chats = |h: &Harness| -> Vec<crate::chats::ChatsCommand> {
        h.reopens
            .borrow()
            .iter()
            .filter_map(|event| match event {
                SessionEvent::Chats(command) => Some(*command),
                _ => None,
            })
            .collect()
    };
    // The harness maps `<leader>x` to a Lua function: chat.delete is
    // left out here.
    cx.simulate_keystrokes("ctrl-g c ctrl-g n ctrl-g b ctrl-g , ctrl-g s");
    use crate::chats::ChatsCommand as C;
    assert_eq!(
        chats(&h),
        [C::New, C::Next, C::Prev, C::Rename, C::TogglePanel]
    );
    // Without the sidebar the cycle skips it.
    cx.simulate_keystrokes("ctrl-tab ctrl-tab");
    assert_eq!(chats(&h).len(), 5);
    h.session.update(cx, |s, _| s.set_chats_shown(true));
    cx.simulate_keystrokes("ctrl-tab ctrl-tab");
    assert_eq!(chats(&h).last(), Some(&C::Focus), "the timeline hands over");
}

#[gpui::test]
fn quitting_during_a_turn_asks_twice(cx: &mut TestAppContext) {
    let (h, cx) = harness(Vec::new(), cx);
    let quits = Rc::new(std::cell::Cell::new(0));
    let counter = quits.clone();
    cx.update(|_, cx| {
        cx.on_action(move |_: &crate::app::Quit, _| counter.set(counter.get() + 1));
    });
    cx.simulate_keystrokes("ctrl-q");
    assert_eq!(quits.get(), 1, "idle: quits at once");
    // A turn that never ends by itself.
    let (_tx, rx) = oneshot::channel::<()>();
    h.session.update(cx, |s, cx| {
        s.running = Some(CancelToken::new());
        s.turn = Some(cx.spawn(async move |_, _| {
            rx.await.ok();
        }));
    });
    cx.simulate_keystrokes("ctrl-q");
    assert_eq!(quits.get(), 1, "the first ask only warns");
    assert!(h.session.read_with(cx, |s, _| s.quit_pending()));
    cx.simulate_keystrokes("ctrl-q");
    assert_eq!(quits.get(), 2, "the second quits");
    // Over the help modal too.
    cx.simulate_keystrokes("f1");
    cx.run_until_parked();
    h.session.update(cx, |s, _| s.running = None);
    cx.simulate_keystrokes("ctrl-q");
    assert_eq!(quits.get(), 3);
}

#[gpui::test]
fn the_leader_enter_gives_the_keys_to_the_block_widget_and_esc_back(cx: &mut TestAppContext) {
    use domain::devops::flame::{FlameTree, ProfileFormat};
    let mut tree = FlameTree::new();
    tree.add(["main", "parse"], 30);
    let spec = tree.finish(ProfileFormat::Folded, "samples");
    let block = Block::new(BlockKind::FlameGraph(Box::new(spec.clone())));
    let id = block.id;
    let (h, cx) = harness_with(Vec::new(), vec![block], cx);
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    let flame = cx.update(|window, cx| {
        crate::blocks::flame::view(id, &spec, crate::timeline::NO_REV, window, cx)
    });
    let focused = |cx: &mut VisualTestContext| {
        cx.update(|window, cx| {
            let s = h.session.read(cx);
            (
                flame.read(cx).focus_handle(cx).is_focused(window),
                s.timeline().focus_handle(cx).is_focused(window),
                s.input().focus_handle(cx).contains_focused(window, cx),
            )
        })
    };
    cx.simulate_keystrokes("ctrl-g enter");
    assert_eq!(focused(cx), (true, false, false), "the widget has the keys");
    cx.simulate_keystrokes("escape");
    assert_eq!(
        focused(cx),
        (false, true, false),
        "Esc: back to the timeline"
    );
    cx.simulate_keystrokes("escape");
    assert_eq!(focused(cx), (false, false, true), "then to the prompt");
    // A block without keys of its own says so.
    h.session.update(cx, |s, cx| {
        s.dispatch(
            DomainEvent::BlockAdded {
                block: Block::new(BlockKind::Markdown {
                    title: None,
                    source: "plain".into(),
                }),
            },
            cx,
        );
    });
    cx.simulate_keystrokes("ctrl-tab end ctrl-g enter");
    assert_eq!(focused(cx), (false, true, false), "the timeline keeps it");
}

#[test]
fn the_new_actions_have_descriptions_in_the_help() {
    let rows = crate::help::key_rows(&default_keymap(), |_| None);
    for action in [
        "turn.interrupt",
        "queue.edit",
        "app.quit",
        "window.fullscreen",
        "chat.new",
        "block.focus_widget",
        "permissions.open",
    ] {
        let row = rows.iter().find(|r| r.action == action).unwrap();
        assert_ne!(row.description, "unknown builtin action", "{action}");
    }
}

fn permissions_view(h: &Harness, cx: &mut VisualTestContext) -> Option<Entity<PermissionsView>> {
    h.session.read_with(cx, |s, cx| s.permissions_view(cx))
}

fn rule_texts(view: &Entity<PermissionsView>, cx: &mut VisualTestContext) -> Vec<String> {
    view.read_with(cx, |v, _| {
        v.rows().iter().map(|row| row.text.clone()).collect()
    })
}

fn rule(text: &str) -> PermissionRule {
    text.parse().unwrap()
}

#[gpui::test]
fn leader_a_lists_the_rules_and_d_and_p_change_them(cx: &mut TestAppContext) {
    use domain::permission::RuleScope;
    use std::path::Path;

    use crate::permissions::RuleGroup;

    let (h, cx) = harness(Vec::new(), cx);
    let ws = Path::new("/ws");
    h.rules_file.set_text(
        ws,
        "allow = [\"edit_file(src/*)\"]\ndeny = [\"run_command(git push *)\"]\n",
    );
    {
        let mut policy = h.permissions.lock().unwrap();
        policy.remember(rule("run_command(cargo test *)"));
        policy.remember(rule("ls"));
        for kept in h.rules_file.rules(ws) {
            policy.remember(kept);
        }
    }
    cx.simulate_keystrokes("ctrl-g shift-a");
    cx.run_until_parked();
    let view = permissions_view(&h, cx).expect("<leader>A opens /permissions");
    assert_eq!(
        rule_texts(&view, cx),
        [
            "run_command(cargo test *)",
            "ls",
            "edit_file(src/*)",
            "run_command(git push *)"
        ]
    );
    view.read_with(cx, |v, _| {
        let groups: Vec<_> = v.rows().iter().map(|row| row.group).collect();
        assert_eq!(
            groups,
            [
                RuleGroup::Session,
                RuleGroup::Session,
                RuleGroup::Project,
                RuleGroup::Deny
            ]
        );
        assert_eq!(
            v.listing().project_file.as_deref(),
            Some(Path::new("/ws/.crest/permissions.local.toml"))
        );
        assert_eq!(v.mode(), PermissionMode::Ask);
    });
    cx.update(|window, cx| {
        assert!(view.focus_handle(cx).contains_focused(window, cx));
    });

    // `p` moves the selected session rule to the project file.
    cx.simulate_keystrokes("p");
    cx.run_until_parked();
    let promoted = rule("run_command(cargo test *)").with_scope(RuleScope::Project);
    assert!(h.rules_file.rules(ws).contains(&promoted));
    assert!(h.permissions.lock().unwrap().rules.contains(&promoted));
    assert_eq!(
        rule_texts(&view, cx),
        [
            "ls",
            "run_command(cargo test *)",
            "edit_file(src/*)",
            "run_command(git push *)"
        ],
        "it moved to the Project group, in its place in the policy"
    );

    // `d` removes a session rule at once.
    cx.simulate_keystrokes("d");
    cx.run_until_parked();
    assert!(!h.permissions.lock().unwrap().rules.contains(&rule("ls")));
    assert_eq!(rule_texts(&view, cx).len(), 3);

    // A project rule asks first; another key keeps it.
    cx.simulate_keystrokes("down d");
    cx.run_until_parked();
    assert!(view.read_with(cx, |v, _| v.confirming().is_some()));
    cx.simulate_keystrokes("n");
    cx.run_until_parked();
    assert!(view.read_with(cx, |v, _| v.confirming().is_none()));
    assert_eq!(h.rules_file.rules(ws).len(), 3);
    // A second `d` deletes it from the file and the policy.
    cx.simulate_keystrokes("d d");
    cx.run_until_parked();
    let edit = rule("edit_file(src/*)").with_scope(RuleScope::Project);
    assert!(!h.rules_file.rules(ws).contains(&edit));
    assert!(!h.permissions.lock().unwrap().rules.contains(&edit));
    assert_eq!(
        rule_texts(&view, cx),
        ["run_command(cargo test *)", "run_command(git push *)"]
    );
    let status = view.read_with(cx, |v, _| v.status().map(str::to_owned));
    assert_eq!(
        status.as_deref(),
        Some("removed `edit_file(src/*)` from the project file")
    );

    // The filter box.
    cx.simulate_keystrokes("/");
    cx.simulate_input("push");
    cx.run_until_parked();
    assert_eq!(rule_texts(&view, cx), ["run_command(git push *)"]);
    cx.simulate_keystrokes("enter");
    cx.update(|window, cx| assert!(!view.read(cx).is_filtering(window, cx)));

    // `m` cycles the mode, for the status bar too.
    cx.simulate_keystrokes("m");
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |v, _| v.mode()),
        PermissionMode::AutoEdit
    );
    assert_eq!(
        h.session
            .read_with(cx, |s, _| s.controller.permission_mode()),
        PermissionMode::AutoEdit
    );

    cx.simulate_keystrokes("escape");
    assert!(permissions_view(&h, cx).is_none(), "Esc closes it");
    cx.update(|window, cx| {
        let input = h.session.read(cx).input().focus_handle(cx);
        assert!(input.contains_focused(window, cx), "the focus is back");
    });
}

#[gpui::test]
fn the_permissions_selection_stays_in_view(cx: &mut TestAppContext) {
    let (h, cx) = harness(Vec::new(), cx);
    {
        let mut policy = h.permissions.lock().unwrap();
        for n in 0..60 {
            policy.remember(rule(&format!("run_command(tool{n} *)")));
        }
    }
    h.session
        .update_in(cx, |s, window, cx| s.command("permissions", window, cx));
    cx.run_until_parked();
    let view = permissions_view(&h, cx).expect("open");
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    cx.simulate_keystrokes("end");
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    view.read_with(cx, |v, _| {
        assert_eq!(v.selected(), 59);
        assert!(crate::keyscroll::is_visible(v.scroll_handle(), 59));
        assert!(!crate::keyscroll::is_visible(v.scroll_handle(), 0));
    });
}

#[gpui::test]
fn slash_permissions_without_rules_says_how_to_add_one(cx: &mut TestAppContext) {
    let (h, cx) = harness(Vec::new(), cx);
    cx.simulate_input("/permissions");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let view = permissions_view(&h, cx).expect("/permissions opens it");
    assert!(rule_texts(&view, cx).is_empty());
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    assert!(cx.debug_bounds("permissions-empty").is_some());
    // The mode line names the keys of permission.cycle.
    assert!(cx.debug_bounds("permissions-mode").is_some());
    // A file with problems is listed and not rewritten.
    h.rules_file
        .set_text(std::path::Path::new("/ws"), "allow = [\"bad(\"]\n");
    cx.simulate_keystrokes("escape");
    h.session
        .update_in(cx, |s, window, cx| s.command("permissions", window, cx));
    cx.run_until_parked();
    let view = permissions_view(&h, cx).expect("open again");
    assert_eq!(view.read_with(cx, |v, _| v.listing().warnings.len()), 1);
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    assert!(cx.debug_bounds("permissions-warnings").is_some());
}

#[gpui::test]
fn the_leader_shows_what_can_follow_until_a_key_esc_or_the_timeout(cx: &mut TestAppContext) {
    let (h, cx) = harness(Vec::new(), cx);
    let which_key = |cx: &mut VisualTestContext| {
        h.session
            .read_with(cx, |s, cx| s.input().read(cx).which_key())
    };
    let drawn = |cx: &mut VisualTestContext| {
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        cx.debug_bounds("which-key").is_some()
    };
    assert!(which_key(cx).is_empty());
    assert!(!drawn(cx));
    cx.simulate_keystrokes("ctrl-g");
    let next = which_key(cx);
    let what = |key: &str| {
        next.iter()
            .find(|n| n.key == key)
            .map(|n| n.what.clone())
            .unwrap_or_default()
    };
    assert_eq!(what("h"), "Open the help");
    assert_eq!(what("f"), "+2 more", "ff and fg start with f");
    assert_eq!(what("x"), "Lua function", "the harness maps <leader>x");
    assert!(drawn(cx), "the popup shows");
    // The next key of a longer mapping narrows it down.
    cx.simulate_keystrokes("f");
    let next = which_key(cx);
    let keys: Vec<&str> = next.iter().map(|n| n.key.as_str()).collect();
    assert_eq!(keys, ["f", "g"]);
    // Esc settles the prefix: the popup goes.
    cx.simulate_keystrokes("escape");
    assert!(which_key(cx).is_empty());
    assert!(!drawn(cx));
    // So does a pause.
    cx.simulate_keystrokes("ctrl-g");
    assert!(drawn(cx));
    cx.executor().advance_clock(Duration::from_millis(1100));
    cx.run_until_parked();
    assert!(which_key(cx).is_empty());
    assert!(!drawn(cx));
    // And a key that completes a mapping.
    cx.simulate_keystrokes("ctrl-g p");
    assert!(which_key(cx).is_empty());
}

#[gpui::test]
fn the_help_keys_filter_and_copy_a_remap_snippet(cx: &mut TestAppContext) {
    let (h, cx) = harness(Vec::new(), cx);
    cx.simulate_keystrokes("f1");
    cx.run_until_parked();
    let view = help_view(&h, cx).expect("the help is open");
    cx.simulate_keystrokes("2 /");
    cx.simulate_input("picker.grep");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    view.read_with(cx, |v, _| {
        assert_eq!(v.section(), Section::Keys);
        let rows = v.rows();
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].tags, ["picker.grep", "default"]);
    });
    cx.simulate_keystrokes("c");
    let copied = cx
        .read_from_clipboard()
        .and_then(|item| item.text())
        .unwrap_or_default();
    assert!(
        copied.contains("app.keymap.set(\"<leader>fg\", \"picker.grep\")"),
        "{copied}"
    );
    assert!(
        copied.contains("[[bind]]\nkeys = \"<leader>fg\"\naction = \"picker.grep\""),
        "{copied}"
    );
    view.read_with(cx, |v, _| {
        assert!(v.status().is_some_and(|s| s.contains("remap snippet")));
    });
    // The source is searchable; a mapping of the config says so.
    cx.update(|window, cx| {
        view.update(cx, |v, cx| v.set_query("plugin x", window, cx));
    });
    view.read_with(cx, |v, _| {
        assert!(v.rows().is_empty(), "the harness maps no plugin keys");
    });
    cx.update(|window, cx| {
        view.update(cx, |v, cx| v.set_query("global lua", window, cx));
    });
    view.read_with(cx, |v, _| {
        let rows = v.rows();
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].primary, "<C-g>x");
    });
    // A fixed key of a panel has no snippet.
    cx.write_to_clipboard(gpui::ClipboardItem::new_string("before".into()));
    cx.update(|window, cx| {
        view.update(cx, |v, cx| v.set_query("Expand the row", window, cx));
    });
    view.read_with(cx, |v, _| {
        let rows = v.rows();
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].tags, ["Help", "fixed"]);
    });
    cx.simulate_keystrokes("c");
    let clipboard = cx.read_from_clipboard().and_then(|item| item.text());
    assert_eq!(clipboard.as_deref(), Some("before"));
    view.read_with(cx, |v, _| {
        assert!(v.status().is_some_and(|s| s.contains("cannot be remapped")));
    });
}

#[gpui::test]
fn ctrl_c_in_the_picker_copies_its_selection_without_interrupting(cx: &mut TestAppContext) {
    let (h, cx) = harness(vec![ScriptedResponse::text("uno").hang_after(2)], cx);
    send("primero", cx);
    let llm = h.llm.clone();
    wait_until(&h, cx, "the turn to reach the model", move |_, _| {
        llm.remaining() == 0
    });
    assert!(h.session.read_with(cx, |s, _| s.is_running()));
    cx.simulate_keystrokes("ctrl-g f f");
    let picker = h.session.read_with(cx, |s, _| s.picker.clone());
    assert!(picker.read_with(cx, |p, _| p.is_open()));
    cx.simulate_input("abc");
    cx.simulate_keystrokes("shift-left shift-left");
    assert_eq!(
        picker.read_with(cx, |p, cx| p.selected_text(cx)).as_deref(),
        Some("bc")
    );
    cx.simulate_keystrokes("ctrl-c");
    cx.run_until_parked();
    let copied = cx.read_from_clipboard().and_then(|item| item.text());
    assert_eq!(copied.as_deref(), Some("bc"));
    assert!(
        h.session.read_with(cx, |s, _| s.is_running()),
        "copying does not interrupt the turn"
    );
    assert!(picker.read_with(cx, |p, _| p.is_open()));
    // Without a selection, Ctrl+C interrupts as before.
    cx.simulate_keystrokes("right ctrl-c");
    wait_until(&h, cx, "the interrupted turn", |s, _| !s.is_running());
}

#[gpui::test]
fn the_welcome_screen_names_the_keys_of_the_keymap(cx: &mut TestAppContext) {
    use application::keymap::Platform;
    use application::keymap::layers::build_keymap;
    use application::ports::plugin::{KeyAction, KeymapSpec};
    let unbind = |lhs: &str| KeymapSpec {
        lhs: lhs.into(),
        action: KeyAction::Unmap,
        description: None,
        plugin: "config".into(),
    };
    let specs = [
        unbind("<C-Tab>"),
        KeymapSpec {
            lhs: "<F8>".into(),
            action: KeyAction::Builtin("focus.next".into()),
            description: None,
            plugin: "config".into(),
        },
        unbind("<leader>m"),
    ];
    let (keymap, warnings) = build_keymap(&specs, Some("<C-Space>"), Platform::current());
    assert!(warnings.is_empty(), "{warnings:?}");
    let (h, cx) = harness_keymap(keymap, cx);
    let welcome = h
        .session
        .read_with(cx, |s, cx| s.timeline().read(cx).welcome_keys().to_vec());
    let row = |keys: &str| {
        welcome
            .iter()
            .find(|(k, _)| k == keys)
            .map(|(_, what)| what.clone())
    };
    let shortcuts = row("Ctrl+Space").expect("the leader row");
    assert_eq!(
        shortcuts, "shortcuts: e tree · ff files · fg grep · j/k blocks · h help",
        "no model: its key was removed"
    );
    assert!(row("Ctrl+G").is_none(), "{welcome:?}");
    assert!(row("Ctrl+Tab").is_none(), "{welcome:?}");
    assert!(row("F8").is_some_and(|what| what.starts_with("move the focus")));
    assert!(row("Ctrl+Shift+C").is_some(), "{welcome:?}");
}
