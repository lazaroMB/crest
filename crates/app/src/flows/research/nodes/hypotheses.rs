//! Node 5, Hipótesis: one-sentence hypotheses, their causes and priority.
//!
//! The `plan` model reads the evidence and the findings of the analysis
//! and answers a JSON list of hypotheses with causes grouped by category.
//! They are stored in [`ResearchState::hypotheses`], drawn as an Ishikawa
//! diagram (`Mermaid` flowchart: cause → category → effect) and listed in
//! a `Table` by priority. At the gate any answer other than an approval
//! reruns the node with the current hypotheses and the user's words, so
//! the user can reprioritize, edit, add or drop them; kept hypotheses keep
//! their id and status.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use async_trait::async_trait;
use domain::block::{BlockKind, Cell, Column, MermaidSpec, TableSpec};
use domain::research::{
    EvidenceId, Hypothesis, HypothesisId, HypothesisStatus, ResearchState, Stage,
};
use serde::Deserialize;

use super::analysis::{self, Finding, KEY_FINDINGS};
use crate::flows::research::node::{NodeContext, NodeError, NodeOutcome, ResearchNode};

/// Flow data key of the evidence behind each hypothesis
/// (`BTreeMap<HypothesisId as string, Vec<EvidenceId>>`).
pub const KEY_EVIDENCE: &str = "hypotheses.evidence";
/// Flow data key of the effect at the head of the Ishikawa diagram.
pub const KEY_EFFECT: &str = "hypotheses.effect";

const SYSTEM: &str = "\
Eres la etapa de Hipótesis de una investigación sobre un proyecto de software. \
Propón hipótesis de una oración que expliquen el efecto observado, cada una con sus \
causas agrupadas por categoría (diagrama de Ishikawa: Código, Datos, Infraestructura, \
Dependencias, Configuración, Uso…) y una prioridad (1 es la más alta: la más probable \
y barata de probar). Básate en la evidencia y cita sus ids. Responde solo con un objeto \
JSON, sin texto extra.";

const SCHEMA: &str = r#"Formato de la respuesta:
{
  "effect": "efecto observado, en pocas palabras",
  "hypotheses": [
    {
      "id": "H1",
      "statement": "hipótesis en una oración",
      "priority": 1,
      "causes": [{ "category": "Código", "cause": "causa concreta" }],
      "evidence_ids": ["E1"]
    }
  ]
}
"id" solo para conservar una hipótesis actual; omítelo en las nuevas. Devuelve la lista
completa: las hipótesis actuales que no aparezcan se descartan."#;

#[derive(Debug, Clone, Deserialize)]
struct CauseOut {
    category: String,
    cause: String,
}

#[derive(Debug, Clone, Deserialize)]
struct HypothesisOut {
    #[serde(default)]
    id: Option<HypothesisId>,
    statement: String,
    priority: u8,
    causes: Vec<CauseOut>,
    #[serde(default)]
    evidence_ids: Vec<EvidenceId>,
}

#[derive(Debug, Clone, Deserialize)]
struct Answer {
    effect: String,
    hypotheses: Vec<HypothesisOut>,
}

/// The Hipótesis node.
#[derive(Debug, Default)]
pub struct Hypotheses;

impl Hypotheses {
    /// The node.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ResearchNode for Hypotheses {
    fn stage(&self) -> Stage {
        Stage::Hypotheses
    }

    async fn run(&self, ctx: &mut NodeContext) -> Result<NodeOutcome, NodeError> {
        if ctx.state().evidence.is_empty() {
            let to = analysis::back_target(ctx.state(), &[Stage::Data, Stage::Observation]);
            return Ok(NodeOutcome::Back {
                to,
                reason: "no hay evidencia en la que basar las hipótesis".into(),
            });
        }
        let findings: Vec<Finding> = ctx.data(KEY_FINDINGS).unwrap_or_default();
        let prompt = prompt(ctx.problem(), ctx.state(), &findings, ctx.user_reply());
        let state = ctx.state().clone();
        let answer: Answer = analysis::ask_json(ctx, SYSTEM, &prompt, |a| check(a, &state)).await?;

        let evidence = apply(ctx.state_mut(), &answer);
        let effect = one_line(&answer.effect);
        let hypotheses = by_priority(&ctx.state().hypotheses);
        ctx.emit(BlockKind::Mermaid(MermaidSpec {
            source: ishikawa(&effect, &hypotheses),
            title: Some("Diagrama de causas (Ishikawa)".into()),
        }))?;
        ctx.emit(BlockKind::Table(table(&hypotheses, &evidence)))?;
        ctx.set_data(KEY_EVIDENCE, &evidence)?;
        ctx.set_data(KEY_EFFECT, &effect)?;

        let order: Vec<String> = hypotheses.iter().map(|h| h.id.to_string()).collect();
        let action = if ctx.user_reply().is_some() {
            "Hipótesis revisadas con el usuario"
        } else {
            "Hipótesis"
        };
        ctx.log(format!(
            "{action}: {} por prioridad ({}); efecto: {effect}.",
            order.len(),
            order.join(", ")
        ));
        Ok(NodeOutcome::Done)
    }
}

fn prompt(
    problem: &str,
    state: &ResearchState,
    findings: &[Finding],
    reply: Option<&str>,
) -> String {
    let mut out = analysis::context_prompt(problem, state, reply);
    if !findings.is_empty() {
        out.push_str("\nHallazgos del análisis:\n");
        for f in findings {
            let ids: Vec<String> = f.evidence_ids.iter().map(ToString::to_string).collect();
            let _ = writeln!(out, "- {} [{}]", f.statement, ids.join(", "));
        }
    }
    if !state.hypotheses.is_empty() {
        out.push_str("\nHipótesis actuales:\n");
        for h in by_priority(&state.hypotheses) {
            let _ = writeln!(
                out,
                "- {} (prioridad {}, {}): {} Causas: {}",
                h.id,
                h.priority,
                status_label(h.status),
                h.statement,
                h.causes.join("; ")
            );
        }
    }
    let _ = write!(out, "\n{SCHEMA}");
    out
}

fn check(answer: &Answer, state: &ResearchState) -> Result<(), String> {
    if answer.effect.trim().is_empty() {
        return Err("falta \"effect\"".into());
    }
    if answer.hypotheses.is_empty() {
        return Err("\"hypotheses\" está vacío".into());
    }
    let mut seen = Vec::new();
    for (i, h) in answer.hypotheses.iter().enumerate() {
        let n = i + 1;
        let statement = h.statement.trim();
        if statement.is_empty() || statement.contains('\n') {
            return Err(format!("la hipótesis {n} debe ser una sola oración"));
        }
        if h.priority == 0 {
            return Err(format!(
                "la hipótesis {n} tiene prioridad 0; la más alta es 1"
            ));
        }
        if h.causes.is_empty()
            || h.causes
                .iter()
                .any(|c| c.category.trim().is_empty() || c.cause.trim().is_empty())
        {
            return Err(format!("la hipótesis {n} necesita causas con categoría"));
        }
        if !h.evidence_ids.is_empty() {
            analysis::cites_known(state, &h.evidence_ids, &format!("la hipótesis {n}"))?;
        }
        if let Some(id) = h.id {
            if !state.hypotheses.iter().any(|old| old.id == id) {
                return Err(format!("{id} no es una hipótesis actual; omite el id"));
            }
            if seen.contains(&id) {
                return Err(format!("{id} aparece dos veces"));
            }
            seen.push(id);
        }
    }
    Ok(())
}

/// Replaces the hypotheses of `state` with `answer`, keeping the id and
/// status of those it names; returns the evidence of each one.
fn apply(state: &mut ResearchState, answer: &Answer) -> BTreeMap<String, Vec<EvidenceId>> {
    let mut ids = state.allocator();
    let old = std::mem::take(&mut state.hypotheses);
    let mut evidence = BTreeMap::new();
    for h in &answer.hypotheses {
        let kept = h.id.and_then(|id| old.iter().find(|o| o.id == id));
        let id = kept.map_or_else(|| ids.next_hypothesis(), |o| o.id);
        evidence.insert(id.to_string(), h.evidence_ids.clone());
        state.hypotheses.push(Hypothesis {
            id,
            statement: one_line(&h.statement),
            causes: h
                .causes
                .iter()
                .map(|c| format!("{}: {}", one_line(&c.category), one_line(&c.cause)))
                .collect(),
            priority: h.priority,
            status: kept.map_or(HypothesisStatus::Open, |o| o.status),
        });
    }
    evidence
}

/// The hypotheses ordered by priority, then id.
pub(super) fn by_priority(hypotheses: &[Hypothesis]) -> Vec<Hypothesis> {
    let mut sorted = hypotheses.to_vec();
    sorted.sort_by_key(|h| (h.priority, h.id));
    sorted
}

pub(super) fn status_label(status: HypothesisStatus) -> &'static str {
    match status {
        HypothesisStatus::Open => "abierta",
        HypothesisStatus::Supported => "respaldada",
        HypothesisStatus::Refuted => "refutada",
        HypothesisStatus::Untested => "sin probar",
    }
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Text safe inside a quoted Mermaid label.
fn label(s: &str) -> String {
    one_line(s).replace('"', "#quot;")
}

/// A cause of the diagram and the hypotheses that name it.
type CauseNode = (String, Vec<HypothesisId>);

/// The Ishikawa diagram as a flowchart: each cause points to its category
/// and each category to the effect. Causes are `Categoría: causa`; equal
/// causes of several hypotheses are drawn once, tagged with their ids.
fn ishikawa(effect: &str, hypotheses: &[Hypothesis]) -> String {
    let mut categories: Vec<(String, Vec<CauseNode>)> = Vec::new();
    for h in hypotheses {
        for cause in &h.causes {
            let (category, text) = cause.split_once(": ").unwrap_or(("Otras", cause.as_str()));
            let i = categories
                .iter()
                .position(|(c, _)| c.eq_ignore_ascii_case(category))
                .unwrap_or_else(|| {
                    categories.push((category.to_owned(), Vec::new()));
                    categories.len() - 1
                });
            let causes = &mut categories[i].1;
            match causes.iter_mut().find(|(t, _)| t == text) {
                Some((_, ids)) if !ids.contains(&h.id) => ids.push(h.id),
                Some(_) => {}
                None => causes.push((text.to_owned(), vec![h.id])),
            }
        }
    }
    let mut out = format!("flowchart LR\n  effect([\"{}\"])\n", label(effect));
    for (c, (category, causes)) in categories.iter().enumerate() {
        let _ = writeln!(out, "  cat{c}[\"{}\"] --> effect", label(category));
        for (k, (cause, ids)) in causes.iter().enumerate() {
            let ids: Vec<String> = ids.iter().map(ToString::to_string).collect();
            let _ = writeln!(
                out,
                "  cat{c}_{k}[\"{} ({})\"] --> cat{c}",
                label(cause),
                ids.join(", ")
            );
        }
    }
    out
}

fn table(hypotheses: &[Hypothesis], evidence: &BTreeMap<String, Vec<EvidenceId>>) -> TableSpec {
    let text = |s: String| Cell::Text(s);
    TableSpec {
        columns: [
            "Prioridad",
            "Id",
            "Hipótesis",
            "Causas",
            "Evidencia",
            "Estado",
        ]
        .into_iter()
        .map(Column::new)
        .collect(),
        rows: hypotheses
            .iter()
            .map(|h| {
                let ids = evidence
                    .get(&h.id.to_string())
                    .map(|ids| {
                        ids.iter()
                            .map(ToString::to_string)
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_default();
                vec![
                    Cell::Number(f64::from(h.priority)),
                    text(h.id.to_string()),
                    text(h.statement.clone()),
                    text(h.causes.join("; ")),
                    text(ids),
                    text(status_label(h.status).to_owned()),
                ]
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use domain::block::SourceRef;
    use domain::model::{ModelPolicy, ModelRole};
    use serde_json::json;

    use super::*;
    use crate::flows::research::node::NodeDeps;
    use crate::ports::events::DomainEvent;
    use crate::testing::llm::{ScriptedLlm, ScriptedResponse};
    use crate::testing::system::RecordingSink;

    fn blocks(sink: &RecordingSink) -> Vec<BlockKind> {
        sink.take()
            .into_iter()
            .filter_map(|e| match e {
                DomainEvent::BlockAdded { block } => Some(block.kind),
                _ => None,
            })
            .collect()
    }

    fn state() -> ResearchState {
        let mut state = ResearchState::default();
        state.add_evidence(
            "3 consultas por login",
            SourceRef::File {
                path: "src/login.rs".into(),
                line_start: Some(4),
                line_end: None,
            },
            1,
        );
        state
    }

    fn context(
        llm: &Arc<ScriptedLlm>,
        sink: &Arc<RecordingSink>,
        state: ResearchState,
    ) -> NodeContext {
        let deps = NodeDeps::new(
            Arc::clone(llm) as _,
            ModelPolicy::default(),
            Arc::clone(sink) as _,
        )
        .with_clock(|| 9);
        NodeContext::new(Stage::Hypotheses, deps, "el login tarda 2 s", state)
    }

    fn first_answer() -> String {
        json!({
            "effect": "login de 2 s",
            "hypotheses": [
                {
                    "statement": "El pool de conexiones se agota con 50 usuarios.",
                    "priority": 2,
                    "causes": [{ "category": "Infraestructura", "cause": "pool de 5 \"conexiones\"" }]
                },
                {
                    "statement": "Las 3 consultas por login no usan índice.",
                    "priority": 1,
                    "causes": [
                        { "category": "Datos", "cause": "falta índice en users.email" },
                        { "category": "Código", "cause": "consultas N+1" }
                    ],
                    "evidence_ids": ["E1"]
                }
            ]
        })
        .to_string()
    }

    #[tokio::test]
    async fn hypotheses_go_to_the_state_with_a_diagram_and_a_table() {
        let llm = Arc::new(ScriptedLlm::new([ScriptedResponse::text(first_answer())]));
        let sink = Arc::new(RecordingSink::new());
        let mut ctx = context(&llm, &sink, state());
        assert_eq!(
            Hypotheses::new().run(&mut ctx).await.unwrap(),
            NodeOutcome::Done
        );
        assert_eq!(llm.last_request().unwrap().model.role, ModelRole::Plan);

        let hs = &ctx.state().hypotheses;
        assert_eq!(hs.len(), 2);
        assert_eq!((hs[1].id.to_string(), hs[1].priority), ("H2".into(), 1));
        assert_eq!(
            hs[1].causes,
            [
                "Datos: falta índice en users.email",
                "Código: consultas N+1"
            ]
        );
        assert!(hs.iter().all(|h| h.status == HypothesisStatus::Open));

        let blocks = blocks(&sink);
        let BlockKind::Mermaid(diagram) = &blocks[0] else {
            panic!("{blocks:?}")
        };
        assert!(
            diagram
                .source
                .starts_with("flowchart LR\n  effect([\"login de 2 s\"])")
        );
        assert!(
            diagram.source.contains("cat0[\"Datos\"] --> effect"),
            "{}",
            diagram.source
        );
        assert!(
            diagram
                .source
                .contains("pool de 5 #quot;conexiones#quot; (H1)")
        );
        let BlockKind::Table(table) = &blocks[1] else {
            panic!("{blocks:?}")
        };
        assert_eq!(table.rows[0][1], Cell::Text("H2".into()));
        assert_eq!(table.rows[0][4], Cell::Text("E1".into()));
        let evidence: BTreeMap<String, Vec<EvidenceId>> = ctx.data(KEY_EVIDENCE).unwrap();
        assert_eq!(evidence["H2"].len(), 1);
        assert!(ctx.state().journal[0].text.contains("(H2, H1)"));
    }

    #[tokio::test]
    async fn the_gate_reply_reprioritizes_and_edits_them() {
        let llm = Arc::new(ScriptedLlm::new([ScriptedResponse::text(first_answer())]));
        let sink = Arc::new(RecordingSink::new());
        let mut ctx = context(&llm, &sink, state());
        Hypotheses::new().run(&mut ctx).await.unwrap();
        let mut state = ctx.into_state();
        state.hypotheses[0].status = HypothesisStatus::Untested;

        llm.push(ScriptedResponse::text(
            json!({
                "effect": "login de 2 s",
                "hypotheses": [
                    {
                        "id": "H1",
                        "statement": "El pool se agota con 20 usuarios.",
                        "priority": 1,
                        "causes": [{ "category": "Infraestructura", "cause": "pool de 5" }]
                    },
                    {
                        "statement": "La caché de sesiones expira cada request.",
                        "priority": 2,
                        "causes": [{ "category": "Configuración", "cause": "TTL de 0" }]
                    }
                ]
            })
            .to_string(),
        ));
        let mut ctx =
            context(&llm, &sink, state).with_reply("sube H1 y descarta H2; agrega la caché");
        Hypotheses::new().run(&mut ctx).await.unwrap();
        let sent = serde_json::to_string(&llm.last_request().unwrap().messages).unwrap();
        assert!(sent.contains("sube H1 y descarta H2"), "{sent}");
        assert!(sent.contains("H2 (prioridad 1, abierta)"), "{sent}");

        let hs = &ctx.state().hypotheses;
        let ids: Vec<String> = hs.iter().map(|h| h.id.to_string()).collect();
        assert_eq!(ids, ["H1", "H3"]);
        assert_eq!(hs[0].statement, "El pool se agota con 20 usuarios.");
        assert_eq!(
            (hs[0].priority, hs[0].status),
            (1, HypothesisStatus::Untested)
        );
        assert!(
            ctx.state()
                .journal
                .last()
                .unwrap()
                .text
                .starts_with("Hipótesis revisadas")
        );
    }

    #[tokio::test]
    async fn unknown_ids_are_retried() {
        let bad = json!({
            "effect": "e",
            "hypotheses": [{ "id": "H7", "statement": "x.", "priority": 1,
                "causes": [{ "category": "Código", "cause": "c" }] }]
        });
        let llm = Arc::new(ScriptedLlm::new([
            ScriptedResponse::text(bad.to_string()),
            ScriptedResponse::text(first_answer()),
        ]));
        let sink = Arc::new(RecordingSink::new());
        let mut ctx = context(&llm, &sink, state());
        Hypotheses::new().run(&mut ctx).await.unwrap();
        let retry = serde_json::to_string(&llm.last_request().unwrap().messages).unwrap();
        assert!(retry.contains("H7 no es una hipótesis actual"), "{retry}");
        assert_eq!(ctx.state().hypotheses.len(), 2);
    }

    #[tokio::test]
    async fn without_evidence_it_goes_back_to_the_data() {
        let llm = Arc::new(ScriptedLlm::new([]));
        let sink = Arc::new(RecordingSink::new());
        let mut ctx = context(&llm, &sink, ResearchState::default());
        let outcome = Hypotheses::new().run(&mut ctx).await.unwrap();
        assert!(matches!(
            outcome,
            NodeOutcome::Back {
                to: Stage::Data,
                ..
            }
        ));
        assert!(llm.requests().is_empty());
    }
}
