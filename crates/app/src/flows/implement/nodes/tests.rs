//! Shared fixtures of the step tests: a loaded plan and an agent over
//! `ScriptedLlm`, `InMemoryFs` and a shell.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use domain::model::ModelPolicy;
use domain::permission::{PermissionMode, PermissionPolicy};

use crate::flows::implement::load::parse_plan;
use crate::flows::implement::load::tests::plan_json;
use crate::flows::implement::node::{AgentKit, ImplementDeps, StepContext};
use crate::flows::implement::state::{ImplementState, Step};
use crate::ports::approval::ApprovalResponse;
use crate::ports::shell::{CommandOutput, RunCommandRequest, ShellError, ShellPort};
use crate::testing::fs::{InMemoryFs, ScriptedShell, test_root};
use crate::testing::llm::{ScriptedLlm, ScriptedResponse};
use crate::testing::system::{RecordingSink, RecordingTelemetry, ScriptedApproval};
use crate::tools::files::{FileToolPorts, register_file_tools};
use crate::tools::registry::ToolRegistry;
use crate::usecases::run_turn::AgentDeps;

/// The state right after Load plan: tasks `(id, depends_on)` loaded,
/// ordered and on the branch.
pub(crate) fn loaded_state(tasks: &[(&str, &[&str])]) -> ImplementState {
    let loaded = parse_plan("p", &plan_json(tasks)).unwrap();
    let mut state = ImplementState::new(".crest/specs/0007-session-cache/plan.json");
    state.plan = Some(loaded.plan);
    state.order = loaded.order;
    state.branch = Some("spec/0007-session-cache".into());
    state
}

/// An exit status with `stdout`.
pub(crate) fn exit(code: i32, stdout: &str) -> CommandOutput {
    CommandOutput {
        status: Some(code),
        stdout: stdout.into(),
        ..CommandOutput::default()
    }
}

/// A shell that answers each call with the next output of a list (the
/// last one repeats), recording the calls.
#[derive(Debug, Default)]
pub(crate) struct SequenceShell {
    outputs: Mutex<VecDeque<CommandOutput>>,
    last: Mutex<Option<CommandOutput>>,
    calls: Mutex<Vec<RunCommandRequest>>,
}

impl SequenceShell {
    pub(crate) fn new(outputs: impl IntoIterator<Item = CommandOutput>) -> Self {
        Self {
            outputs: Mutex::new(outputs.into_iter().collect()),
            ..Self::default()
        }
    }

    pub(crate) fn calls(&self) -> Vec<RunCommandRequest> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait]
impl ShellPort for SequenceShell {
    async fn run(&self, request: RunCommandRequest) -> Result<CommandOutput, ShellError> {
        self.calls.lock().unwrap().push(request);
        let next = self.outputs.lock().unwrap().pop_front();
        let mut last = self.last.lock().unwrap();
        if let Some(next) = next {
            *last = Some(next);
        }
        last.clone().ok_or(ShellError::NotFound {
            command: "nothing scripted".into(),
        })
    }
}

/// Ports of a step test.
pub(crate) struct Harness {
    pub llm: Arc<ScriptedLlm>,
    pub fs: Arc<InMemoryFs>,
    pub shell: Arc<dyn ShellPort>,
    pub approval: Arc<ScriptedApproval>,
    pub sink: Arc<RecordingSink>,
}

impl Harness {
    /// An agent answering `script`, writing to an in-memory workspace in
    /// auto-edit mode, over `shell`.
    pub(crate) fn new(
        script: impl IntoIterator<Item = ScriptedResponse>,
        shell: Arc<dyn ShellPort>,
    ) -> Self {
        Self {
            llm: Arc::new(ScriptedLlm::new(script)),
            fs: Arc::new(InMemoryFs::new(test_root())),
            shell,
            approval: Arc::new(ScriptedApproval::new()),
            sink: Arc::new(RecordingSink::new()),
        }
    }

    /// The same with a shell that knows no command.
    pub(crate) fn without_shell(script: impl IntoIterator<Item = ScriptedResponse>) -> Self {
        Self::new(script, Arc::new(ScriptedShell::new()))
    }

    pub(crate) fn approve_next(&self) {
        self.approval.push(ApprovalResponse::Approve);
    }

    pub(crate) fn deps(&self) -> ImplementDeps {
        let mut tools = ToolRegistry::new();
        register_file_tools(
            &mut tools,
            FileToolPorts {
                file_search: Arc::clone(&self.fs) as _,
                content_search: Arc::clone(&self.fs) as _,
                fs: Arc::clone(&self.fs) as _,
                shell: Arc::clone(&self.shell),
                rewriter: None,
            },
        )
        .unwrap();
        let agent = AgentDeps {
            llm: Arc::clone(&self.llm) as _,
            events: Arc::clone(&self.sink) as _,
            approval: Arc::clone(&self.approval) as _,
            telemetry: Arc::new(RecordingTelemetry::new()),
        };
        let permissions = Arc::new(Mutex::new(PermissionPolicy::new(PermissionMode::AutoEdit)));
        let kit = AgentKit::new(agent, tools, ModelPolicy::default(), permissions);
        ImplementDeps::new(
            Arc::clone(&self.sink) as _,
            Arc::clone(&self.fs) as _,
            Arc::clone(&self.shell),
        )
        .with_agent(kit)
        .with_clock(|| 42)
    }

    pub(crate) fn ctx(&self, step: Step, state: ImplementState) -> StepContext {
        StepContext::new(step, self.deps(), state)
    }
}
