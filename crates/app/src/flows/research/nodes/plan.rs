//! Node 13, Plan: splits each Spec written by the Spec node into tasks
//! with dependencies, files and tests, as `plan.json` (the schema of
//! [`domain::plan::Plan`]), and writes `plan.md` generated from it.
//!
//! The plan comes through the forced tool `submit_plan`; it is checked
//! with [`Plan::validate`] (which rejects cycles) and
//! [`Plan::check_constraints`], and an invalid plan is asked again with
//! its errors up to two times.

use std::fmt::Write as _;
use std::sync::Arc;

use async_trait::async_trait;
use domain::block::{BlockKind, Cell, Column, MermaidSpec, TableSpec};
use domain::plan::Plan;
use domain::research::{ConstraintId, Stage};
use serde_json::Value;

use super::review::PLANS_DATA_KEY;
use super::spec::sink::{MemorySpecSink, SpecSink};
use super::spec::structured::{ask_valid, submit_tool};
use super::spec::{DRAFTS_KEY, SpecDraft, sink_error, write_journal};
use crate::flows::research::node::{NodeContext, NodeError, NodeOutcome, ResearchNode};
use crate::ports::llm::ToolDefinition;

/// Flow data with the folders that got a `plan.json` (`Vec<String>`).
pub const PLANS_KEY: &str = "plan.folders";

/// Retries after an invalid plan.
const RETRIES: usize = 2;

const SYSTEM: &str = "You are the planner of a software research flow. You split a Spec \
into small tasks that are implemented and committed one by one. Each task has an id (T1, \
T2…), depends only on earlier tasks (no cycles), lists the only files it may touch and has \
executable acceptance criteria (tests or commands). Add the hypothesis tests and one test per \
constraint, citing only the constraint ids given. Reply by calling the submit_plan tool with \
JSON that matches the schema.";

const TOOL_DESCRIPTION: &str = "Submits the implementation plan (the content of plan.json).";

/// The Plan node.
pub struct Planner {
    sink: Arc<dyn SpecSink>,
}

impl std::fmt::Debug for Planner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Planner").finish_non_exhaustive()
    }
}

impl Default for Planner {
    fn default() -> Self {
        Self::new()
    }
}

impl Planner {
    /// A Plan node writing to memory until the flow provides a workspace
    /// sink (see [`Planner::with_sink`]).
    #[must_use]
    pub fn new() -> Self {
        Self::with_sink(Arc::new(MemorySpecSink::new()))
    }

    /// A Plan node writing `plan.json` and `plan.md` to `sink`.
    #[must_use]
    pub fn with_sink(sink: Arc<dyn SpecSink>) -> Self {
        Self { sink }
    }

    /// Asks the model for the plan of `draft` until it is valid, with the
    /// user's `corrections`. Returns the plan and the lines for the
    /// journal (retries). Only reads the context: several plans can be
    /// asked at once.
    ///
    /// # Errors
    ///
    /// The model's errors, or [`NodeError::Failed`] when no plan was
    /// valid.
    pub(crate) async fn plan_one(
        &self,
        ctx: &NodeContext,
        draft: &SpecDraft,
        corrections: Option<&str>,
        tool: &PlanTool,
    ) -> Result<(Plan, Vec<String>), NodeError> {
        let known: Vec<ConstraintId> = ctx.state().constraints.iter().map(|c| c.id).collect();
        let prompt = prompt(ctx, draft, &tool.schema, corrections);
        let folder = draft.folder.as_str();
        let mut notes = Vec::new();
        let plan = ask_valid(
            ctx,
            &mut notes,
            SYSTEM,
            &prompt,
            &tool.tool,
            RETRIES,
            |value| check(value, folder, &known),
        )
        .await?;
        Ok((plan, notes))
    }

    /// Writes `plan.json` and `plan.md` of `plan`, shows it and logs it.
    ///
    /// # Errors
    ///
    /// The sink's errors, or a block that does not validate.
    pub(crate) async fn apply(&self, ctx: &mut NodeContext, plan: &Plan) -> Result<(), NodeError> {
        self.write_plan(ctx, plan).await?;
        ctx.log(format!(
            "Plan for {} with {} tasks, {} hypothesis tests and {} constraint tests.",
            plan.spec,
            plan.tasks.len(),
            plan.hypothesis_tests.len(),
            plan.constraint_tests.len()
        ));
        Ok(())
    }

    /// Stores `new` plans in the flow data (replacing earlier plans of the
    /// same Specs) and rewrites the journal of their folders.
    ///
    /// # Errors
    ///
    /// The sink's errors, or flow data that cannot be written.
    pub(crate) async fn store(
        &self,
        ctx: &mut NodeContext,
        new: Vec<Plan>,
    ) -> Result<(), NodeError> {
        let folders: Vec<String> = new.iter().map(|p| p.spec.clone()).collect();
        let mut plans: Vec<Plan> = ctx.data(PLANS_DATA_KEY).unwrap_or_default();
        for plan in new {
            plans.retain(|p| p.spec != plan.spec);
            plans.push(plan);
        }
        for folder in &folders {
            write_journal(self.sink.as_ref(), folder, ctx.state()).await?;
        }
        ctx.set_data(PLANS_KEY, &folders)?;
        ctx.set_data(PLANS_DATA_KEY, &plans)
    }
}

/// The forced tool of the plan and its schema, shown in the prompt.
#[derive(Debug, Clone)]
pub(crate) struct PlanTool {
    tool: ToolDefinition,
    schema: String,
}

impl PlanTool {
    /// The `submit_plan` tool.
    ///
    /// # Errors
    ///
    /// [`NodeError::Failed`] when its schema does not serialize.
    pub(crate) fn new() -> Result<Self, NodeError> {
        let tool = submit_tool::<Plan>("submit_plan", TOOL_DESCRIPTION)?;
        let schema = serde_json::to_string_pretty(&tool.input_schema)
            .map_err(|e| NodeError::Failed(format!("schema of submit_plan: {e}")))?;
        Ok(Self { tool, schema })
    }
}

#[async_trait]
impl ResearchNode for Planner {
    fn stage(&self) -> Stage {
        Stage::Plan
    }

    async fn run(&self, ctx: &mut NodeContext) -> Result<NodeOutcome, NodeError> {
        let drafts: Vec<SpecDraft> = ctx.data(DRAFTS_KEY).unwrap_or_default();
        if drafts.is_empty() {
            return Ok(NodeOutcome::Back {
                to: Stage::Spec,
                reason: "there is no written Spec to split into tasks".into(),
            });
        }
        let tool = PlanTool::new()?;
        let mut plans = Vec::with_capacity(drafts.len());
        for draft in &drafts {
            let (plan, notes) = self.plan_one(ctx, draft, ctx.user_reply(), &tool).await?;
            for note in notes {
                ctx.log(note);
            }
            self.apply(ctx, &plan).await?;
            plans.push(plan);
        }
        self.store(ctx, plans).await?;
        Ok(NodeOutcome::Done)
    }
}

impl Planner {
    async fn write_plan(&self, ctx: &NodeContext, plan: &Plan) -> Result<(), NodeError> {
        let json = serde_json::to_string_pretty(plan)
            .map_err(|e| NodeError::Failed(format!("plan.json: {e}")))?;
        self.sink
            .write(&plan.spec, "plan.json", &(json + "\n"))
            .await
            .map_err(sink_error)?;
        self.sink
            .write(&plan.spec, "plan.md", &plan.to_markdown())
            .await
            .map_err(sink_error)?;
        ctx.emit(BlockKind::Mermaid(MermaidSpec {
            source: plan.to_mermaid(),
            title: Some(format!("Dependencies of {}", plan.spec)),
        }))?;
        ctx.emit(BlockKind::Table(tasks_table(plan)))?;
        Ok(())
    }
}

/// The plan in `value` for `folder`, or every reason it cannot be used.
fn check(mut value: Value, folder: &str, known: &[ConstraintId]) -> Result<Plan, Vec<String>> {
    if let Some(object) = value.as_object_mut() {
        object.insert("spec".into(), Value::String(folder.to_owned()));
    }
    let plan: Plan = serde_json::from_value(value)
        .map_err(|e| vec![format!("the JSON does not match the plan.json schema: {e}")])?;
    let mut errors: Vec<String> = Vec::new();
    if let Err(found) = plan.validate() {
        errors.extend(found.iter().map(ToString::to_string));
    }
    if let Err(found) = plan.check_constraints(known) {
        errors.extend(found.iter().map(ToString::to_string));
    }
    if errors.is_empty() {
        Ok(plan)
    } else {
        Err(errors)
    }
}

/// Table of the tasks in execution order.
fn tasks_table(plan: &Plan) -> TableSpec {
    let tasks = plan
        .topological_order()
        .unwrap_or_else(|_| plan.tasks.iter().collect());
    let text = |s: String| Cell::Text(s);
    let rows = tasks
        .iter()
        .map(|t| {
            let deps: Vec<&str> = t.depends_on.iter().map(|d| d.as_str()).collect();
            let role = serde_json::to_value(t.role)
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_default();
            vec![
                text(t.id.to_string()),
                text(t.title.clone()),
                text(deps.join(", ")),
                text(t.files.join(", ")),
                text(role),
                text(t.risk.as_str().to_owned()),
            ]
        })
        .collect();
    TableSpec {
        columns: ["Id", "Task", "Depends on", "Files", "Role", "Risk"]
            .into_iter()
            .map(Column::new)
            .collect(),
        rows,
    }
}

/// The request for the plan of `draft`.
fn prompt(ctx: &NodeContext, draft: &SpecDraft, schema: &str, corrections: Option<&str>) -> String {
    let mut out = format!(
        "Spec folder (spec field): {}\n\nSpec:\n\n{}\n",
        draft.folder, draft.spec_md
    );
    let constraints = &ctx.state().constraints;
    if constraints.is_empty() {
        out.push_str("\nThere are no constraints: leave constraint_tests empty.\n");
    } else {
        out.push_str("\nValid constraints for constraint_tests:\n");
        for c in constraints {
            let test = c
                .test
                .as_ref()
                .map_or_else(String::new, |t| format!(" (test: {})", t.reference));
            let _ = writeln!(out, "- {}: {}{test}", c.id, c.statement);
        }
    }
    if let Some(reply) = corrections {
        let _ = write!(out, "\nUser corrections: {reply}\n");
    }
    let _ = write!(out, "\nJSON schema of plan.json:\n{schema}\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flows::research::node::NodeDeps;
    use crate::ports::events::DomainEvent;
    use crate::testing::llm::{ScriptedLlm, ScriptedResponse};
    use crate::testing::system::RecordingSink;
    use domain::model::ModelPolicy;
    use domain::research::{ConstraintKind, ResearchState, TestKind, TestRef};
    use serde_json::json;

    const FOLDER: &str = "0004-session-cache";

    fn task(id: &str, deps: &[&str]) -> Value {
        json!({
            "id": id,
            "title": format!("Task {id}"),
            "description": "do something",
            "depends_on": deps,
            "files": ["crates/app/src/ports/store.rs"],
            "acceptance": [{ "kind": "test", "ref": "cargo test -p application cache" }],
            "role": "main",
            "risk": "low"
        })
    }

    fn plan(tasks: Vec<Value>) -> Value {
        json!({
            "tasks": tasks,
            "hypothesis_tests": [{ "kind": "command", "ref": "make bench", "expect": "p95 < 20ms" }],
            "constraint_tests": [{ "constraint": "C1", "kind": "test", "ref": "cargo test offline" }]
        })
    }

    fn context(llm: Arc<ScriptedLlm>, sink: Arc<RecordingSink>, drafts: &[&str]) -> NodeContext {
        let deps = NodeDeps::new(llm, ModelPolicy::default(), sink).with_clock(|| 1);
        let mut state = ResearchState::default();
        state.add_constraint(
            ConstraintKind::Performance,
            "p95 < 20ms",
            Some(TestRef {
                kind: TestKind::Command,
                reference: "make bench".into(),
            }),
        );
        let ctx = NodeContext::new(Stage::Plan, deps, "login is slow", state);
        let drafts: Vec<SpecDraft> = drafts
            .iter()
            .map(|f| SpecDraft {
                folder: (*f).to_owned(),
                problem: "p".into(),
                title: "t".into(),
                spec_md: "# Spec".into(),
            })
            .collect();
        ctx.set_data(DRAFTS_KEY, &drafts).unwrap();
        ctx
    }

    #[tokio::test]
    async fn writes_a_valid_plan_json_and_its_markdown() {
        let llm = Arc::new(ScriptedLlm::new([ScriptedResponse::tool_use(
            "u1",
            "submit_plan",
            plan(vec![task("T1", &[]), task("T2", &["T1"])]),
        )]));
        let events = Arc::new(RecordingSink::new());
        let files = Arc::new(MemorySpecSink::new());
        let mut ctx = context(Arc::clone(&llm), Arc::clone(&events), &[FOLDER]);
        let outcome = Planner::with_sink(files.clone())
            .run(&mut ctx)
            .await
            .unwrap();
        assert_eq!(outcome, NodeOutcome::Done);

        let json = files.file(FOLDER, "plan.json").unwrap();
        let written: Plan = serde_json::from_str(&json).unwrap();
        written.validate().unwrap();
        assert_eq!(written.spec, FOLDER);
        assert_eq!(
            files.file(FOLDER, "plan.md").unwrap(),
            written.to_markdown()
        );
        assert!(
            files
                .file(FOLDER, "journal.md")
                .unwrap()
                .contains("Plan for")
        );

        let req = llm.last_request().unwrap();
        assert_eq!(
            req.tool_choice,
            crate::ports::llm::ToolChoice::Tool("submit_plan".into())
        );
        assert_eq!(req.tools[0].name, "submit_plan");

        let kinds: Vec<BlockKind> = events
            .events()
            .into_iter()
            .filter_map(|e| match e {
                DomainEvent::BlockAdded { block } => Some(block.kind),
                _ => None,
            })
            .collect();
        assert!(matches!(&kinds[0], BlockKind::Mermaid(m) if m.source == written.to_mermaid()));
        assert!(matches!(&kinds[1], BlockKind::Table(t) if t.rows.len() == 2));
        assert_eq!(ctx.data::<Vec<String>>(PLANS_KEY).unwrap(), [FOLDER]);
        let plans: Vec<Plan> = ctx.data(PLANS_DATA_KEY).unwrap();
        assert_eq!(plans, [written]);
    }

    #[tokio::test]
    async fn a_cycle_is_asked_again_with_its_errors() {
        let llm = Arc::new(ScriptedLlm::new([
            ScriptedResponse::tool_use(
                "u1",
                "submit_plan",
                plan(vec![task("T1", &["T2"]), task("T2", &["T1"])]),
            ),
            ScriptedResponse::tool_use("u2", "submit_plan", plan(vec![task("T1", &[])])),
        ]));
        let files = Arc::new(MemorySpecSink::new());
        let mut ctx = context(Arc::clone(&llm), Arc::new(RecordingSink::new()), &[FOLDER]);
        let outcome = Planner::with_sink(files.clone())
            .run(&mut ctx)
            .await
            .unwrap();
        assert_eq!(outcome, NodeOutcome::Done);

        let requests = llm.requests();
        assert_eq!(requests.len(), 2);
        let retry = requests[1].messages[0].content.clone();
        let text = format!("{retry:?}");
        assert!(text.contains("cycle"), "{text}");
        let written: Plan =
            serde_json::from_str(&files.file(FOLDER, "plan.json").unwrap()).unwrap();
        assert_eq!(written.tasks.len(), 1);
    }

    #[tokio::test]
    async fn unknown_constraints_fail_after_two_retries() {
        let mut bad = plan(vec![task("T1", &[])]);
        bad["constraint_tests"][0]["constraint"] = json!("C9");
        let llm = Arc::new(ScriptedLlm::new((0..3).map(|i| {
            ScriptedResponse::tool_use(format!("u{i}"), "submit_plan", bad.clone())
        })));
        let files = Arc::new(MemorySpecSink::new());
        let mut ctx = context(Arc::clone(&llm), Arc::new(RecordingSink::new()), &[FOLDER]);
        let err = Planner::with_sink(files.clone())
            .run(&mut ctx)
            .await
            .unwrap_err();
        assert!(
            matches!(&err, NodeError::Failed(m) if m.contains("C9")),
            "{err}"
        );
        assert_eq!(llm.requests().len(), 3);
        assert!(files.paths().is_empty());
    }

    #[tokio::test]
    async fn json_in_text_is_accepted() {
        let text = format!("```json\n{}\n```", plan(vec![task("T1", &[])]));
        let llm = Arc::new(ScriptedLlm::new([ScriptedResponse::text(text)]));
        let files = Arc::new(MemorySpecSink::new());
        let mut ctx = context(llm, Arc::new(RecordingSink::new()), &[FOLDER]);
        Planner::with_sink(files.clone())
            .run(&mut ctx)
            .await
            .unwrap();
        assert!(files.file(FOLDER, "plan.json").is_some());
    }

    #[tokio::test]
    async fn without_a_spec_it_goes_back_to_the_spec() {
        let llm = Arc::new(ScriptedLlm::new([]));
        let mut ctx = context(llm, Arc::new(RecordingSink::new()), &[]);
        let outcome = Planner::new().run(&mut ctx).await.unwrap();
        assert!(matches!(
            outcome,
            NodeOutcome::Back {
                to: Stage::Spec,
                ..
            }
        ));
    }
}
