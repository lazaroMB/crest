//! Node 14, Critical review: looks for holes in each Spec and its plan
//! and, when it finds serious ones, sends the flow back to the earliest
//! stage that left the information out.
//!
//! The review runs in two steps:
//!
//! 1. Deterministic ([`checks`]): every `[E…]` cited exists, the sections
//!    of the template have content, every paragraph of the background
//!    cites evidence, every constraint has a test, cited files exist in
//!    the workspace (with [`Review::with_workspace`]) and the plan
//!    validates and covers `hypothesis_tests` and `constraint_tests`.
//!    Any serious gap here sends the flow back without calling the model.
//! 2. The model's judgment, as a subagent with a clean context: a new
//!    request holding only `spec.md`, `plan.json` and the evidence with
//!    its sources (never the flow's history), answering
//!    `{"gaps": [{claim, problem, missing_stage, severity}]}` through the
//!    forced tool `submit_review`.
//!
//! Serious gaps end in `Back { to }` to the earliest `missing_stage`, with
//! the gaps in the reason; otherwise `Done` with "Review: no gaps"
//! and a table of minor remarks. Either way `review.md` is written to the
//! folder of each Spec through its [`SpecSink`]. The frame stops after
//! three returns from this stage and asks the user.
//!
//! The plan is read from the flow data [`PLANS_DATA_KEY`] (`Vec<Plan>`)
//! when present, else from `.crest/specs/<folder>/plan.json` through the
//! workspace; without either the plan checks are reported as a minor
//! remark.

pub mod checks;

#[cfg(test)]
mod tests;

use std::fmt::Write as _;
use std::sync::Arc;

use async_trait::async_trait;
use domain::block::{BlockKind, Cell, Column, SourceRef, TableSpec};
use domain::plan::Plan;
use domain::research::{ResearchState, Stage, extract_citations};
use domain::workspace::WorkspacePath;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use self::checks::{EVIDENCE_STAGES, Gap, plan_gaps, spec_gaps};
use super::plan::PLANS_KEY;
use super::spec::render::describe_source;
use super::spec::sink::{MemorySpecSink, SPECS_DIR, SpecSink};
use super::spec::structured::{ask_valid, submit_tool};
use super::spec::{DRAFTS_KEY, SpecDraft, sink_error};
use crate::flows::research::node::{NodeContext, NodeError, NodeOutcome, ResearchNode};
use crate::ports::fs::{FileSystemPort, FsError, ReadFileRequest};

/// Flow data with the plans of the Specs (`Vec<Plan>`), when the Plan
/// node leaves them there.
pub const PLANS_DATA_KEY: &str = "plan.plans";

/// Flow data with every gap of the last review (`Vec<Gap>`).
pub const GAPS_KEY: &str = "review.gaps";

/// File written to the folder of each Spec.
pub const REVIEW_FILE: &str = "review.md";

/// Retries after an invalid answer of the model.
const RETRIES: usize = 1;

/// Most lines read from `plan.json`.
const PLAN_LINES: usize = 100_000;

const SYSTEM: &str = "You are an independent critical reviewer of software Specs. You do not \
know the conversation that produced the Spec: you only see the Spec, its plan and the evidence \
with its sources. Look for gaps: claims the cited evidence does not support, a problem that is \
not measurable, a hypothesis with no way to test it, a proposal that does not solve the \
problem or violates a constraint, a technical specification too thin to implement, and a plan \
that does not cover the Spec. For each gap give the earliest stage of the flow where the \
information is missing (missing_stage) and its severity: major if it prevents implementing or \
defending the Spec, minor if it is an improvement. Do not report style or wording. If there \
are no gaps, submit empty gaps. Reply by calling the submit_review tool.";

const TOOL_DESCRIPTION: &str = "Submits the gaps found in the Spec and its plan.";

/// What the reviewer answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReviewAnswer {
    /// Gaps found; empty if there are none.
    pub gaps: Vec<Gap>,
}

/// The critical review node.
pub struct Review {
    sink: Arc<dyn SpecSink>,
    fs: Option<Arc<dyn FileSystemPort>>,
}

impl std::fmt::Debug for Review {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Review")
            .field("workspace", &self.fs.is_some())
            .finish_non_exhaustive()
    }
}

impl Default for Review {
    fn default() -> Self {
        Self::new()
    }
}

impl Review {
    /// A review writing `review.md` to memory and without workspace
    /// checks (see [`Review::with_sink`] and [`Review::with_workspace`]).
    #[must_use]
    pub fn new() -> Self {
        Self {
            sink: Arc::new(MemorySpecSink::new()),
            fs: None,
        }
    }

    /// Writes `review.md` to `sink` (the one of the Spec node).
    #[must_use]
    pub fn with_sink(mut self, sink: Arc<dyn SpecSink>) -> Self {
        self.sink = sink;
        self
    }

    /// Reads `plan.json` and checks that the cited files exist through
    /// `fs` (read only).
    #[must_use]
    pub fn with_workspace(mut self, fs: Arc<dyn FileSystemPort>) -> Self {
        self.fs = Some(fs);
        self
    }
}

#[async_trait]
impl ResearchNode for Review {
    fn stage(&self) -> Stage {
        Stage::CriticalReview
    }

    async fn run(&self, ctx: &mut NodeContext) -> Result<NodeOutcome, NodeError> {
        let drafts: Vec<SpecDraft> = ctx.data(DRAFTS_KEY).unwrap_or_default();
        if drafts.is_empty() {
            return Ok(NodeOutcome::Back {
                to: Stage::Spec,
                reason: "there is no written Spec to review".into(),
            });
        }
        let planned: Vec<String> = ctx.data(PLANS_KEY).unwrap_or_default();
        let plans: Vec<Plan> = ctx.data(PLANS_DATA_KEY).unwrap_or_default();
        let mut reviews: Vec<(String, Vec<Gap>)> = Vec::with_capacity(drafts.len());
        for draft in &drafts {
            let plan = self.plan_of(&plans, &draft.folder).await?;
            let (gaps, notes) = self
                .review_one(ctx, draft, plan.as_ref(), &planned, ctx.user_reply())
                .await?;
            for note in notes {
                ctx.log(note);
            }
            reviews.push((draft.folder.clone(), gaps));
        }
        self.conclude(ctx, drafts.len(), reviews).await
    }
}

impl Review {
    /// The plan of `folder`: from `plans` (the flow data), else from its
    /// `plan.json` in the workspace.
    ///
    /// # Errors
    ///
    /// A `plan.json` that exists but cannot be read.
    pub(crate) async fn plan_of(
        &self,
        plans: &[Plan],
        folder: &str,
    ) -> Result<Option<Plan>, NodeError> {
        match plans.iter().find(|p| p.spec == folder) {
            Some(p) => Ok(Some(p.clone())),
            None => self.read_plan(folder).await,
        }
    }

    /// Every gap of `draft` and its `plan`: the deterministic ones and,
    /// when none of those is major, the judgment of a clean-context
    /// subagent with the user's `corrections`. `planned` lists the
    /// folders that got a plan. Returns the gaps and the lines for the
    /// journal. Only reads the context: several Specs can be reviewed at
    /// once.
    ///
    /// # Errors
    ///
    /// The model's errors, or a workspace that cannot be read.
    pub(crate) async fn review_one(
        &self,
        ctx: &NodeContext,
        draft: &SpecDraft,
        plan: Option<&Plan>,
        planned: &[String],
        corrections: Option<&str>,
    ) -> Result<(Vec<Gap>, Vec<String>), NodeError> {
        let mut notes = Vec::new();
        let mut gaps = self.deterministic(ctx, draft, planned, plan).await?;
        if gaps.iter().any(Gap::is_major) {
            notes.push(format!(
                "Deterministic review of {}: {} major gaps; the model is not consulted.",
                draft.folder,
                gaps.iter().filter(|g| g.is_major()).count()
            ));
        } else {
            let found = judge(ctx, &mut notes, draft, plan, corrections).await?;
            notes.push(format!(
                "Review of {} by a subagent without history: {} gaps ({} major).",
                draft.folder,
                found.len(),
                found.iter().filter(|g| g.is_major()).count()
            ));
            gaps.extend(found);
        }
        Ok((gaps, notes))
    }

    /// Ends the review of `specs` Specs with the gaps of each folder:
    /// writes `review.md` to each, stores the gaps under [`GAPS_KEY`] and
    /// goes back to the earliest stage of a major gap, or is done with a
    /// summary and the minor remarks.
    ///
    /// # Errors
    ///
    /// The sink's errors, or a block that does not validate.
    pub(crate) async fn conclude(
        &self,
        ctx: &mut NodeContext,
        specs: usize,
        reviews: Vec<(String, Vec<Gap>)>,
    ) -> Result<NodeOutcome, NodeError> {
        for (folder, gaps) in &reviews {
            self.sink
                .write(folder, REVIEW_FILE, &review_markdown(folder, gaps))
                .await
                .map_err(sink_error)?;
        }
        let all: Vec<Gap> = reviews.into_iter().flat_map(|(_, g)| g).collect();
        ctx.set_data(GAPS_KEY, &all)?;
        let major: Vec<&Gap> = all.iter().filter(|g| g.is_major()).collect();
        let minor: Vec<&Gap> = all.iter().filter(|g| !g.is_major()).collect();
        if let Some(to) = major.iter().map(|g| g.missing_stage).min() {
            let list = gap_list(&major);
            ctx.emit(BlockKind::Markdown {
                title: Some(format!("Review: {} gaps", major.len())),
                source: format!("Back to **{}**.\n\n{list}", to.name()),
            })?;
            return Ok(NodeOutcome::Back {
                to,
                reason: format!(
                    "the critical review found {} major gaps:\n{list}",
                    major.len()
                ),
            });
        }
        ctx.emit(BlockKind::Markdown {
            title: None,
            source: format!(
                "Review: no gaps\n\n{specs} Specs reviewed; {} minor remarks.",
                minor.len()
            ),
        })?;
        if !minor.is_empty() {
            ctx.emit(BlockKind::Table(gaps_table(&minor)))?;
        }
        Ok(NodeOutcome::Done)
    }
}

impl Review {
    /// Every deterministic gap of `draft`.
    async fn deterministic(
        &self,
        ctx: &NodeContext,
        draft: &SpecDraft,
        planned: &[String],
        plan: Option<&Plan>,
    ) -> Result<Vec<Gap>, NodeError> {
        let state = ctx.state();
        let evidence_stage = ctx.first_stage_of(&EVIDENCE_STAGES);
        let mut gaps = spec_gaps(&draft.spec_md, state, evidence_stage);
        gaps.extend(self.missing_files(state, &draft.spec_md).await?);
        let claim = format!("{}/plan.json", draft.folder);
        match plan {
            Some(plan) => gaps.extend(plan_gaps(&draft.folder, plan, state)),
            None if !planned.contains(&draft.folder) => gaps.push(Gap::major(
                claim,
                "the Spec has no implementation plan",
                Stage::Plan,
            )),
            None => gaps.push(Gap::minor(
                claim,
                "the plan could not be read to review it",
                Stage::Plan,
            )),
        }
        Ok(gaps)
    }

    /// Cited evidence whose file is not in the workspace.
    async fn missing_files(
        &self,
        state: &ResearchState,
        spec_md: &str,
    ) -> Result<Vec<Gap>, NodeError> {
        let Some(fs) = &self.fs else {
            return Ok(Vec::new());
        };
        let mut gaps = Vec::new();
        for id in extract_citations(spec_md) {
            let Some(e) = state.evidence(id) else {
                continue;
            };
            let SourceRef::File { path, .. } = &e.source else {
                continue;
            };
            let exists = match WorkspacePath::parse(path) {
                Ok(p) => fs
                    .exists(&p)
                    .await
                    .map_err(|e| NodeError::Failed(e.to_string()))?,
                Err(_) => false,
            };
            if !exists {
                gaps.push(Gap::major(
                    format!("[{}] {}", e.id, e.claim),
                    format!("the source `{path}` does not exist in the workspace"),
                    Stage::Observation,
                ));
            }
        }
        Ok(gaps)
    }

    /// `plan.json` of `folder` from the workspace, if readable.
    async fn read_plan(&self, folder: &str) -> Result<Option<Plan>, NodeError> {
        let Some(fs) = &self.fs else {
            return Ok(None);
        };
        let Ok(path) = WorkspacePath::parse(&format!("{SPECS_DIR}/{folder}/plan.json")) else {
            return Ok(None);
        };
        let request = ReadFileRequest {
            path,
            offset: None,
            limit: Some(PLAN_LINES),
        };
        match fs.read_file(request).await {
            Ok(content) => Ok(serde_json::from_str(&content.lines.join("\n")).ok()),
            Err(FsError::NotFound { .. }) => Ok(None),
            Err(e) => Err(NodeError::Failed(format!("{folder}/plan.json: {e}"))),
        }
    }
}

/// Asks a clean-context reviewer for the gaps of `draft`.
async fn judge(
    ctx: &NodeContext,
    notes: &mut Vec<String>,
    draft: &SpecDraft,
    plan: Option<&Plan>,
    corrections: Option<&str>,
) -> Result<Vec<Gap>, NodeError> {
    let tool = submit_tool::<ReviewAnswer>("submit_review", TOOL_DESCRIPTION)?;
    let prompt = prompt(ctx.state(), draft, plan, corrections);
    ask_valid(ctx, notes, SYSTEM, &prompt, &tool, RETRIES, |value| {
        let answer: ReviewAnswer = serde_json::from_value(value)
            .map_err(|e| vec![format!("the JSON does not match the schema: {e}")])?;
        let mut errors = Vec::new();
        for gap in &answer.gaps {
            if gap.missing_stage >= Stage::CriticalReview {
                errors.push(format!(
                    "missing_stage of \"{}\" must be a stage before the review",
                    gap.claim
                ));
            }
            if gap.claim.trim().is_empty() || gap.problem.trim().is_empty() {
                errors.push("each gap needs claim and problem".into());
            }
        }
        if errors.is_empty() {
            Ok(answer.gaps)
        } else {
            Err(errors)
        }
    })
    .await
}

/// The reviewer's only input: the Spec, its plan and the evidence. The
/// flow's history (journal, original request, earlier answers) stays out.
fn prompt(
    state: &ResearchState,
    draft: &SpecDraft,
    plan: Option<&Plan>,
    reply: Option<&str>,
) -> String {
    let mut out = format!(
        "Spec (folder {}), spec.md:\n\n{}\n",
        draft.folder, draft.spec_md
    );
    match plan.and_then(|p| serde_json::to_string_pretty(p).ok()) {
        Some(json) => {
            let _ = write!(out, "\nplan.json:\n\n```json\n{json}\n```\n");
        }
        None => out.push_str("\nplan.json: not available.\n"),
    }
    out.push_str("\nEvidence gathered (id, claim, source):\n");
    if state.evidence.is_empty() {
        out.push_str("- none\n");
    }
    for e in &state.evidence {
        let _ = writeln!(
            out,
            "- [{}] {} ({})",
            e.id,
            e.claim,
            describe_source(&e.source)
        );
    }
    out.push_str("\nPossible stages for missing_stage:\n");
    for stage in Stage::ALL
        .into_iter()
        .filter(|s| *s < Stage::CriticalReview)
    {
        let id = serde_json::to_value(stage)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default();
        let _ = writeln!(out, "- {id}: {}", stage.name());
    }
    if let Some(reply) = reply {
        let _ = write!(out, "\nUser instructions for this review: {reply}\n");
    }
    out
}

/// The gaps as a Markdown list.
fn gap_list(gaps: &[&Gap]) -> String {
    gaps.iter()
        .map(|g| {
            format!(
                "- \"{}\": {} (missing in {})\n",
                g.claim,
                g.problem,
                g.missing_stage.name()
            )
        })
        .collect()
}

/// Table of `gaps`.
fn gaps_table(gaps: &[&Gap]) -> TableSpec {
    let text = |s: &str| Cell::Text(s.to_owned());
    TableSpec {
        columns: ["Claim", "Problem", "Stage", "Severity"]
            .into_iter()
            .map(Column::new)
            .collect(),
        rows: gaps
            .iter()
            .map(|g| {
                vec![
                    text(&g.claim),
                    text(&g.problem),
                    text(g.missing_stage.name()),
                    text(g.severity.name()),
                ]
            })
            .collect(),
    }
}

/// Content of `review.md` for `folder`.
fn review_markdown(folder: &str, gaps: &[Gap]) -> String {
    let major: Vec<&Gap> = gaps.iter().filter(|g| g.is_major()).collect();
    let minor: Vec<&Gap> = gaps.iter().filter(|g| !g.is_major()).collect();
    let mut out = format!("# Critical review of {folder}\n\n");
    if major.is_empty() {
        out.push_str("No major gaps.\n\n");
    } else {
        let _ = write!(out, "## Major gaps\n\n{}\n", gap_list(&major));
    }
    out.push_str("## Minor remarks\n\n");
    if minor.is_empty() {
        out.push_str("None.\n");
    } else {
        out.push_str("| Claim | Problem | Stage |\n| --- | --- | --- |\n");
        for g in minor {
            let cell = |s: &str| s.replace('|', "\\|");
            let _ = writeln!(
                out,
                "| {} | {} | {} |",
                cell(&g.claim),
                cell(&g.problem),
                g.missing_stage.name()
            );
        }
    }
    out
}
