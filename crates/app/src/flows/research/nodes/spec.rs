//! Node 12, Spec: writes one folder per problem statement in
//! `.crest/specs/NNNN-<slug>/` with `spec.md` (the ten sections of the
//! template), `journal.md`, `sources.json` and the CSV of the charts in
//! `evidence/`.
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

use self::render::{SpecAnswer, citation_problems, cited_evidence, folder_number, slug};
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

const SYSTEM: &str = "Eres el redactor de Specs de un flujo de investigación de software \
(X-Workflow). Redactas en español un Spec para un solo problema, a partir de la evidencia, \
hipótesis, restricciones y decisiones reunidas. Toda afirmación de hecho cita los ids de \
evidencia que la sustentan entre corchetes, como [E3] o [E1, E4]; usa solo ids de la lista. \
No inventes datos que no estén en la evidencia. Responde llamando a la herramienta submit_spec.";

const TOOL_DESCRIPTION: &str = "Entrega las secciones del Spec. Los campos de diagramas son \
Mermaid sin cercas de código; el resto es markdown.";

/// A Spec written by this node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpecDraft {
    /// Folder name, e.g. `0007-cache-de-sesiones`.
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
                    .map_err(|e| vec![format!("el JSON no cumple el schema: {e}")])?;
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
        let charts: Vec<ChartSpec> = ctx.data(CHARTS_KEY).unwrap_or_default();
        for draft in &drafts {
            let cited = cited_evidence(&draft.spec_md, ctx.state()).len();
            ctx.log(format!(
                "Spec {} escrito para «{}» con {cited} fuentes citadas.",
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
            let file = format!("evidence/chart-{}-{}.csv", i + 1, slug(&chart.title));
            sink.write(folder, &file, &render::chart_csv(chart))
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
            "no hay definición del problema para redactar el Spec",
        )
    } else if state.evidence.is_empty() {
        (
            Stage::Observation,
            "no hay evidencia que el Spec pueda citar",
        )
    } else if state.decisions.is_empty() {
        (
            Stage::Modeling,
            "no hay una solución elegida para proponer en el Spec",
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
        "Pedido original: {}\n\nProblema de este Spec: {problem}\n",
        ctx.problem()
    );
    let others: Vec<&String> = all.iter().filter(|p| p.as_str() != problem).collect();
    if !others.is_empty() {
        out.push_str("\nOtros problemas (tienen su propio Spec; quedan fuera de alcance):\n");
        for p in others {
            let _ = writeln!(out, "- {p}");
        }
    }
    out.push_str("\nEvidencia (cita solo estos ids):\n");
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
        out.push_str("\nHipótesis:\n");
        for h in &state.hypotheses {
            let _ = writeln!(
                out,
                "- {} (prioridad {}, {:?}): {}{}",
                h.id,
                h.priority,
                h.status,
                h.statement,
                if h.causes.is_empty() {
                    String::new()
                } else {
                    format!("; causas: {}", h.causes.join(", "))
                }
            );
        }
    }
    if !state.constraints.is_empty() {
        out.push_str("\nRestricciones (la tabla se agrega sola; menciónalas por id):\n");
        for c in &state.constraints {
            let _ = writeln!(out, "- {} ({:?}): {}", c.id, c.kind, c.statement);
        }
    }
    out.push_str("\nDecisiones:\n");
    for d in &state.decisions {
        let evidence: Vec<String> = d.evidence_ids.iter().map(ToString::to_string).collect();
        let _ = writeln!(
            out,
            "- {}: elegida «{}»; descartadas: {}; razón: {} [{}]",
            d.id,
            d.chosen,
            if d.discarded.is_empty() {
                "ninguna".to_owned()
            } else {
                d.discarded.join(", ")
            },
            d.rationale,
            evidence.join(", ")
        );
    }
    if let Some(reply) = ctx.user_reply() {
        let _ = write!(out, "\nCorrecciones del usuario: {reply}\n");
    }
    out
}
