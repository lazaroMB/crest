//! Restricts the file tools of the implementation agent to the files of
//! its task.
//!
//! [`scoped_tools`] wraps `edit_file` and `write_file` in a
//! [`ScopedFileTool`]: a call on a path listed in `task.files` (or below a
//! listed directory) is prepared as usual; any other path stops first at
//! an explicit approval through the [`ApprovalPort`], whatever the
//! permission mode, and is recorded in the [`TaskScope`] so the step can
//! write it to the journal. A denied call goes back to the model as an
//! error. An approved call still goes through the permission policy, like
//! any write. "Approve always" allows that path for the rest of the task.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use async_trait::async_trait;
use domain::permission::ToolEffect;
use domain::plan::TaskId;
use domain::workspace::WorkspacePath;
use serde_json::Value;

use crate::ports::approval::{
    ApprovalPort, ApprovalPreview, ApprovalRequest, ApprovalRequestId, ApprovalResponse,
};
use crate::tools::registry::{
    PreparedCall, RegisterError, Tool, ToolContext, ToolError, ToolRegistry,
};

/// Tools whose calls change a file named by their subject.
pub const SCOPED_TOOLS: [&str; 2] = ["edit_file", "write_file"];

/// First id of the approvals asked by the scope, far from the agent
/// loop's own counter.
const FIRST_REQUEST_ID: u64 = 1 << 40;

/// A file outside the task that the agent tried to change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeTouch {
    /// Workspace path of the file.
    pub path: String,
    /// Tool that tried (`edit_file`, `write_file`).
    pub tool: String,
    /// Whether the user allowed it.
    pub approved: bool,
    /// Why it was denied, if it was.
    pub reason: Option<String>,
}

/// The files one task may change, and the touches outside them.
pub struct TaskScope {
    task: TaskId,
    allowed: Mutex<BTreeSet<String>>,
    touches: Mutex<Vec<ScopeTouch>>,
    approval: Arc<dyn ApprovalPort>,
    next_id: AtomicU64,
}

impl std::fmt::Debug for TaskScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TaskScope")
            .field("task", &self.task)
            .field("allowed", &*lock(&self.allowed))
            .finish_non_exhaustive()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// `path` normalized as a workspace path, or as given if it is not one.
fn normalized(path: &str) -> String {
    WorkspacePath::parse(path.trim().trim_end_matches('/'))
        .map_or_else(|_| path.trim().to_owned(), |p| p.as_str().to_owned())
}

impl TaskScope {
    /// The scope of `task`, which may change `files`; other files are asked
    /// through `approval`.
    #[must_use]
    pub fn new<I, S>(task: TaskId, files: I, approval: Arc<dyn ApprovalPort>) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        Self {
            task,
            allowed: Mutex::new(files.into_iter().map(|f| normalized(f.as_ref())).collect()),
            touches: Mutex::new(Vec::new()),
            approval,
            next_id: AtomicU64::new(FIRST_REQUEST_ID),
        }
    }

    /// Whether `path` is one of the task's files or below one of them.
    #[must_use]
    pub fn allows(&self, path: &str) -> bool {
        let path = normalized(path);
        lock(&self.allowed).iter().any(|allowed| {
            allowed == "."
                || *allowed == path
                || path
                    .strip_prefix(allowed.as_str())
                    .is_some_and(|rest| rest.starts_with('/'))
        })
    }

    /// The touches outside the task so far, in order.
    #[must_use]
    pub fn touches(&self) -> Vec<ScopeTouch> {
        lock(&self.touches).clone()
    }

    /// Asks the user whether `tool` may change `path`, outside the task.
    async fn ask(
        &self,
        tool: &str,
        path: &str,
        preview: Option<ApprovalPreview>,
        ctx: &ToolContext,
    ) -> Result<(), String> {
        let files: Vec<_> = lock(&self.allowed).iter().cloned().collect();
        let request = ApprovalRequest {
            id: ApprovalRequestId(self.next_id.fetch_add(1, Ordering::Relaxed)),
            tool_use_id: ctx.tool_use_id.clone(),
            tool: tool.to_owned(),
            effect: ToolEffect::Write,
            subject: path.to_owned(),
            preview: preview.unwrap_or_else(|| ApprovalPreview::Text {
                text: format!(
                    "Task {} wants to change {path}, which is not one of its files ({}).",
                    self.task,
                    files.join(", ")
                ),
            }),
        };
        let verdict = match self.approval.request(request).await {
            // "All edits" turns off the mode's questions, not the task's
            // scope: this file only.
            Ok(ApprovalResponse::Approve | ApprovalResponse::ApproveAllEdits) => Ok(()),
            Ok(ApprovalResponse::ApproveAlways { .. }) => {
                lock(&self.allowed).insert(normalized(path));
                Ok(())
            }
            Ok(ApprovalResponse::Deny { reason }) => {
                Err(reason.unwrap_or_else(|| "denied by user".to_owned()))
            }
            Err(err) => Err(format!("not approved: {err}")),
        };
        lock(&self.touches).push(ScopeTouch {
            path: path.to_owned(),
            tool: tool.to_owned(),
            approved: verdict.is_ok(),
            reason: verdict.as_ref().err().cloned(),
        });
        verdict.map_err(|reason| {
            format!(
                "{path} is not one of the files of task {} ({}); the user did not allow \
                 changing it ({reason}). Change only the task's files, or explain in your \
                 answer why another file is needed.",
                self.task,
                files.join(", ")
            )
        })
    }
}

/// A file tool that asks before changing files outside its [`TaskScope`].
pub struct ScopedFileTool {
    inner: Arc<dyn Tool>,
    scope: Arc<TaskScope>,
}

impl ScopedFileTool {
    /// Wraps `inner`, whose prepared subject is the path it changes.
    #[must_use]
    pub fn new(inner: Arc<dyn Tool>, scope: Arc<TaskScope>) -> Self {
        Self { inner, scope }
    }
}

#[async_trait]
impl Tool for ScopedFileTool {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn description(&self) -> &str {
        self.inner.description()
    }

    fn input_schema(&self) -> Value {
        self.inner.input_schema()
    }

    fn effect(&self, input: &Value) -> ToolEffect {
        self.inner.effect(input)
    }

    async fn prepare(&self, input: Value, ctx: &ToolContext) -> Result<PreparedCall, ToolError> {
        let call = self.inner.prepare(input, ctx).await?;
        if self.scope.allows(&call.subject) {
            return Ok(call);
        }
        self.scope
            .ask(self.inner.name(), &call.subject, call.preview.clone(), ctx)
            .await
            .map_err(ToolError::Execution)?;
        Ok(call)
    }
}

/// `tools` with `edit_file` and `write_file` restricted to `scope`.
///
/// # Errors
///
/// [`RegisterError`], which cannot happen for tools already registered.
pub fn scoped_tools(
    tools: &ToolRegistry,
    scope: &Arc<TaskScope>,
) -> Result<ToolRegistry, RegisterError> {
    let mut scoped = ToolRegistry::new()
        .with_budget(tools.budget())
        .with_redactor(tools.redactor().clone());
    for name in tools.names() {
        let Some(tool) = tools.get(&name) else {
            continue;
        };
        let tool: Arc<dyn Tool> = if SCOPED_TOOLS.contains(&name.as_str()) {
            Arc::new(ScopedFileTool::new(tool, Arc::clone(scope)))
        } else {
            tool
        };
        scoped.register(tool)?;
    }
    Ok(scoped)
}

#[cfg(test)]
mod tests {
    use domain::conversation::{SessionId, ToolUseId, TurnId};

    use serde_json::json;

    use super::*;
    use crate::testing::fs::{InMemoryFs, test_root};
    use crate::testing::system::ScriptedApproval;
    use crate::tools::files::{EditFileTool, WriteFileTool};

    fn ctx() -> ToolContext {
        ToolContext::new(SessionId::new(), TurnId::new(), ToolUseId::new("toolu_1"))
    }

    fn registry(fs: &Arc<InMemoryFs>) -> ToolRegistry {
        let mut tools = ToolRegistry::new();
        tools
            .register_typed(EditFileTool::new(Arc::clone(fs) as _))
            .unwrap();
        tools
            .register_typed(WriteFileTool::new(Arc::clone(fs) as _))
            .unwrap();
        tools
    }

    fn scope(approval: &Arc<ScriptedApproval>) -> Arc<TaskScope> {
        Arc::new(TaskScope::new(
            TaskId::new("T1"),
            ["src/a.rs", "docs/"],
            Arc::clone(approval) as _,
        ))
    }

    #[test]
    fn allows_listed_files_and_files_below_listed_dirs() {
        let s = scope(&Arc::new(ScriptedApproval::new()));
        assert!(s.allows("src/a.rs") && s.allows("./src/a.rs"));
        assert!(s.allows("docs/x/y.md"));
        assert!(!s.allows("src/ab.rs") && !s.allows("docsx/a.md") && !s.allows("src"));
    }

    #[tokio::test]
    async fn files_of_the_task_need_no_extra_approval() {
        let fs = Arc::new(InMemoryFs::new(test_root()));
        let approval = Arc::new(ScriptedApproval::new());
        let s = scope(&approval);
        let tools = scoped_tools(&registry(&fs), &s).unwrap();
        let input = json!({ "path": "src/a.rs", "content": "fn a() {}\n" });
        tools.dispatch("write_file", input, &ctx()).await.unwrap();
        assert_eq!(fs.file("src/a.rs").as_deref(), Some("fn a() {}\n"));
        assert!(approval.requests().is_empty() && s.touches().is_empty());
    }

    #[tokio::test]
    async fn other_files_ask_and_are_recorded() {
        let fs = Arc::new(InMemoryFs::new(test_root()));
        let approval = Arc::new(ScriptedApproval::new());
        approval.push(ApprovalResponse::Deny {
            reason: Some("do not touch that".into()),
        });
        approval.push(ApprovalResponse::Approve);
        let s = scope(&approval);
        let tools = scoped_tools(&registry(&fs), &s).unwrap();
        let input = json!({ "path": "Cargo.toml", "content": "[package]\n" });

        let err = tools
            .dispatch("write_file", input.clone(), &ctx())
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("not one of the files of task T1"),
            "{err}"
        );
        assert_eq!(fs.file("Cargo.toml"), None);

        tools.dispatch("write_file", input, &ctx()).await.unwrap();
        assert_eq!(fs.file("Cargo.toml").as_deref(), Some("[package]\n"));

        let requests = approval.requests();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].subject, "Cargo.toml");
        assert!(matches!(requests[0].preview, ApprovalPreview::Diff(_)));
        let touches = s.touches();
        assert_eq!(
            touches.iter().map(|t| t.approved).collect::<Vec<_>>(),
            [false, true]
        );
        assert_eq!(touches[0].reason.as_deref(), Some("do not touch that"));
        assert_eq!(touches[0].tool, "write_file");
    }
}
