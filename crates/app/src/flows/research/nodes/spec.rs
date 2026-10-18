//! Node 12, Spec: writes one folder per problem statement in
//! `.crest/specs/NNNN-<slug>/` with `spec.md` (the ten sections of the
//! template), `journal.md`, `sources.json` and the CSV of the charts in
//! `evidence/`, which the sources of `spec.md` link to.
//!
//! The model fills the sections through the forced tool `submit_spec`;
//! the constraints table and the sources are rendered from the state.
//! Every `[E…]` cited must exist: an answer citing unknown evidence is
//! asked again once, and then the node fails.

pub mod render;
pub mod sink;
pub(crate) mod structured;

#[cfg(test)]
mod tests;

use std::fmt::Write as _;
use std::sync::Arc;

use async_trait::async_trait;
use domain::block::{BlockKind, ChartSpec};
use domain::research::{ResearchState, Stage};
use serde::{Deserialize, Serialize};

use self::render::{
    SpecAnswer, chart_file, charts_section, citation_problems, cited_evidence, folder_number, slug,
};
use self::sink::{MemorySpecSink, SpecSink, SpecSinkError};
use self::structured::{ask_valid, submit_tool};
use crate::flows::research::node::{NodeContext, NodeError, NodeOutcome, ResearchNode};

/// Flow data with the Specs written so far (`Vec<SpecDraft>`), read by
/// the Plan node.
pub const DRAFTS_KEY: &str = "spec.folders";

/// Flow data where nodes may leave the charts they showed
/// (`Vec<ChartSpec>`); each one is written as a CSV in `evidence/`.
pub const CHARTS_KEY: &str = "evidence.charts";

/// Retries after an answer with unknown citations or missing sections.
const RETRIES: usize = 1;

const SYSTEM: &str = "You are the Spec writer of a software research flow (X-Workflow). You \
write, in English, a Spec for a single problem from the evidence, hypotheses, constraints and \
decisions gathered. Every factual claim cites the ids of the evidence that supports it in \
brackets, such as [E3] or [E1, E4]; use only ids from the list. Do not make up data that is \
not in the evidence. Reply by calling the submit_spec tool.";

const TOOL_DESCRIPTION: &str = "Submits the sections of the Spec. The diagram fields are \
Mermaid without code fences; the rest is markdown.";

/// A Spec written by this node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpecDraft {
    /// Folder name, e.g. `0007-session-cache`.
    pub folder: String,
    /// Problem statement it solves.
    pub problem: String,
    /// Title given by the model.
    pub title: String,
    /// Content of `spec.md`.
    pub spec_md: String,
}

/// The Spec node.
pub struct Spec {
    sink: Arc<dyn SpecSink>,
}

impl std::fmt::Debug for Spec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Spec").finish_non_exhaustive()
    }
}

impl Default for Spec {
    fn default() -> Self {
        Self::new()
    }
}

impl Spec {
    /// A Spec node writing to memory until the flow provides a workspace
    /// sink (see [`Spec::with_sink`]).
    #[must_use]
    pub fn new() -> Self {
        Self::with_sink(Arc::new(MemorySpecSink::new()))
    }

    /// A Spec node writing the folders to `sink`.
    #[must_use]
    pub fn with_sink(sink: Arc<dyn SpecSink>) -> Self {
        Self { sink }
    }
}

/// Records `charts` under [`CHARTS_KEY`] so the Spec writes their data
/// to `evidence/`; a chart with the title of a recorded one replaces it
/// (a node run again does not duplicate its charts).
///
/// # Errors
///
/// [`NodeError::Context`] when the flow data cannot be written.
pub fn record_charts(ctx: &NodeContext, charts: &[ChartSpec]) -> Result<(), NodeError> {
    if charts.is_empty() {
        return Ok(());
    }
    let mut all: Vec<ChartSpec> = ctx.data(CHARTS_KEY).unwrap_or_default();
    for chart in charts {
        match all.iter_mut().find(|c| c.title == chart.title) {
            Some(old) => *old = chart.clone(),
            None => all.push(chart.clone()),
        }
    }
    ctx.set_data(CHARTS_KEY, &all)
}

pub(crate) fn sink_error(e: SpecSinkError) -> NodeError {
    NodeError::Failed(e.to_string())
}

/// Writes `journal.md` of `folder` from the state's journal.
pub(crate) async fn write_journal(
    sink: &dyn SpecSink,
    folder: &str,
    state: &ResearchState,
) -> Result<(), NodeError> {
    sink.write(
        folder,
        "journal.md",
        &render::journal_markdown(&state.journal),
    )
    .await
    .map_err(sink_error)
}

#[async_trait]
impl ResearchNode for Spec {
    fn stage(&self) -> Stage {
        Stage::Spec
    }

    async fn run(&self, ctx: &mut NodeContext) -> Result<NodeOutcome, NodeError> {
        if let Some(back) = missing_input(ctx.state()) {
            return Ok(back);
        }
        let previous: Vec<SpecDraft> = ctx.data(DRAFTS_KEY).unwrap_or_default();
        let existing = self.sink.folders().await.map_err(sink_error)?;
        let mut next = existing
            .iter()
            .chain(previous.iter().map(|d| &d.folder))
            .filter_map(|f| folder_number(f))
            .max()
            .unwrap_or(0)
            + 1;
        let charts: Vec<ChartSpec> = ctx.data(CHARTS_KEY).unwrap_or_default();
        let tool = submit_tool::<SpecAnswer>("submit_spec", TOOL_DESCRIPTION)?;
        let statements = ctx.state().problem_statements.clone();
        let mut drafts = Vec::with_capacity(statements.len());
        for problem in &statements {
            let reused = previous.iter().find(|d| &d.problem == problem);
            let number = match reused.and_then(|d| folder_number(&d.folder)) {
                Some(n) => n,
                None => {
                    next += 1;
                    next - 1
                }
            };
            let number = format!("{number:04}");
            let prompt = prompt(ctx, problem, &statements);
            let state = ctx.state().clone();
            let (answer, spec_md) = ask_valid(ctx, SYSTEM, &prompt, &tool, RETRIES, |value| {
                let answer: SpecAnswer = serde_json::from_value(value)
                    .map_err(|e| vec![format!("the JSON does not match the schema: {e}")])?;
                let answer = answer.normalized();
                let mut errors = answer.problems();
                let md = render::spec_markdown(&number, &answer, &state);
                errors.extend(citation_problems(&md, &answer, &state));
                if errors.is_empty() {
                    Ok((answer, md))
                } else {
                    Err(errors)
                }
            })
            .await?;
            let spec_md = spec_md + &charts_section(&charts);
            let folder = reused.map_or_else(
                || format!("{number}-{}", slug(&answer.title)),
                |d| d.folder.clone(),
            );
            drafts.push(SpecDraft {
                folder,
                problem: problem.clone(),
                title: answer.title,
                spec_md,
            });
        }
        for draft in &drafts {
            let cited = cited_evidence(&draft.spec_md, ctx.state()).len();
            ctx.log(format!(
                "Spec {} written for \"{}\" with {cited} sources cited.",
                draft.folder, draft.problem
            ));
        }
        for draft in &drafts {
            self.write_folder(ctx, draft, &charts).await?;
        }
        let mut all = previous;
        all.retain(|p| !drafts.iter().any(|d| d.folder == p.folder));
        all.extend(drafts);
        ctx.set_data(DRAFTS_KEY, &all)?;
        Ok(NodeOutcome::Done)
    }
}

impl Spec {
    async fn write_folder(
        &self,
        ctx: &NodeContext,
        draft: &SpecDraft,
        charts: &[ChartSpec],
    ) -> Result<(), NodeError> {
        let sink = self.sink.as_ref();
        let folder = draft.folder.as_str();
        let state = ctx.state();
        let cited = cited_evidence(&draft.spec_md, state);
        sink.write(folder, "spec.md", &draft.spec_md)
            .await
            .map_err(sink_error)?;
        let sources = render::sources_json(&cited)
            .map_err(|e| NodeError::Failed(format!("sources.json: {e}")))?;
        sink.write(folder, "sources.json", &sources)
            .await
            .map_err(sink_error)?;
        for (i, chart) in charts.iter().enumerate() {
            sink.write(folder, &chart_file(i, chart), &render::chart_csv(chart))
                .await
                .map_err(sink_error)?;
        }
        write_journal(sink, folder, state).await?;
        ctx.emit(BlockKind::Markdown {
            title: Some(format!("{folder}/spec.md")),
            source: draft.spec_md.clone(),
        })?;
        if !cited.is_empty() {
            ctx.emit(BlockKind::Sources {
                sources: cited.iter().map(|e| e.source.clone()).collect(),
            })?;
        }
        Ok(())
    }
}

/// `Back` when an earlier stage left out what the Spec needs.
fn missing_input(state: &ResearchState) -> Option<NodeOutcome> {
    let (to, reason) = if state.problem_statements.is_empty() {
        (
            Stage::Definition,
            "there is no problem definition to write the Spec from",
        )
    } else if state.evidence.is_empty() {
        (Stage::Observation, "there is no evidence the Spec can cite")
    } else if state.decisions.is_empty() {
        (
            Stage::Modeling,
            "there is no chosen solution to propose in the Spec",
        )
    } else {
        return None;
    };
    Some(NodeOutcome::Back {
        to,
        reason: reason.into(),
    })
}

/// The request for the Spec of `problem`, with the whole state.
fn prompt(ctx: &NodeContext, problem: &str, all: &[String]) -> String {
    let state = ctx.state();
    let mut out = format!(
        "Original request: {}\n\nProblem of this Spec: {problem}\n",
        ctx.problem()
    );
    let others: Vec<&String> = all.iter().filter(|p| p.as_str() != problem).collect();
    if !others.is_empty() {
        out.push_str("\nOther problems (they have their own Spec; out of scope here):\n");
        for p in others {
            let _ = writeln!(out, "- {p}");
        }
    }
    out.push_str("\nEvidence (cite only these ids):\n");
    for e in &state.evidence {
        let _ = writeln!(
            out,
            "- [{}] {} ({})",
            e.id,
            e.claim,
            render::describe_source(&e.source)
        );
    }
    if !state.hypotheses.is_empty() {
        out.push_str("\nHypotheses:\n");
        for h in &state.hypotheses {
            let _ = writeln!(
                out,
                "- {} (priority {}, {:?}): {}{}",
                h.id,
                h.priority,
                h.status,
                h.statement,
                if h.causes.is_empty() {
                    String::new()
                } else {
                    format!("; causes: {}", h.causes.join(", "))
                }
            );
        }
    }
    if !state.constraints.is_empty() {
        out.push_str("\nConstraints (the table is added for you; mention them by id):\n");
        for c in &state.constraints {
            let _ = writeln!(out, "- {} ({:?}): {}", c.id, c.kind, c.statement);
        }
    }
    out.push_str("\nDecisions:\n");
    for d in &state.decisions {
        let evidence: Vec<String> = d.evidence_ids.iter().map(ToString::to_string).collect();
        let _ = writeln!(
            out,
            "- {}: chose \"{}\"; discarded: {}; rationale: {} [{}]",
            d.id,
            d.chosen,
            if d.discarded.is_empty() {
                "none".to_owned()
            } else {
                d.discarded.join(", ")
            },
            d.rationale,
            evidence.join(", ")
        );
    }
    if let Some(reply) = ctx.user_reply() {
        let _ = write!(out, "\nUser corrections: {reply}\n");
    }
    out
}
