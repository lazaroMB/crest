//! Fast 5, Delivery: the Spec, its plan and its review for each problem
//! statement, at once.
//!
//! Each statement goes through [`Spec::draft_one`], [`Planner::plan_one`]
//! and [`Review::review_one`] in turn, and up to [`MAX_PARALLEL`]
//! statements run concurrently (`buffered`, never `tokio::spawn`). Only
//! then are the folders written, the plans shown and the review recorded,
//! in the order of the statements.
//!
//! Major gaps decide what follows:
//!
//! - a gap an earlier stage must close (the evidence, the hypotheses, the
//!   decision) goes back to the fast stage that owns it, as the deep
//!   review does;
//! - gaps in the Spec or the plan themselves redo only those Specs once,
//!   with the gaps as corrections; if some remain, the user is asked:
//!   "ok" accepts the Specs as they are and anything else redoes them all
//!   with it as corrections.

use std::fmt::Write as _;
use std::sync::Arc;

use async_trait::async_trait;
use domain::block::{BlockKind, ChartSpec};
use domain::plan::Plan;
use domain::research::{Pipeline, Stage};
use futures::{StreamExt, TryStreamExt, stream};

use crate::flows::research::gate::is_approval;
use crate::flows::research::node::{NodeContext, NodeError, NodeOutcome, ResearchNode};
use crate::flows::research::nodes::plan::{PlanTool, Planner};
use crate::flows::research::nodes::review::checks::Gap;
use crate::flows::research::nodes::review::{Review, gap_list};
use crate::flows::research::nodes::spec::sink::SpecSink;
use crate::flows::research::nodes::spec::{CHARTS_KEY, Spec, SpecDraft, SpecJob, missing_input};
use crate::ports::fs::FileSystemPort;
use crate::ports::llm::ToolDefinition;

/// Statements worked on at once.
pub const MAX_PARALLEL: usize = 3;

/// Flow data key of the major gaps the user was asked about
/// (`Vec<Gap>`), while Delivery waits for the answer.
pub const PENDING_KEY: &str = "delivery.pending_gaps";

/// The Delivery node.
pub struct Delivery {
    spec: Spec,
    planner: Planner,
    review: Review,
}

impl std::fmt::Debug for Delivery {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Delivery").finish_non_exhaustive()
    }
}

impl Default for Delivery {
    fn default() -> Self {
        Self::new()
    }
}

/// One statement delivered: its Spec, plan, gaps and journal lines.
struct Delivered {
    draft: SpecDraft,
    plan: Plan,
    gaps: Vec<Gap>,
    notes: Vec<String>,
}

impl Delivered {
    /// Whether a major gap is the Spec's or the plan's own.
    fn redo(&self) -> bool {
        self.gaps
            .iter()
            .any(|g| g.is_major() && owner(g) == Stage::Delivery)
    }
}

/// The fast stage that must close `gap`.
fn owner(gap: &Gap) -> Stage {
    gap.missing_stage.owner_in(Pipeline::Fast)
}

/// The forced tools of the Spec and the plan.
struct Tools {
    spec: ToolDefinition,
    plan: PlanTool,
}

impl Delivery {
    /// A Delivery node writing to memory (see [`Delivery::with_sink`]).
    #[must_use]
    pub fn new() -> Self {
        Self {
            spec: Spec::new(),
            planner: Planner::new(),
            review: Review::new(),
        }
    }

    /// Writes the Spec folders, plans and reviews to `sink`.
    #[must_use]
    pub fn with_sink(sink: Arc<dyn SpecSink>) -> Self {
        Self {
            spec: Spec::with_sink(Arc::clone(&sink)),
            planner: Planner::with_sink(Arc::clone(&sink)),
            review: Review::new().with_sink(sink),
        }
    }

    /// Lets the review read `plan.json` and check cited files through
    /// `fs`.
    #[must_use]
    pub fn with_workspace(mut self, fs: Arc<dyn FileSystemPort>) -> Self {
        self.review = std::mem::take(&mut self.review).with_workspace(fs);
        self
    }

    /// Spec, plan and review of `job`. Only reads the context.
    async fn deliver_one(
        &self,
        ctx: &NodeContext,
        job: &SpecJob,
        corrections: Option<&str>,
        tools: &Tools,
        charts: &[ChartSpec],
    ) -> Result<Delivered, NodeError> {
        let (draft, mut notes) = self
            .spec
            .draft_one(ctx, job, corrections, &tools.spec, charts)
            .await?;
        let (plan, plan_notes) = self
            .planner
            .plan_one(ctx, &draft, corrections, &tools.plan)
            .await?;
        notes.extend(plan_notes);
        let planned = [draft.folder.clone()];
        let (gaps, review_notes) = self
            .review
            .review_one(ctx, &draft, Some(&plan), &planned, ctx.user_reply())
            .await?;
        notes.extend(review_notes);
        Ok(Delivered {
            draft,
            plan,
            gaps,
            notes,
        })
    }

    /// [`Delivery::deliver_one`] for each of `jobs`, at most
    /// [`MAX_PARALLEL`] at once, in the order of `jobs`.
    async fn deliver(
        &self,
        ctx: &NodeContext,
        jobs: &[&SpecJob],
        corrections: Option<&str>,
        tools: &Tools,
        charts: &[ChartSpec],
    ) -> Result<Vec<Delivered>, NodeError> {
        // The futures are built up front (they do nothing until polled):
        // a closure inside the stream would not be `Send` for every
        // lifetime, as `async_trait` needs.
        let runs: Vec<_> = jobs
            .iter()
            .map(|job| self.deliver_one(ctx, job, corrections, tools, charts))
            .collect();
        stream::iter(runs)
            .buffered(MAX_PARALLEL)
            .try_collect()
            .await
    }

    /// Accepts the Specs with the gaps the user was asked about.
    fn accept(ctx: &mut NodeContext, pending: &[Gap]) -> Result<NodeOutcome, NodeError> {
        ctx.set_data(PENDING_KEY, None::<Vec<Gap>>)?;
        ctx.emit(BlockKind::Markdown {
            title: None,
            source: format!(
                "Specs accepted as they are, with {} open gaps (see `review.md`).",
                pending.len()
            ),
        })?;
        ctx.log(format!(
            "The user accepted the Specs with {} major gaps.",
            pending.len()
        ));
        Ok(NodeOutcome::Done)
    }
}

/// The question about the gaps left after the redo.
fn gaps_prompt(major: &[&Gap]) -> String {
    let mut out = format!(
        "The review still finds {} major gaps in the Specs or their plans:\n\n",
        major.len()
    );
    out.push_str(&gap_list(major));
    let _ = write!(
        out,
        "\n\nAnswer \"ok\" to accept the Specs as they are, or write corrections to redo them."
    );
    out
}

#[async_trait]
impl ResearchNode for Delivery {
    fn stage(&self) -> Stage {
        Stage::Delivery
    }

    async fn run(&self, ctx: &mut NodeContext) -> Result<NodeOutcome, NodeError> {
        let pending: Option<Vec<Gap>> = ctx.data::<Option<Vec<Gap>>>(PENDING_KEY).flatten();
        if let Some(pending) = pending {
            if ctx.user_reply().is_some_and(is_approval) {
                return Self::accept(ctx, &pending);
            }
            ctx.set_data(PENDING_KEY, None::<Vec<Gap>>)?;
        }
        if let Some(back) = missing_input(ctx.state()) {
            return Ok(back);
        }
        let charts: Vec<ChartSpec> = ctx.data(CHARTS_KEY).unwrap_or_default();
        let tools = Tools {
            spec: Spec::tool()?,
            plan: PlanTool::new()?,
        };
        let jobs = self.spec.jobs(ctx).await?;
        let reply = ctx.user_reply().map(str::to_owned);

        let shared: &NodeContext = ctx;
        let all: Vec<&SpecJob> = jobs.iter().collect();
        let mut delivered = self
            .deliver(shared, &all, reply.as_deref(), &tools, &charts)
            .await?;
        let earlier = delivered
            .iter()
            .flat_map(|d| &d.gaps)
            .any(|g| g.is_major() && owner(g) < Stage::Delivery);
        let redo: Vec<usize> = (0..delivered.len())
            .filter(|&i| delivered[i].redo())
            .collect();
        let mut redone = Vec::new();
        if !earlier && !redo.is_empty() {
            for &i in &redo {
                let major: Vec<&Gap> = delivered[i].gaps.iter().filter(|g| g.is_major()).collect();
                let mut corrections = format!(
                    "The critical review found these gaps; fix them:\n{}",
                    gap_list(&major)
                );
                if let Some(reply) = &reply {
                    let _ = write!(corrections, "\n{reply}");
                }
                redone.push((i, corrections));
            }
            let runs: Vec<_> = redone
                .iter()
                .map(|(i, corrections)| {
                    self.deliver_one(
                        shared,
                        &jobs[*i],
                        Some(corrections.as_str()),
                        &tools,
                        &charts,
                    )
                })
                .collect();
            let again: Vec<Delivered> = stream::iter(runs)
                .buffered(MAX_PARALLEL)
                .try_collect()
                .await?;
            for ((i, _), mut better) in redone.iter().zip(again) {
                let mut notes = std::mem::take(&mut delivered[*i].notes);
                notes.push(format!(
                    "Spec {} redone once to close the gaps of the review.",
                    better.draft.folder
                ));
                notes.append(&mut better.notes);
                better.notes = notes;
                delivered[*i] = better;
            }
        }

        let specs = delivered.len();
        let mut drafts = Vec::with_capacity(specs);
        let mut plans = Vec::with_capacity(specs);
        let mut reviews = Vec::with_capacity(specs);
        for d in delivered {
            for note in d.notes {
                ctx.log(note);
            }
            reviews.push((d.draft.folder.clone(), d.gaps));
            drafts.push(d.draft);
            plans.push(d.plan);
        }
        self.spec.store(ctx, drafts, &charts).await?;
        for plan in &plans {
            self.planner.apply(ctx, plan).await?;
        }
        self.planner.store(ctx, plans).await?;
        let gaps = self.review.record(ctx, reviews).await?;

        let major: Vec<&Gap> = gaps.iter().filter(|g| g.is_major()).collect();
        let only_own = !major.is_empty() && major.iter().all(|g| owner(g) == Stage::Delivery);
        if !only_own {
            return Review::verdict(ctx, specs, &gaps);
        }
        let pending: Vec<Gap> = major.iter().map(|g| (*g).clone()).collect();
        ctx.set_data(PENDING_KEY, Some(&pending))?;
        ctx.emit(BlockKind::Markdown {
            title: Some(format!("Review: {} gaps", major.len())),
            source: gap_list(&major),
        })?;
        ctx.log(format!(
            "{} major gaps in the Specs or plans after {} redo(s); asking the user.",
            major.len(),
            redone.len()
        ));
        Ok(NodeOutcome::NeedsInput {
            prompt: gaps_prompt(&major),
        })
    }
}

#[cfg(test)]
mod tests {
    use domain::block::SourceRef;
    use domain::model::ModelPolicy;
    use domain::research::{ConstraintKind, ResearchState, TestKind, TestRef};
    use serde_json::{Value, json};

    use super::*;
    use crate::flows::research::node::NodeDeps;
    use crate::flows::research::nodes::review::GAPS_KEY;
    use crate::flows::research::nodes::spec::DRAFTS_KEY;
    use crate::flows::research::nodes::spec::sink::MemorySpecSink;
    use crate::ports::llm::LlmPort;
    use crate::testing::llm::{BarrierLlm, RoutedLlm, ScriptedResponse};
    use crate::testing::system::RecordingSink;

    const SPEC: &str = "You are the Spec writer";
    const PLAN: &str = "You are the planner";
    const REVIEW: &str = "independent critical reviewer";

    pub(crate) fn state(problems: &[&str]) -> ResearchState {
        let mut s = ResearchState::default();
        let e1 = s.add_evidence(
            "Login takes 500 ms at 500 rps",
            SourceRef::Command {
                command: "make bench-login".into(),
                output_excerpt: "p95 500ms".into(),
            },
            1,
        );
        s.add_evidence(
            "Every login reads the session from SQLite",
            SourceRef::File {
                path: "crates/app/src/ports/store.rs".into(),
                line_start: Some(10),
                line_end: Some(20),
            },
            2,
        );
        s.add_hypothesis("An in-memory cache brings p95 down to 20 ms", vec![], 1);
        s.add_constraint(
            ConstraintKind::Performance,
            "p95 < 20 ms at 500 rps",
            Some(TestRef {
                kind: TestKind::Command,
                reference: "make bench-login".into(),
            }),
        );
        s.add_decision("LRU cache", vec!["Redis".into()], "no services", vec![e1]);
        s.problem_statements = problems.iter().map(|p| (*p).to_owned()).collect();
        s
    }

    pub(crate) fn spec() -> ScriptedResponse {
        ScriptedResponse::tool_use(
            "s",
            "submit_spec",
            json!({
                "title": "Session cache",
                "problem": "At 500 rps login goes from 10 ms to 500 ms [E1].",
                "background": "Every login reads SQLite [E2] and is slow [E1].",
                "hypothesis": "An in-memory cache brings p95 down to 20 ms.",
                "proposal": "LRU cache in front of the store [E2].",
                "black_box": "flowchart LR\n  login --> cache --> store",
                "functional": "Login checks the cache before the store.",
                "sequence": "sequenceDiagram\n  login->>cache: get",
                "states": "stateDiagram-v2\n  [*] --> empty",
                "technical": "`SessionCache` in `crates/app/src/ports/store.rs`.",
                "test_protocol": "`make bench-login` must give p95 < 20 ms.",
                "out_of_scope": "Redis.",
                "risks": "Invalidation."
            }),
        )
    }

    pub(crate) fn plan() -> ScriptedResponse {
        ScriptedResponse::tool_use(
            "p",
            "submit_plan",
            json!({
                "spec": "x",
                "tasks": [{
                    "id": "T1", "title": "Cache", "description": "add the cache",
                    "depends_on": [], "files": ["crates/app/src/ports/store.rs"],
                    "acceptance": [{ "kind": "test", "ref": "cargo test cache" }],
                    "risk": "low"
                }],
                "hypothesis_tests": [{ "kind": "command", "ref": "make bench-login",
                    "expect": "p95 < 20ms" }],
                "constraint_tests": [{ "constraint": "C1", "kind": "command",
                    "ref": "make bench-login" }]
            }),
        )
    }

    pub(crate) fn review(gaps: Value) -> ScriptedResponse {
        ScriptedResponse::tool_use("r", "submit_review", json!({ "gaps": gaps }))
    }

    fn gap(stage: &str) -> Value {
        json!([{"claim": "the proposal", "problem": "the eviction policy is not specified",
            "missing_stage": stage, "severity": "major"}])
    }

    fn context(llm: Arc<dyn LlmPort>, problems: &[&str]) -> NodeContext {
        let deps = NodeDeps::new(llm, ModelPolicy::default(), Arc::new(RecordingSink::new()))
            .with_clock(|| 5);
        NodeContext::new(Stage::Delivery, deps, "login is slow", state(problems))
    }

    #[tokio::test]
    async fn statements_are_delivered_at_once_in_a_fixed_order() {
        let routed = Arc::new(
            RoutedLlm::new()
                .route(SPEC, [spec(), spec()])
                .route(PLAN, [plan(), plan()])
                .route(REVIEW, [review(json!([])), review(json!([]))]),
        );
        // Drafted one after another, the first Spec would time out here.
        let llm: Arc<dyn LlmPort> = Arc::new(BarrierLlm::new(routed.clone(), 2));
        let sink = Arc::new(MemorySpecSink::new());
        let delivery = Delivery::with_sink(sink.clone());
        let mut ctx = context(llm, &["first problem", "second problem"]);

        let outcome = delivery.run(&mut ctx).await.unwrap();

        assert_eq!(outcome, NodeOutcome::Done);
        assert_eq!(routed.remaining(), 0);
        let drafts: Vec<SpecDraft> = ctx.data(DRAFTS_KEY).unwrap();
        let folders: Vec<(&str, &str)> = drafts
            .iter()
            .map(|d| (d.folder.as_str(), d.problem.as_str()))
            .collect();
        assert_eq!(
            folders,
            [
                ("0001-session-cache", "first problem"),
                ("0002-session-cache", "second problem")
            ]
        );
        for (folder, _) in folders {
            for file in ["spec.md", "plan.json", "review.md"] {
                assert!(sink.file(folder, file).is_some(), "{folder}/{file}");
            }
        }
        assert!(ctx.data::<Vec<Gap>>(GAPS_KEY).unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_gap_of_the_spec_is_redone_once_then_asked() {
        let routed = Arc::new(
            RoutedLlm::new()
                .route(SPEC, [spec(), spec()])
                .route(PLAN, [plan(), plan()])
                .route(REVIEW, [review(gap("spec")), review(gap("plan"))]),
        );
        let delivery = Delivery::new();
        let mut ctx = context(routed.clone(), &["slow login"]);

        let outcome = delivery.run(&mut ctx).await.unwrap();

        let NodeOutcome::NeedsInput { prompt } = outcome else {
            panic!("expected a question, got {outcome:?}");
        };
        assert!(prompt.contains("eviction policy") && prompt.contains("\"ok\""));
        let specs = routed.requests(SPEC);
        assert_eq!(specs.len(), 2);
        let redo = format!("{:?}", specs[1].messages);
        assert!(redo.contains("eviction policy"), "{redo}");
        assert_eq!(
            ctx.data::<Option<Vec<Gap>>>(PENDING_KEY)
                .flatten()
                .unwrap()
                .len(),
            1
        );

        // "ok" accepts the Specs as they are, without asking the model.
        let mut ctx = ctx.with_reply("ok");
        assert_eq!(delivery.run(&mut ctx).await.unwrap(), NodeOutcome::Done);
        assert_eq!(routed.requests(SPEC).len(), 2);
        assert!(
            ctx.data::<Option<Vec<Gap>>>(PENDING_KEY)
                .flatten()
                .is_none()
        );
    }

    #[tokio::test]
    async fn a_gap_of_an_earlier_stage_goes_back_without_a_redo() {
        let routed = Arc::new(
            RoutedLlm::new()
                .route(SPEC, [spec()])
                .route(PLAN, [plan()])
                .route(REVIEW, [review(gap("observation"))]),
        );
        let mut ctx = context(routed.clone(), &["slow login"]);

        let outcome = Delivery::new().run(&mut ctx).await.unwrap();

        let NodeOutcome::Back { to, reason } = outcome else {
            panic!("expected a return, got {outcome:?}");
        };
        assert_eq!(to.owner_in(Pipeline::Fast), Stage::Discovery);
        assert!(reason.contains("eviction policy"));
        assert_eq!(routed.requests(SPEC).len(), 1);
    }

    #[tokio::test]
    async fn without_a_decision_it_goes_back_to_solution() {
        let mut ctx = context(Arc::new(RoutedLlm::new()), &["slow login"]);
        ctx.state_mut().decisions.clear();
        let outcome = Delivery::new().run(&mut ctx).await.unwrap();
        let NodeOutcome::Back { to, .. } = outcome else {
            panic!("expected a return, got {outcome:?}");
        };
        assert_eq!(to.owner_in(Pipeline::Fast), Stage::Solution);
    }
}
