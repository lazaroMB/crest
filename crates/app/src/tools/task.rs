//! `task`: hands a broad search to a read-only subagent and returns its
//! report.
//!
//! The subagent is a child [`AgentService`] that runs one turn in a fresh,
//! ephemeral session with the `explore` model role pinned. It cannot write
//! or run commands, by construction and by policy:
//!
//! - its registry holds only the read tools (`list_files`, `find_file`,
//!   `grep`, `tree`, `read_file`, and `web_search` / `web_fetch` when the
//!   web is configured); `edit_file`, `write_file`,
//!   `run_command` and `task` itself (no recursion) are not there, and
//!   [`TaskPorts`] has no shell to build them from;
//! - its permissions are [`PermissionPolicy::read_only`], which denies any
//!   call that is not a read whatever the mode;
//! - its approval port denies everything, so nothing can reach the user.
//!
//! The subagent's events go to a sink that drops them: its streaming text
//! and tool calls would flood the parent's timeline, and the parent already
//! shows the `task` call with its description and, when it ends, the
//! report. Model calls and tool calls still go to telemetry.
//!
//! Dropping the [`PreparedCall::run`] future (the parent turn was
//! cancelled) drops the subagent's turn, which closes its model stream and
//! cancels its running tool.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use domain::conversation::{ContentPart, Role, Session, StopReason};
use domain::model::{ModelPolicy, ModelRole};
use domain::permission::{PermissionPolicy, ToolEffect};
use futures::FutureExt;
use schemars::JsonSchema;
use serde::Deserialize;

use super::budget::ResultBudget;
use super::files::{FindFileTool, GrepTool, ListFilesTool, ReadFileTool, TreeTool};
use super::registry::{
    PreparedCall, RegisterError, ToolContext, ToolError, ToolOutput, ToolRegistry, TypedTool,
};
use super::web::WebTools;
use crate::ports::approval::{ApprovalError, ApprovalPort, ApprovalRequest, ApprovalResponse};
use crate::ports::events::{DomainEvent, EventSink};
use crate::ports::fs::FileSystemPort;
use crate::ports::llm::LlmPort;
use crate::ports::search::{ContentSearchPort, FileSearchPort};
use crate::ports::telemetry::TelemetryPort;
use crate::usecases::run_turn::{AgentDeps, AgentService, TurnOptions};

/// Default limit of model calls of one subagent run.
pub const DEFAULT_SUBAGENT_MAX_ITERATIONS: u32 = 25;

/// System prompt of the explore subagent.
pub const SUBAGENT_SYSTEM_PROMPT: &str = "\
You are an explore subagent of crest, a coding assistant. Another agent gave you a search or \
reading task, in the workspace or on the web. You can only read: list_files, find_file, grep, \
tree and read_file, plus web_search and web_fetch when they are offered. You cannot edit, \
write or run commands, and you cannot ask the user anything.
- Search broadly first (grep, find_file, tree), then read only the parts that matter.
- For the web: search, read the 2-3 best results with web_fetch, search again with other words \
if they fall short, and give the URL of every source.
- Stop as soon as you can answer.
- Answer with a concise report for the other agent: the findings, each with its workspace path \
and line numbers (e.g. `src/lib.rs:42`), and short code excerpts only when the exact text \
matters. Say what you did not find. Do not repeat the task.";

const TASK_DESCRIPTION: &str = "\
Launches a read-only subagent to explore the workspace and returns its report.
- Use it for broad searches that would take many list/grep/read calls (\"where is X handled\", \
\"how does Y work across modules\"), or for a piece of web research (\"compare the options \
for Z\"), to keep their output out of this conversation. Several tasks can research \
different questions in parallel.
- The subagent only has list_files, find_file, grep, tree and read_file, plus web_search and \
web_fetch when the web is configured: it cannot edit, write or run commands. Do the changes \
yourself after reading its report.
- It does not see this conversation: put everything it needs in `prompt` (what to find, where \
to look, what to report).
- Its report lists paths and line numbers; read the files yourself before editing them.";

/// Input of `task`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct TaskInput {
    /// Short title of the task, 3 to 5 words (e.g. "Find permission checks").
    pub description: String,
    /// Full instructions for the subagent: what to find or read and what to
    /// report. It does not see this conversation.
    pub prompt: String,
}

/// The ports the subagent runs on. There is deliberately no shell nor any
/// way to write: the subagent's tools are built from these alone.
#[derive(Clone)]
pub struct TaskPorts {
    /// The model, shared with the parent.
    pub llm: Arc<dyn LlmPort>,
    /// Telemetry, shared with the parent.
    pub telemetry: Arc<dyn TelemetryPort>,
    /// Model selection; the subagent pins the `explore` role.
    pub models: ModelPolicy,
    /// File listing and fuzzy finding.
    pub file_search: Arc<dyn FileSearchPort>,
    /// Content search.
    pub content_search: Arc<dyn ContentSearchPort>,
    /// Reading files (the subagent's tools never call its write methods).
    pub fs: Arc<dyn FileSystemPort>,
    /// The web tools, when the web is configured.
    pub web: Option<WebTools>,
}

/// `task`: runs a read-only `explore` subagent and returns its report.
pub struct TaskTool {
    ports: TaskPorts,
    tools: ToolRegistry,
    budget: ResultBudget,
    max_iterations: u32,
}

impl std::fmt::Debug for TaskTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TaskTool")
            .field("tools", &self.tools)
            .field("max_iterations", &self.max_iterations)
            .finish_non_exhaustive()
    }
}

impl TaskTool {
    /// A `task` tool whose subagent cuts its tool results, and whose report
    /// is cut, to `budget`.
    ///
    /// # Errors
    ///
    /// [`RegisterError`] if the read tools cannot be registered (it cannot
    /// happen with the fixed set of names).
    pub fn new(ports: TaskPorts, budget: ResultBudget) -> Result<Self, RegisterError> {
        let tools = read_only_tools(&ports, budget)?;
        Ok(Self {
            ports,
            tools,
            budget,
            max_iterations: DEFAULT_SUBAGENT_MAX_ITERATIONS,
        })
    }

    /// Sets the limit of model calls of one subagent run.
    #[must_use]
    pub fn with_max_iterations(mut self, limit: u32) -> Self {
        self.max_iterations = limit.max(1);
        self
    }

    /// The tools the subagent can call.
    #[must_use]
    pub fn subagent_tools(&self) -> &ToolRegistry {
        &self.tools
    }

    fn subagent(&self) -> AgentService {
        let deps = AgentDeps {
            llm: Arc::clone(&self.ports.llm),
            events: Arc::new(DiscardEvents),
            approval: Arc::new(DenyAll),
            telemetry: Arc::clone(&self.ports.telemetry),
        };
        let permissions = Arc::new(Mutex::new(PermissionPolicy::read_only()));
        AgentService::new(
            deps,
            self.tools.clone(),
            self.ports.models.clone(),
            permissions,
        )
        .with_system_prompt(SUBAGENT_SYSTEM_PROMPT)
        .with_max_iterations(self.max_iterations)
    }
}

/// The subagent's registry: the five read tools and the web tools.
fn read_only_tools(ports: &TaskPorts, budget: ResultBudget) -> Result<ToolRegistry, RegisterError> {
    let mut tools = ToolRegistry::new().with_budget(budget);
    let search = &ports.file_search;
    tools.register_typed(ListFilesTool::new(Arc::clone(search), budget))?;
    tools.register_typed(FindFileTool::new(Arc::clone(search), budget))?;
    tools.register_typed(TreeTool::new(Arc::clone(search), budget))?;
    tools.register_typed(GrepTool::new(Arc::clone(&ports.content_search), budget))?;
    tools.register_typed(ReadFileTool::new(Arc::clone(&ports.fs), budget))?;
    if let Some(web) = &ports.web {
        web.register(&mut tools)?;
    }
    Ok(tools)
}

#[async_trait]
impl TypedTool for TaskTool {
    type Input = TaskInput;

    fn name(&self) -> &str {
        "task"
    }

    fn description(&self) -> &str {
        TASK_DESCRIPTION
    }

    fn effect(&self, _input: &TaskInput) -> ToolEffect {
        ToolEffect::Read
    }

    async fn prepare(
        &self,
        input: TaskInput,
        _ctx: &ToolContext,
    ) -> Result<PreparedCall, ToolError> {
        if input.prompt.trim().is_empty() {
            return Err(ToolError::InvalidInput {
                tool: "task".to_owned(),
                message: "`prompt` is empty; describe what the subagent must find".to_owned(),
            });
        }
        let subject = input.description.trim().to_owned();
        let service = self.subagent();
        let root = self.ports.fs.root().as_path().display().to_string();
        let budget = self.budget;
        let run = async move {
            let mut session = Session::new(root, 0);
            let outcome = service
                .run_turn(
                    &mut session,
                    input.prompt,
                    TurnOptions {
                        pinned_role: Some(ModelRole::Explore),
                        ..TurnOptions::new(ModelRole::Main)
                    },
                )
                .await;
            let report = final_text(&session);
            match outcome {
                Ok(outcome) => Ok(ToolOutput::text(
                    budget.truncate_text(&with_note(report, outcome.stop_reason)),
                )),
                Err(err) => Err(ToolError::Execution(format!("the subagent failed: {err}"))),
            }
        };
        Ok(PreparedCall::new(subject, run.boxed()))
    }
}

/// Text of the last assistant message of the session's last turn.
fn final_text(session: &Session) -> String {
    session
        .turns
        .last()
        .and_then(|turn| {
            turn.messages
                .iter()
                .rev()
                .find(|message| message.role == Role::Assistant)
        })
        .map(|message| {
            message
                .content
                .iter()
                .filter_map(|part| match part {
                    ContentPart::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

/// The report, with a note when it is empty or was cut short.
fn with_note(report: String, stop_reason: StopReason) -> String {
    let report = if report.trim().is_empty() {
        "[the subagent finished without a report]".to_owned()
    } else {
        report
    };
    match stop_reason {
        StopReason::MaxTokens => format!("{report}\n[the subagent's report was cut short]"),
        StopReason::Interrupted => format!("{report}\n[the subagent was interrupted]"),
        _ => report,
    }
}

/// Drops every event of the subagent (see the module docs).
struct DiscardEvents;

impl EventSink for DiscardEvents {
    fn emit(&self, _event: DomainEvent) {}
}

/// Denies every approval: a subagent never asks the user.
struct DenyAll;

#[async_trait]
impl ApprovalPort for DenyAll {
    async fn request(&self, _req: ApprovalRequest) -> Result<ApprovalResponse, ApprovalError> {
        Err(ApprovalError::Unavailable(
            "a subagent cannot ask for approval".to_owned(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use domain::conversation::{ContentPart, Role, Session, StopReason};
    use domain::permission::PermissionMode;

    use serde_json::json;

    use super::*;
    use crate::ports::llm::LlmRequest;
    use crate::testing::fs::{InMemoryFs, ScriptedShell, test_root};
    use crate::testing::llm::{ScriptedLlm, ScriptedResponse};
    use crate::testing::system::{RecordingSink, RecordingTelemetry, ScriptedApproval};
    use crate::tools::files::{FileToolPorts, register_file_tools};
    use crate::usecases::cancel::CancelToken;
    use crate::usecases::run_turn::TurnOutcome;

    const LIB: &str = "pub fn check(x: u32) -> bool {\n    x > 3\n}\n";

    /// A parent agent with every file tool and `task`, all on one fake
    /// workspace, shell and model.
    struct Parent {
        llm: Arc<ScriptedLlm>,
        fs: Arc<InMemoryFs>,
        shell: Arc<ScriptedShell>,
        sink: Arc<RecordingSink>,
        service: AgentService,
        session: Session,
    }

    impl Parent {
        fn new(script: Vec<ScriptedResponse>) -> Self {
            let llm = Arc::new(ScriptedLlm::new(script));
            let fs = Arc::new(
                InMemoryFs::new(test_root())
                    .with_file("src/lib.rs", LIB)
                    .with_file("README.md", "# demo\n"),
            );
            let shell = Arc::new(ScriptedShell::new());
            let sink = Arc::new(RecordingSink::new());
            let telemetry = Arc::new(RecordingTelemetry::new());
            let budget = ResultBudget::default();
            let mut tools = ToolRegistry::new().with_budget(budget);
            register_file_tools(
                &mut tools,
                FileToolPorts {
                    file_search: fs.clone(),
                    content_search: fs.clone(),
                    fs: fs.clone(),
                    shell: shell.clone(),
                    rewriter: None,
                },
            )
            .unwrap();
            let task = TaskTool::new(
                TaskPorts {
                    llm: llm.clone(),
                    telemetry: telemetry.clone(),
                    models: ModelPolicy::default(),
                    file_search: fs.clone(),
                    content_search: fs.clone(),
                    fs: fs.clone(),
                    web: None,
                },
                budget,
            )
            .unwrap();
            tools.register_typed(task).unwrap();
            let deps = AgentDeps {
                llm: llm.clone(),
                events: sink.clone(),
                approval: Arc::new(ScriptedApproval::always(ApprovalResponse::Approve)),
                telemetry,
            };
            // The parent approves every write and command, so only
            // the subagent's own limits can stop them.
            let permissions = Arc::new(Mutex::new(PermissionPolicy::new(PermissionMode::Ask)));
            let service = AgentService::new(deps, tools, ModelPolicy::default(), permissions);
            Self {
                llm,
                fs,
                shell,
                sink,
                service,
                session: Session::new("/ws", 0),
            }
        }

        async fn run(&mut self) -> TurnOutcome {
            self.service
                .run_turn(&mut self.session, "busca", TurnOptions::default())
                .await
                .unwrap()
        }
    }

    fn task_call() -> ScriptedResponse {
        ScriptedResponse::tool_use(
            "toolu_task",
            "task",
            json!({ "description": "Find check function", "prompt": "Where is `check` defined?" }),
        )
    }

    /// The `tool_result`s the last user message of `req` carries.
    fn results(req: &LlmRequest) -> Vec<(String, bool)> {
        let last = req.messages.last().unwrap();
        assert_eq!(last.role, Role::User);
        last.content
            .iter()
            .filter_map(|part| match part {
                ContentPart::ToolResult {
                    content, is_error, ..
                } => Some((
                    content
                        .iter()
                        .map(|c| {
                            serde_json::to_value(c).unwrap()["text"]
                                .as_str()
                                .unwrap()
                                .to_owned()
                        })
                        .collect::<Vec<_>>()
                        .join("\n"),
                    *is_error,
                )),
                _ => None,
            })
            .collect()
    }

    fn tool_names(req: &LlmRequest) -> Vec<String> {
        req.tools.iter().map(|tool| tool.name.clone()).collect()
    }

    #[tokio::test]
    async fn subagent_report_comes_back_as_the_tool_result() {
        let mut parent = Parent::new(vec![
            task_call(),
            ScriptedResponse::tool_use("toolu_g", "grep", json!({ "pattern": "fn check" })),
            ScriptedResponse::tool_use("toolu_r", "read_file", json!({ "path": "src/lib.rs" })),
            ScriptedResponse::text("`check` is defined at src/lib.rs:1."),
            ScriptedResponse::text("Listo."),
        ]);
        let outcome = parent.run().await;
        assert_eq!(outcome.stop_reason, StopReason::EndTurn);
        assert_eq!(outcome.model_calls, 2);
        let reqs = parent.llm.requests();
        assert_eq!(reqs.len(), 5);
        // The subagent's tools ran and found the file.
        let grep = results(&reqs[2]);
        assert!(!grep[0].1 && grep[0].0.contains("src/lib.rs:1"), "{grep:?}");
        let read = results(&reqs[3]);
        assert!(!read[0].1 && read[0].0.contains("x > 3"), "{read:?}");
        // Its final text is what the parent model gets back.
        assert_eq!(
            results(&reqs[4]),
            vec![("`check` is defined at src/lib.rs:1.".to_owned(), false)]
        );
        // Its turn stays out of the parent session and timeline.
        assert_eq!(parent.session.turns.len(), 1);
        let turns_started = parent
            .sink
            .kinds()
            .iter()
            .filter(|kind| *kind == "turn_started")
            .count();
        assert_eq!(turns_started, 1);
    }

    #[tokio::test]
    async fn subagent_uses_the_explore_role_and_read_tools_only() {
        let mut parent = Parent::new(vec![
            task_call(),
            ScriptedResponse::text("nothing"),
            ScriptedResponse::text("ok"),
        ]);
        parent.run().await;
        let reqs = parent.llm.requests();
        assert_eq!(reqs[0].model.role, ModelRole::Main);
        assert!(tool_names(&reqs[0]).contains(&"task".to_owned()));
        let child = &reqs[1];
        assert_eq!(child.model.role, ModelRole::Explore);
        assert_eq!(
            child.model.spec.id,
            ModelPolicy::default()
                .resolve(&domain::model::ResolveRequest {
                    role: ModelRole::Explore,
                    turn_override: None,
                    pinned_role: None,
                })
                .spec
                .id
        );
        assert_eq!(
            tool_names(child),
            ["find_file", "grep", "list_files", "read_file", "tree"]
        );
        assert_eq!(child.system[0].text, SUBAGENT_SYSTEM_PROMPT);
        assert_eq!(child.messages.len(), 1);
    }

    #[tokio::test]
    async fn subagent_cannot_write_or_run_commands() {
        let mut parent = Parent::new(vec![
            task_call(),
            ScriptedResponse::tool_use(
                "toolu_e",
                "edit_file",
                json!({ "path": "src/lib.rs", "old": "x > 3", "new": "true" }),
            )
            .then_tool_use(
                "toolu_w",
                "write_file",
                json!({ "path": "evil.txt", "content": "pwned" }),
            )
            .then_tool_use(
                "toolu_c",
                "run_command",
                json!({ "command": "rm", "args": ["-rf", "/"] }),
            ),
            ScriptedResponse::text("I could not change anything."),
            ScriptedResponse::text("ok"),
        ]);
        let before = parent.fs.snapshot();
        parent.run().await;
        let reqs = parent.llm.requests();
        let denied = results(&reqs[2]);
        assert_eq!(denied.len(), 3);
        for ((text, is_error), tool) in
            denied
                .iter()
                .zip(["edit_file", "write_file", "run_command"])
        {
            assert!(*is_error, "{tool}: {text}");
            assert_eq!(text, &format!("unknown tool `{tool}`"));
        }
        assert_eq!(parent.fs.snapshot(), before);
        assert!(parent.shell.calls().is_empty());
        assert_eq!(
            results(&reqs[3]),
            vec![("I could not change anything.".to_owned(), false)]
        );
    }

    #[tokio::test]
    async fn subagent_cannot_launch_another_subagent() {
        let mut parent = Parent::new(vec![
            task_call(),
            ScriptedResponse::tool_use(
                "toolu_task2",
                "task",
                json!({ "description": "Recurse", "prompt": "again" }),
            ),
            ScriptedResponse::text("done"),
            ScriptedResponse::text("ok"),
        ]);
        parent.run().await;
        let reqs = parent.llm.requests();
        assert_eq!(reqs.len(), 4, "only one subagent ran");
        assert_eq!(
            results(&reqs[2]),
            vec![("unknown tool `task`".to_owned(), true)]
        );
        assert!(!tool_names(&reqs[1]).contains(&"task".to_owned()));
    }

    #[tokio::test]
    async fn empty_prompt_is_rejected_without_a_subagent() {
        let mut parent = Parent::new(vec![
            ScriptedResponse::tool_use(
                "toolu_task",
                "task",
                json!({ "description": "Nothing", "prompt": "  " }),
            ),
            ScriptedResponse::text("ok"),
        ]);
        parent.run().await;
        let reqs = parent.llm.requests();
        assert_eq!(reqs.len(), 2);
        let result = results(&reqs[1]);
        assert!(
            result[0].1 && result[0].0.contains("`prompt` is empty"),
            "{result:?}"
        );
    }

    #[test]
    fn effect_is_read_and_the_input_needs_description_and_prompt() {
        let parent = Parent::new(vec![]);
        let task = parent.service.tools().get("task").unwrap();
        assert_eq!(
            task.effect(&json!({ "description": "a", "prompt": "b" })),
            ToolEffect::Read
        );
        assert_eq!(
            task.input_schema()["required"],
            json!(["description", "prompt"])
        );
    }

    #[tokio::test]
    async fn cancelling_the_parent_cancels_the_subagent() {
        use futures::FutureExt as _;

        let mut parent = Parent::new(vec![
            task_call(),
            ScriptedResponse::text("still searching").hang_after(1),
        ]);
        let token = CancelToken::new();
        let opts = TurnOptions::default().with_cancel(token.clone());
        let llm = parent.llm.clone();
        let outcome = {
            let turn = parent.service.run_turn(&mut parent.session, "busca", opts);
            let mut turn = std::pin::pin!(turn);
            // Drive the turn until the subagent's stream hangs, then cancel.
            loop {
                futures::select_biased! {
                    outcome = turn.as_mut().fuse() => panic!("finished early: {outcome:?}"),
                    () = tokio::task::yield_now().fuse() => {}
                }
                if llm.requests().len() == 2 && llm.open_streams() == 1 {
                    break;
                }
            }
            token.cancel();
            turn.await.unwrap()
        };
        assert_eq!(outcome.stop_reason, StopReason::Interrupted);
        assert_eq!(llm.open_streams(), 0, "the subagent's stream was dropped");
        assert_eq!(llm.requests().len(), 2);
        let last = parent.session.history().last().unwrap();
        assert!(matches!(
            &last.content[..],
            [ContentPart::ToolResult { is_error: true, .. }]
        ));
    }
}
