//! Node 8, Investigación: prior solutions to the defined problem.
//!
//! Two sources, both with the `explore` role:
//!
//! - **the repo**: a turn of the agent ([`NodeContext::agent`]) told to
//!   only read, looking for patterns and pieces already present; each
//!   finding becomes evidence with its file and lines;
//! - **the web**: one request with the provider's web search and fetch
//!   tools, looking for libraries, articles and issues; every citation
//!   becomes evidence ([`citations_to_evidence`]), and so does the page of
//!   each option that no citation covers.
//!
//! The options found are shown as a table with their sources and stored
//! under [`OPTIONS_KEY`] for the brainstorm and the modeling. Without a
//! problem definition the flow goes back to it.

use std::fmt::Write as _;

use async_trait::async_trait;
use domain::block::{BlockKind, Cell, Column, SourceRef, TableSpec};
use domain::conversation::{ContentPart, Message, Role, Session};
use domain::research::{EvidenceId, Stage};
use futures::StreamExt;
use serde::{Deserialize, Serialize};

use super::define::json;
use crate::flows::research::node::{NodeContext, NodeError, NodeOutcome, ResearchNode};
use crate::ports::llm::{Citation, LlmError, MessageAssembler, ServerTool};
use crate::usecases::evidence::citations_to_evidence;

/// Key of the prior solutions found (`Vec<PriorSolution>`).
pub const OPTIONS_KEY: &str = "investigation.options";

/// Searches the web may run in the request.
const WEB_SEARCHES: u32 = 5;
/// Pages the web fetch may read in the request.
const WEB_FETCHES: u32 = 3;

const REPO_PROMPT: &str = "\
Eres el nodo «Investigación» de una investigación sobre este repositorio. SOLO PUEDES LEER: \
usa list_files, find_file, grep, tree y read_file; no edites, no escribas archivos ni ejecutes \
comandos. Busca en el repo soluciones previas al problema: patrones, módulos, utilidades o \
dependencias ya presentes que lo resuelvan en parte o sirvan de modelo.
Al terminar responde solo con JSON:
{\"findings\": [{\"name\": \"…\", \"summary\": \"una línea\", \"path\": \"ruta/relativa.rs\", \
\"line_start\": 10, \"line_end\": 40}]}
line_start y line_end pueden ser null. Si no hay nada, devuelve \"findings\": [].";

const WEB_SYSTEM: &str = "\
Eres el nodo «Investigación» de una investigación sobre un proyecto de software. Busca en la web \
soluciones previas al problema: librerías, artículos e issues de proyectos parecidos. Cita las \
fuentes. No inventes URLs: incluye solo páginas que hayas encontrado.
Al terminar responde solo con JSON:
{\"options\": [{\"name\": \"…\", \"kind\": \"library\", \"summary\": \"una línea\", \
\"url\": \"https://…\"}]}
kind es library, article, issue o pattern. Si no hay nada, devuelve \"options\": [].";

/// What kind of prior solution an option is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SolutionKind {
    /// A library or crate.
    Library,
    /// An article, post or documentation page.
    Article,
    /// An issue or discussion of another project.
    Issue,
    /// A pattern, already in the repo or described elsewhere.
    Pattern,
}

impl SolutionKind {
    fn name_es(self) -> &'static str {
        match self {
            Self::Library => "librería",
            Self::Article => "artículo",
            Self::Issue => "issue",
            Self::Pattern => "patrón",
        }
    }
}

/// A prior solution found by the node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PriorSolution {
    /// Short name.
    pub name: String,
    /// Kind of solution.
    pub kind: SolutionKind,
    /// What it does, in one line.
    pub summary: String,
    /// Where it was found.
    pub source: SourceRef,
    /// Evidence that backs it.
    pub evidence_ids: Vec<EvidenceId>,
}

#[derive(Debug, Deserialize)]
struct RepoAnswer {
    findings: Vec<RepoFinding>,
}

#[derive(Debug, Deserialize)]
struct RepoFinding {
    name: String,
    summary: String,
    path: String,
    #[serde(default)]
    line_start: Option<u32>,
    #[serde(default)]
    line_end: Option<u32>,
}

impl RepoFinding {
    fn source(&self) -> SourceRef {
        SourceRef::File {
            path: self.path.clone(),
            line_start: self.line_start,
            line_end: self.line_end,
        }
    }
}

#[derive(Debug, Deserialize)]
struct WebAnswer {
    options: Vec<WebOption>,
}

#[derive(Debug, Deserialize)]
struct WebOption {
    name: String,
    kind: SolutionKind,
    summary: String,
    url: String,
}

/// The Investigación node.
#[derive(Debug, Default)]
pub struct Research;

impl Research {
    /// The node.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ResearchNode for Research {
    fn stage(&self) -> Stage {
        Stage::Research
    }

    async fn run(&self, ctx: &mut NodeContext) -> Result<NodeOutcome, NodeError> {
        if ctx.state().problem_statements.is_empty() {
            let reason = "no hay una definición del problema que investigar";
            ctx.log(format!("Regresa a Definición: {reason}."));
            return Ok(NodeOutcome::Back {
                to: Stage::Definition,
                reason: reason.into(),
            });
        }
        let brief = brief(ctx);
        let first_evidence = ctx.state().evidence.len();
        let mut options = repo_options(ctx, &brief).await?;
        let repo_count = options.len();
        options.extend(web_options(ctx, &brief).await?);

        ctx.emit(BlockKind::Table(table(&options)))?;
        let sources: Vec<SourceRef> = ctx.state().evidence[first_evidence..]
            .iter()
            .map(|e| e.source.clone())
            .collect();
        if !sources.is_empty() {
            ctx.emit(BlockKind::Sources { sources })?;
        }
        ctx.set_data(OPTIONS_KEY, &options)?;
        let new_evidence = ctx.state().evidence.len() - first_evidence;
        ctx.log(format!(
            "{} solución(es) previa(s): {repo_count} en el repo y {} en la web; {new_evidence} \
             evidencia(s) nueva(s).",
            options.len(),
            options.len() - repo_count,
        ));
        Ok(NodeOutcome::Done)
    }
}

/// The definition, constraints and hypotheses, for both prompts.
fn brief(ctx: &NodeContext) -> String {
    let state = ctx.state();
    let mut out = String::from("Problema:\n");
    for p in &state.problem_statements {
        let _ = writeln!(out, "- {p}");
    }
    if !state.constraints.is_empty() {
        out.push_str("\nRestricciones:\n");
        for c in &state.constraints {
            let _ = writeln!(out, "- {}: {}", c.id, c.statement);
        }
    }
    if !state.hypotheses.is_empty() {
        out.push_str("\nHipótesis:\n");
        for h in &state.hypotheses {
            let _ = writeln!(out, "- {}: {}", h.id, h.statement);
        }
    }
    out
}

/// Prior solutions already in the repo, from a read-only agent turn.
async fn repo_options(ctx: &mut NodeContext, brief: &str) -> Result<Vec<PriorSolution>, NodeError> {
    let Ok(agent) = ctx.agent().cloned() else {
        ctx.log("Sin agente con tools: no se exploró el repo.");
        return Ok(Vec::new());
    };
    let prompt = format!("{REPO_PROMPT}\n\n{brief}");
    let mut session = Session::new(".", ctx.now());
    agent
        .run_turn(&mut session, prompt.clone(), ctx.turn_options())
        .await?;
    let report = final_text(&session);
    let answer: RepoAnswer = json::fix_json(
        ctx,
        "",
        vec![Message::user_text(prompt)],
        report,
        |a: &RepoAnswer| {
            a.findings.iter().try_for_each(|f| {
                if f.name.trim().is_empty() || f.path.trim().is_empty() {
                    return Err("cada hallazgo necesita `name` y `path`".to_owned());
                }
                f.source().validate().map_err(|e| e.to_string())
            })
        },
    )
    .await?;
    let now = ctx.now();
    Ok(answer
        .findings
        .into_iter()
        .map(|f| {
            let source = f.source();
            let id = ctx
                .state_mut()
                .add_evidence(f.summary.clone(), source.clone(), now);
            PriorSolution {
                name: f.name,
                kind: SolutionKind::Pattern,
                summary: f.summary,
                source,
                evidence_ids: vec![id],
            }
        })
        .collect())
}

/// Libraries, articles and issues from the web, with their citations.
async fn web_options(ctx: &mut NodeContext, brief: &str) -> Result<Vec<PriorSolution>, NodeError> {
    let messages = vec![Message::user_text(brief)];
    let mut req = ctx.request(WEB_SYSTEM, messages.clone());
    req.server_tools = vec![
        ServerTool::WebSearch {
            max_uses: Some(WEB_SEARCHES),
        },
        ServerTool::WebFetch {
            max_uses: Some(WEB_FETCHES),
        },
    ];
    let (text, citations) = complete_with_citations(ctx, req).await?;
    let now = ctx.now();
    let cited = citations_to_evidence(&citations, ctx.state_mut(), now);
    let answer: WebAnswer = json::fix_json(ctx, WEB_SYSTEM, messages, text, |a: &WebAnswer| {
        a.options.iter().try_for_each(|o| {
            if o.name.trim().is_empty() || o.summary.trim().is_empty() {
                Err("cada opción necesita `name` y `summary`".to_owned())
            } else if !o.url.starts_with("http") {
                Err(format!("la URL de «{}» no es válida: {:?}", o.name, o.url))
            } else {
                Ok(())
            }
        })
    })
    .await?;
    let mut options = Vec::new();
    for o in answer.options {
        let mut ids: Vec<EvidenceId> = cited
            .iter()
            .copied()
            .filter(|id| {
                ctx.state().evidence(*id).is_some_and(
                    |e| matches!(&e.source, SourceRef::Url { url, .. } if *url == o.url),
                )
            })
            .collect();
        let source = SourceRef::Url {
            url: o.url.clone(),
            title: Some(o.name.clone()),
        };
        if ids.is_empty() {
            ids.push(
                ctx.state_mut()
                    .add_evidence(o.summary.clone(), source.clone(), now),
            );
        }
        options.push(PriorSolution {
            name: o.name,
            kind: o.kind,
            summary: o.summary,
            source,
            evidence_ids: ids,
        });
    }
    Ok(options)
}

/// Sends `req` and returns its text, trimmed, and its citations.
async fn complete_with_citations(
    ctx: &NodeContext,
    req: crate::ports::llm::LlmRequest,
) -> Result<(String, Vec<Citation>), NodeError> {
    let mut stream = ctx.llm().stream(req).await?;
    let mut assembler = MessageAssembler::new();
    while let Some(event) = stream.next().await {
        assembler
            .push(event?)
            .map_err(|e| LlmError::Decode(e.to_string()))?;
    }
    let assembled = assembler
        .finish()
        .map_err(|e| LlmError::Decode(e.to_string()))?;
    let text = text_of(&assembled.message.content);
    if text.is_empty() {
        return Err(NodeError::EmptyAnswer);
    }
    Ok((text, assembled.citations))
}

fn text_of(parts: &[ContentPart]) -> String {
    parts
        .iter()
        .filter_map(|part| match part {
            ContentPart::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<String>()
        .trim()
        .to_owned()
}

/// Text of the last assistant message of the session's last turn.
fn final_text(session: &Session) -> String {
    session
        .turns
        .last()
        .and_then(|turn| {
            turn.messages
                .iter()
                .rev()
                .find(|m| m.role == Role::Assistant)
        })
        .map(|m| text_of(&m.content))
        .unwrap_or_default()
}

fn source_label(source: &SourceRef) -> String {
    match source {
        SourceRef::File {
            path,
            line_start: Some(start),
            line_end: Some(end),
        } => format!("{path}:{start}-{end}"),
        SourceRef::File {
            path,
            line_start: Some(start),
            ..
        } => format!("{path}:{start}"),
        SourceRef::File { path, .. } => path.clone(),
        SourceRef::Url { url, .. } => url.clone(),
        SourceRef::Commit { sha, .. } => sha.clone(),
        SourceRef::Command { command, .. } => command.clone(),
    }
}

fn table(options: &[PriorSolution]) -> TableSpec {
    TableSpec {
        columns: ["Opción", "Tipo", "Resumen", "Fuente", "Evidencia"]
            .into_iter()
            .map(Column::new)
            .collect(),
        rows: options
            .iter()
            .map(|o| {
                let ids: Vec<String> = o.evidence_ids.iter().map(ToString::to_string).collect();
                vec![
                    Cell::Text(o.name.clone()),
                    Cell::Text(o.kind.name_es().into()),
                    Cell::Text(o.summary.clone()),
                    Cell::Text(source_label(&o.source)),
                    Cell::Text(ids.join(", ")),
                ]
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests;
