//! Node 7, Definición y restricciones: the problem in measurable
//! one-sentence statements (one per Spec) and the constraint table.
//!
//! The model drafts both as JSON from the hypotheses and the evidence.
//! Every statement must pass the [`measurable`] heuristic and then the
//! model's own judgement; when one fails, the model is asked once more
//! with the reasons, and if it still fails the user is asked for what is
//! missing. Without evidence or hypotheses the flow goes back to the
//! stage that should have produced them.

pub mod measurable;

use std::fmt::Write as _;

use async_trait::async_trait;
use domain::block::{BlockKind, Cell, Column, TableSpec};
use domain::research::{ConstraintKind, EvidenceId, Stage, TestKind, TestRef};
use serde::{Deserialize, Serialize};

use crate::flows::research::node::{NodeContext, NodeError, NodeOutcome, ResearchNode};

/// Key of the statements with the evidence they rest on
/// (`Vec<ProblemDraft>`), for the Spec.
pub const PROBLEMS_KEY: &str = "definition.problems";

const DRAFT_SYSTEM: &str = "\
Eres el nodo «Definición y restricciones» de una investigación sobre un proyecto de software. \
Define el problema en una oración medible (si hay varios problemas independientes, una oración \
por problema; cada uno será un Spec) y la tabla de restricciones que la solución debe respetar.
- Una definición medible tiene una magnitud con unidad, una comparación con un número o una \
condición verificable. Buena: «a 500 requests por segundo el tiempo de respuesta sube de 10 ms \
a 500 ms». Mala: «el tiempo de respuesta depende de la carga».
- Apóyate solo en las hipótesis y la evidencia dadas; cita los ids de evidencia (E1…) que \
respaldan cada definición.
- Cada restricción tiene un tipo (platform, performance, compatibility, security u other) y, si \
se conoce, la prueba que la verifica (kind test o command, y ref con el comando).
Responde solo con JSON:
{\"problems\": [{\"statement\": \"…\", \"evidence_ids\": [\"E1\"]}],
 \"constraints\": [{\"kind\": \"performance\", \"statement\": \"…\", \
\"test\": {\"kind\": \"command\", \"ref\": \"…\"}}]}
\"test\" puede ser null.";

const JUDGE_SYSTEM: &str = "\
Juzga si cada definición de problema es medible: debe poder comprobarse con una medición o una \
prueba concreta (magnitud con unidad, umbral numérico o condición verificable) y ser una sola \
oración. Sé estricto: «el tiempo de respuesta depende de la carga» no es medible.
Responde solo con JSON: {\"verdicts\": [{\"index\": 0, \"measurable\": true, \"reason\": \"…\"}]} \
con un veredicto por definición; en reason di qué falta cuando no es medible.";

/// One problem statement as drafted by the model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProblemDraft {
    /// The measurable one-sentence statement.
    pub statement: String,
    /// Evidence it rests on.
    #[serde(default)]
    pub evidence_ids: Vec<EvidenceId>,
}

#[derive(Debug, Clone, Deserialize)]
struct ConstraintDraft {
    kind: ConstraintKind,
    statement: String,
    #[serde(default)]
    test: Option<TestRef>,
}

#[derive(Debug, Clone, Deserialize)]
struct Draft {
    problems: Vec<ProblemDraft>,
    #[serde(default)]
    constraints: Vec<ConstraintDraft>,
}

#[derive(Debug, Deserialize)]
struct Verdicts {
    verdicts: Vec<Verdict>,
}

#[derive(Debug, Deserialize)]
struct Verdict {
    index: usize,
    measurable: bool,
    #[serde(default)]
    reason: String,
}

/// A statement that is not measurable and why.
type Rejection = (String, String);

/// The Definición y restricciones node.
#[derive(Debug, Default)]
pub struct Definition;

impl Definition {
    /// The node.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ResearchNode for Definition {
    fn stage(&self) -> Stage {
        Stage::Definition
    }

    async fn run(&self, ctx: &mut NodeContext) -> Result<NodeOutcome, NodeError> {
        if ctx.state().evidence.is_empty() {
            return Ok(back(
                ctx,
                Stage::Data,
                "no hay evidencia para definir el problema",
            ));
        }
        if ctx.state().hypotheses.is_empty() {
            return Ok(back(
                ctx,
                Stage::Hypotheses,
                "no hay hipótesis en las que basar la definición",
            ));
        }
        let base = context_prompt(ctx);
        let mut draft = ask_draft(ctx, &base).await?;
        let mut rejected = rejections(ctx, &draft).await?;
        if !rejected.is_empty() {
            ctx.log(format!(
                "Definiciones no medibles, se piden de nuevo:\n{}",
                list(&rejected)
            ));
            let retry = format!(
                "{base}\n\nEstas definiciones no son medibles; reescríbelas:\n{}",
                list(&rejected)
            );
            draft = ask_draft(ctx, &retry).await?;
            rejected = rejections(ctx, &draft).await?;
        }
        if !rejected.is_empty() {
            ctx.log(format!(
                "Sigue sin haber una definición medible; se pregunta al usuario:\n{}",
                list(&rejected)
            ));
            return Ok(NodeOutcome::NeedsInput {
                prompt: format!(
                    "No logré una definición medible del problema. Falta:\n{}\nIndica qué se \
                     observa y cuánto (número y unidad, p. ej. «a 500 requests por segundo el \
                     tiempo de respuesta sube de 10 ms a 500 ms») o una condición que se pueda \
                     comprobar (p. ej. «al ejecutar X, el test Y falla»).",
                    list(&rejected)
                ),
            });
        }
        save(ctx, draft)?;
        Ok(NodeOutcome::Done)
    }
}

fn back(ctx: &mut NodeContext, to: Stage, reason: &str) -> NodeOutcome {
    ctx.log(format!("Regresa a {}: {reason}.", to.name_es()));
    NodeOutcome::Back {
        to,
        reason: reason.into(),
    }
}

/// The problem, hypotheses, evidence and previous definition, for the
/// draft prompt.
fn context_prompt(ctx: &NodeContext) -> String {
    let state = ctx.state();
    let mut out = format!("Pedido del usuario: {}\n\nHipótesis:\n", ctx.problem());
    for h in &state.hypotheses {
        let _ = writeln!(
            out,
            "- {} (prioridad {}, {:?}): {}",
            h.id, h.priority, h.status, h.statement
        );
    }
    out.push_str("\nEvidencia:\n");
    for e in &state.evidence {
        let _ = writeln!(out, "- {}: {}", e.id, e.claim);
    }
    if !state.problem_statements.is_empty() {
        out.push_str("\nDefinición anterior:\n");
        for p in &state.problem_statements {
            let _ = writeln!(out, "- {p}");
        }
    }
    if !state.constraints.is_empty() {
        out.push_str("\nRestricciones anteriores (consérvalas o corrígelas):\n");
        for c in &state.constraints {
            let _ = writeln!(out, "- {} ({:?}): {}", c.id, c.kind, c.statement);
        }
    }
    if let Some(reply) = ctx.user_reply() {
        let _ = write!(out, "\nIndicaciones del usuario: {reply}\n");
    }
    out
}

async fn ask_draft(ctx: &NodeContext, prompt: &str) -> Result<Draft, NodeError> {
    let state = ctx.state();
    ctx.ask_json(DRAFT_SYSTEM, prompt, |d: &Draft| {
        if d.problems.is_empty() {
            return Err("`problems` está vacío: hace falta al menos una definición".into());
        }
        if d.problems.iter().any(|p| p.statement.trim().is_empty()) {
            return Err("hay una definición vacía en `problems`".into());
        }
        if d.constraints.iter().any(|c| c.statement.trim().is_empty()) {
            return Err("hay una restricción vacía en `constraints`".into());
        }
        let cited: Vec<EvidenceId> = d
            .problems
            .iter()
            .flat_map(|p| p.evidence_ids.iter().copied())
            .collect();
        state.cites_only_known_evidence(&cited).map_err(|unknown| {
            let ids: Vec<String> = unknown.iter().map(ToString::to_string).collect();
            format!("cita evidencia que no existe: {}", ids.join(", "))
        })
    })
    .await
}

/// The statements of `draft` that are not measurable: by the heuristic,
/// and then by the model for those the heuristic accepts.
async fn rejections(ctx: &NodeContext, draft: &Draft) -> Result<Vec<Rejection>, NodeError> {
    let mut rejected = Vec::new();
    let mut candidates = Vec::new();
    for p in &draft.problems {
        match measurable::check(&p.statement) {
            Ok(_) => candidates.push(p.statement.clone()),
            Err(e) => rejected.push((p.statement.clone(), e.reason)),
        }
    }
    if candidates.is_empty() {
        return Ok(rejected);
    }
    let mut prompt = String::from("Definiciones:\n");
    for (i, s) in candidates.iter().enumerate() {
        let _ = writeln!(prompt, "{i}. {s}");
    }
    let count = candidates.len();
    let verdicts: Verdicts = ctx
        .ask_json(JUDGE_SYSTEM, &prompt, |v: &Verdicts| {
            if (0..count).all(|i| v.verdicts.iter().any(|x| x.index == i)) {
                Ok(())
            } else {
                Err(format!(
                    "falta un veredicto por cada índice de 0 a {}",
                    count - 1
                ))
            }
        })
        .await?;
    for (i, statement) in candidates.into_iter().enumerate() {
        let verdict = verdicts.verdicts.iter().find(|v| v.index == i);
        if let Some(v) = verdict.filter(|v| !v.measurable) {
            let reason = if v.reason.trim().is_empty() {
                "el modelo la juzgó no medible".to_owned()
            } else {
                v.reason.trim().to_owned()
            };
            rejected.push((statement, reason));
        }
    }
    Ok(rejected)
}

fn list(rejected: &[Rejection]) -> String {
    rejected
        .iter()
        .map(|(s, r)| format!("- «{s}»: {r}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Writes the definition to the state (replacing the previous one) and
/// shows it.
fn save(ctx: &mut NodeContext, draft: Draft) -> Result<(), NodeError> {
    let state = ctx.state_mut();
    state.problem_statements = draft.problems.iter().map(|p| p.statement.clone()).collect();
    state.constraints.clear();
    for c in draft.constraints {
        state.add_constraint(c.kind, c.statement, c.test);
    }
    let mut markdown = String::from("## Problema\n\n");
    for (i, p) in draft.problems.iter().enumerate() {
        let _ = write!(markdown, "{}. {}", i + 1, p.statement);
        if !p.evidence_ids.is_empty() {
            let ids: Vec<String> = p.evidence_ids.iter().map(ToString::to_string).collect();
            let _ = write!(markdown, " [{}]", ids.join(", "));
        }
        markdown.push('\n');
    }
    ctx.emit(BlockKind::Markdown {
        title: Some("Definición del problema".into()),
        source: markdown,
    })?;
    ctx.emit(BlockKind::Table(constraint_table(ctx)))?;
    ctx.set_data(PROBLEMS_KEY, &draft.problems)?;
    let summary = format!(
        "{} definición(es) medible(s) y {} restricción(es):\n{}",
        draft.problems.len(),
        ctx.state().constraints.len(),
        draft
            .problems
            .iter()
            .map(|p| format!("- {}", p.statement))
            .collect::<Vec<_>>()
            .join("\n")
    );
    ctx.log(summary);
    Ok(())
}

fn constraint_table(ctx: &NodeContext) -> TableSpec {
    let rows = ctx
        .state()
        .constraints
        .iter()
        .map(|c| {
            let test = c.test.as_ref().map_or(Cell::Null, |t| {
                let kind = match t.kind {
                    TestKind::Test => "test",
                    TestKind::Command => "comando",
                };
                Cell::Text(format!("{kind}: {}", t.reference))
            });
            vec![
                Cell::Text(c.id.to_string()),
                Cell::Text(kind_es(c.kind).into()),
                Cell::Text(c.statement.clone()),
                test,
            ]
        })
        .collect();
    TableSpec {
        columns: ["Id", "Tipo", "Restricción", "Prueba"]
            .into_iter()
            .map(Column::new)
            .collect(),
        rows,
    }
}

fn kind_es(kind: ConstraintKind) -> &'static str {
    match kind {
        ConstraintKind::Platform => "plataforma",
        ConstraintKind::Performance => "rendimiento",
        ConstraintKind::Compatibility => "compatibilidad",
        ConstraintKind::Security => "seguridad",
        ConstraintKind::Other => "otra",
    }
}

#[cfg(test)]
mod tests;
