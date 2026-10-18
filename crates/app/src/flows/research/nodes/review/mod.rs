//! Node 14, Revisión crítica: looks for holes in each Spec and its plan
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
//! the gaps in the reason; otherwise `Done` with "Revisión: sin huecos"
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

const SYSTEM: &str = "Eres un revisor crítico independiente de Specs de software. No conoces \
la conversación que produjo el Spec: solo ves el Spec, su plan y la evidencia con sus fuentes. \
Busca huecos: afirmaciones que la evidencia citada no sostiene, un problema que no es medible, \
una hipótesis sin forma de probarla, una propuesta que no resuelve el problema o viola una \
restricción, especificación técnica insuficiente para implementarla, y un plan que no cubre el \
Spec. Para cada hueco indica la etapa más temprana del flujo donde falta la información \
(missing_stage) y su gravedad: grave si impide implementar o sostener el Spec, menor si es una \
mejora. No reportes estilo ni redacción. Si no hay huecos, entrega gaps vacío. Responde \
llamando a la herramienta submit_review.";

const TOOL_DESCRIPTION: &str = "Entrega los huecos encontrados en el Spec y su plan.";

/// What the reviewer answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReviewAnswer {
    /// Huecos encontrados; vacío si no hay.
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
                reason: "no hay un Spec escrito que revisar".into(),
            });
        }
        let planned: Vec<String> = ctx.data(PLANS_KEY).unwrap_or_default();
        let plans: Vec<Plan> = ctx.data(PLANS_DATA_KEY).unwrap_or_default();
        let mut reviews: Vec<(String, Vec<Gap>)> = Vec::with_capacity(drafts.len());
        for draft in &drafts {
            let plan = match plans.iter().find(|p| p.spec == draft.folder) {
                Some(p) => Some(p.clone()),
                None => self.read_plan(&draft.folder).await?,
            };
            let mut gaps = self
                .deterministic(ctx, draft, &planned, plan.as_ref())
                .await?;
            let blocked = gaps.iter().any(Gap::is_major);
            if blocked {
                ctx.log(format!(
                    "Revisión determinista de {}: {} huecos graves; no se consulta al modelo.",
                    draft.folder,
                    gaps.iter().filter(|g| g.is_major()).count()
                ));
            } else {
                let found = judge(ctx, draft, plan.as_ref()).await?;
                ctx.log(format!(
                    "Revisión de {} por un subagente sin historial: {} huecos ({} graves).",
                    draft.folder,
                    found.len(),
                    found.iter().filter(|g| g.is_major()).count()
                ));
                gaps.extend(found);
            }
            reviews.push((draft.folder.clone(), gaps));
        }
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
                title: Some(format!("Revisión: {} huecos", major.len())),
                source: format!("Regreso a **{}**.\n\n{list}", to.name_es()),
            })?;
            return Ok(NodeOutcome::Back {
                to,
                reason: format!(
                    "la revisión crítica encontró {} huecos graves:\n{list}",
                    major.len()
                ),
            });
        }
        ctx.emit(BlockKind::Markdown {
            title: None,
            source: format!(
                "Revisión: sin huecos\n\nSe revisaron {} Specs; {} observaciones menores.",
                drafts.len(),
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
                "el Spec no tiene plan de implementación",
                Stage::Plan,
            )),
            None => gaps.push(Gap::minor(
                claim,
                "no se pudo leer el plan para revisarlo",
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
                    format!("la fuente `{path}` no existe en el workspace"),
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
    ctx: &mut NodeContext,
    draft: &SpecDraft,
    plan: Option<&Plan>,
) -> Result<Vec<Gap>, NodeError> {
    let tool = submit_tool::<ReviewAnswer>("submit_review", TOOL_DESCRIPTION)?;
    let prompt = prompt(ctx.state(), draft, plan, ctx.user_reply());
    ask_valid(ctx, SYSTEM, &prompt, &tool, RETRIES, |value| {
        let answer: ReviewAnswer = serde_json::from_value(value)
            .map_err(|e| vec![format!("el JSON no cumple el schema: {e}")])?;
        let mut errors = Vec::new();
        for gap in &answer.gaps {
            if gap.missing_stage >= Stage::CriticalReview {
                errors.push(format!(
                    "missing_stage de «{}» debe ser una etapa anterior a la revisión",
                    gap.claim
                ));
            }
            if gap.claim.trim().is_empty() || gap.problem.trim().is_empty() {
                errors.push("cada hueco necesita claim y problem".into());
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
        "Spec (carpeta {}), spec.md:\n\n{}\n",
        draft.folder, draft.spec_md
    );
    match plan.and_then(|p| serde_json::to_string_pretty(p).ok()) {
        Some(json) => {
            let _ = write!(out, "\nplan.json:\n\n```json\n{json}\n```\n");
        }
        None => out.push_str("\nplan.json: no disponible.\n"),
    }
    out.push_str("\nEvidencia reunida (id, afirmación, fuente):\n");
    if state.evidence.is_empty() {
        out.push_str("- ninguna\n");
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
    out.push_str("\nEtapas posibles para missing_stage:\n");
    for stage in Stage::ALL
        .into_iter()
        .filter(|s| *s < Stage::CriticalReview)
    {
        let id = serde_json::to_value(stage)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default();
        let _ = writeln!(out, "- {id}: {}", stage.name_es());
    }
    if let Some(reply) = reply {
        let _ = write!(
            out,
            "\nIndicaciones del usuario para esta revisión: {reply}\n"
        );
    }
    out
}

/// The gaps as a Markdown list.
fn gap_list(gaps: &[&Gap]) -> String {
    gaps.iter()
        .map(|g| {
            format!(
                "- «{}»: {} (falta en {})\n",
                g.claim,
                g.problem,
                g.missing_stage.name_es()
            )
        })
        .collect()
}

/// Table of `gaps`.
fn gaps_table(gaps: &[&Gap]) -> TableSpec {
    let text = |s: &str| Cell::Text(s.to_owned());
    TableSpec {
        columns: ["Afirmación", "Problema", "Etapa", "Gravedad"]
            .into_iter()
            .map(Column::new)
            .collect(),
        rows: gaps
            .iter()
            .map(|g| {
                vec![
                    text(&g.claim),
                    text(&g.problem),
                    text(g.missing_stage.name_es()),
                    text(g.severity.name_es()),
                ]
            })
            .collect(),
    }
}

/// Content of `review.md` for `folder`.
fn review_markdown(folder: &str, gaps: &[Gap]) -> String {
    let major: Vec<&Gap> = gaps.iter().filter(|g| g.is_major()).collect();
    let minor: Vec<&Gap> = gaps.iter().filter(|g| !g.is_major()).collect();
    let mut out = format!("# Revisión crítica de {folder}\n\n");
    if major.is_empty() {
        out.push_str("Sin huecos graves.\n\n");
    } else {
        let _ = write!(out, "## Huecos graves\n\n{}\n", gap_list(&major));
    }
    out.push_str("## Observaciones menores\n\n");
    if minor.is_empty() {
        out.push_str("Ninguna.\n");
    } else {
        out.push_str("| Afirmación | Problema | Etapa |\n| --- | --- | --- |\n");
        for g in minor {
            let cell = |s: &str| s.replace('|', "\\|");
            let _ = writeln!(
                out,
                "| {} | {} | {} |",
                cell(&g.claim),
                cell(&g.problem),
                g.missing_stage.name_es()
            );
        }
    }
    out
}
