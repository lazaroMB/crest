//! Stage 2, Observation: walks the repository with the read-only agent
//! (tree, key files, tests, docs and, when the shell is available,
//! `git log`) and draws the current architecture.
//!
//! Each observation becomes an [`Evidence`](domain::research::Evidence)
//! with a file or commit source. The node shows a `Mermaid` diagram of the
//! architecture, a `Table` of key files and a `Sources` block, and stores
//! an [`ObservationReport`] under [`REPORT_KEY`].
//!
//! Without the brief of Intake there is no problem to observe, so the node
//! goes back there.
//!
//! [`explore_json`] (one agent turn that must end in a JSON object) is
//! shared with the Data node.

use std::fmt::Write as _;

use async_trait::async_trait;
use domain::block::{BlockKind, Cell, Column, MermaidSpec, SourceRef, TableSpec};
use domain::conversation::{ContentPart, Role, Session};
use domain::research::{EvidenceId, Stage};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use super::intake::{BRIEF_KEY, IntakeBrief, invalid_answer, parse_valid, retry_message};
use crate::flows::research::node::{NodeContext, NodeError, NodeOutcome, ResearchNode};

/// Flow data key of the [`ObservationReport`].
pub const REPORT_KEY: &str = "observation.report";

const TASK: &str = "Etapa de observación de una investigación sobre este repositorio. \
Recorre el repositorio SOLO LEYENDO, sin modificar nada: el árbol (`tree`), los archivos \
clave relacionados con el problema, sus tests y la documentación. No propongas soluciones: \
describe lo que hay.";

const GIT: &str = "Puedes usar `run_command` solo con comandos git de lectura, por ejemplo \
`git log --oneline -n 20` o `git log --format=%H%x09%s -n 10 -- <archivo>`, para ver los \
cambios recientes de los archivos implicados.";

const NO_GIT: &str = "No tienes acceso a git: no cites commits.";

const FORMAT: &str = "Al terminar, responde solo con un objeto JSON, sin texto alrededor:
{\"summary\": \"resumen de la arquitectura en 2-3 oraciones\",
 \"architecture\": \"diagrama Mermaid (flowchart) de los módulos actuales y sus relaciones\",
 \"key_files\": [{\"path\": \"ruta/relativa\", \"kind\": \"código\" | \"test\" | \"doc\" | \"config\", \
\"role\": \"qué hace\"}],
 \"observations\": [{\"claim\": \"lo observado, en una oración\", \"source\": \
{\"type\": \"file\", \"path\": \"ruta\", \"line_start\": 1, \"line_end\": 20} | \
{\"type\": \"commit\", \"sha\": \"…\", \"summary\": \"…\"}}]}
Cada observación debe citar un archivo que leíste o un commit que viste.";

/// What Observation learned, read by the later stages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservationReport {
    /// The current architecture in two or three sentences.
    pub summary: String,
    /// Mermaid source of the current architecture.
    pub architecture: String,
    /// Files and modules involved in the problem.
    pub key_files: Vec<KeyFile>,
    /// Evidence recorded by the stage.
    pub evidence: Vec<EvidenceId>,
}

/// A file or module involved in the problem.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyFile {
    /// Path relative to the workspace.
    pub path: String,
    /// `código`, `test`, `doc` or `config`.
    #[serde(default)]
    pub kind: String,
    /// What it does.
    pub role: String,
}

/// One observation of the agent.
#[derive(Debug, Deserialize)]
struct Observed {
    claim: String,
    source: SourceRef,
}

/// What the agent answers.
#[derive(Debug, Deserialize)]
struct Answer {
    summary: String,
    architecture: String,
    key_files: Vec<KeyFile>,
    observations: Vec<Observed>,
}

impl Answer {
    fn validate(&self) -> Result<(), String> {
        if self.summary.trim().is_empty() {
            return Err("`summary` está vacío".into());
        }
        if mermaid_source(&self.architecture).is_empty() {
            return Err("`architecture` no tiene un diagrama Mermaid".into());
        }
        if self.key_files.is_empty() || self.key_files.iter().any(|f| f.path.trim().is_empty()) {
            return Err("`key_files` está vacío o tiene una ruta vacía".into());
        }
        if self.observations.is_empty() {
            return Err("`observations` está vacío".into());
        }
        for (i, o) in self.observations.iter().enumerate() {
            if o.claim.trim().is_empty() {
                return Err(format!("la observación {i} no tiene `claim`"));
            }
            validate_source(&o.source).map_err(|e| format!("la observación {i}: {e}"))?;
        }
        Ok(())
    }
}

/// Checks that a cited source names something.
pub(crate) fn validate_source(source: &SourceRef) -> Result<(), String> {
    source.validate().map_err(|e| e.to_string())?;
    let empty = match source {
        SourceRef::File { path, .. } => path.trim().is_empty(),
        SourceRef::Commit { sha, .. } => sha.trim().is_empty(),
        SourceRef::Url { url, .. } => url.trim().is_empty(),
        SourceRef::Command { command, .. } => command.trim().is_empty(),
    };
    if empty {
        return Err("la fuente está vacía".into());
    }
    Ok(())
}

/// The diagram without the code fence the model may wrap it in.
fn mermaid_source(text: &str) -> String {
    let trimmed = text.trim();
    let Some(rest) = trimmed.strip_prefix("```") else {
        return trimmed.to_owned();
    };
    let body = rest.split_once('\n').map_or("", |(_, body)| body);
    body.trim_end()
        .strip_suffix("```")
        .unwrap_or(body)
        .trim()
        .to_owned()
}

/// The text of the last assistant message of `session`.
fn last_answer(session: &Session) -> String {
    session
        .turns
        .last()
        .and_then(|turn| {
            turn.messages
                .iter()
                .rev()
                .find(|m| m.role == Role::Assistant)
        })
        .map(|m| {
            m.content
                .iter()
                .filter_map(|part| match part {
                    ContentPart::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Runs one turn of the agent with `input` and parses its last answer as
/// a JSON object of type `T`.
///
/// An answer that does not parse or `validate` gets a second turn in the
/// same session with the error; a second failure is a
/// [`NodeError::Failed`].
///
/// # Errors
///
/// [`NodeError::Unavailable`] without an agent, the turn's errors, or the
/// invalid answer.
pub(crate) async fn explore_json<T: DeserializeOwned>(
    ctx: &NodeContext,
    input: &str,
    validate: impl Fn(&T) -> Result<(), String>,
) -> Result<T, NodeError> {
    let agent = ctx.agent()?;
    let mut session = Session::new(".", ctx.now());
    agent
        .run_turn(&mut session, input, ctx.turn_options())
        .await?;
    let error = match parse_valid(&last_answer(&session), &validate) {
        Ok(value) => return Ok(value),
        Err(error) => error,
    };
    agent
        .run_turn(&mut session, retry_message(&error), ctx.turn_options())
        .await?;
    parse_valid(&last_answer(&session), &validate).map_err(|e| invalid_answer(&e))
}

/// Whether the agent can run read-only git commands.
pub(crate) fn has_shell(ctx: &NodeContext) -> bool {
    ctx.agent()
        .is_ok_and(|agent| agent.tools().get("run_command").is_some())
}

/// The `Sources` block of `sources`, without repeats.
pub(crate) fn sources_block(sources: impl IntoIterator<Item = SourceRef>) -> BlockKind {
    let mut unique: Vec<SourceRef> = Vec::new();
    for source in sources {
        if !unique.contains(&source) {
            unique.push(source);
        }
    }
    BlockKind::Sources { sources: unique }
}

/// The Observation node.
#[derive(Debug, Default)]
pub struct Observation;

impl Observation {
    /// The node.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

fn input(problem: &str, brief: &IntakeBrief, git: bool) -> String {
    let mut out = format!(
        "{TASK}\n\nProblema ({}): {}\nPedido original: {problem}\n",
        brief.kind.name_es(),
        brief.restated
    );
    for answer in &brief.answers {
        let _ = writeln!(out, "Aclaración del usuario: {answer}");
    }
    let _ = write!(out, "\n{}\n\n{FORMAT}", if git { GIT } else { NO_GIT });
    out
}

fn key_files_table(files: &[KeyFile]) -> TableSpec {
    TableSpec {
        columns: vec![
            Column::new("Archivo o módulo"),
            Column::new("Tipo"),
            Column::new("Qué hace"),
        ],
        rows: files
            .iter()
            .map(|f| {
                vec![
                    Cell::Text(f.path.clone()),
                    Cell::Text(f.kind.clone()),
                    Cell::Text(f.role.clone()),
                ]
            })
            .collect(),
    }
}

#[async_trait]
impl ResearchNode for Observation {
    fn stage(&self) -> Stage {
        Stage::Observation
    }

    async fn run(&self, ctx: &mut NodeContext) -> Result<NodeOutcome, NodeError> {
        let Some(brief) = ctx.data::<IntakeBrief>(BRIEF_KEY) else {
            return Ok(NodeOutcome::Back {
                to: Stage::Intake,
                reason: "el problema no está definido: falta el pedido reformulado de Intake"
                    .into(),
            });
        };
        let git = has_shell(ctx);
        let answer: Answer =
            explore_json(ctx, &input(ctx.problem(), &brief, git), Answer::validate).await?;

        let now = ctx.now();
        let mut sources = Vec::with_capacity(answer.observations.len());
        let mut evidence = Vec::with_capacity(answer.observations.len());
        for observed in answer.observations {
            sources.push(observed.source.clone());
            evidence.push(ctx.state_mut().add_evidence(
                observed.claim.trim(),
                observed.source,
                now,
            ));
        }
        let report = ObservationReport {
            summary: answer.summary.trim().to_owned(),
            architecture: mermaid_source(&answer.architecture),
            key_files: answer.key_files,
            evidence,
        };
        ctx.set_data(REPORT_KEY, &report)?;

        ctx.emit(BlockKind::Mermaid(MermaidSpec {
            source: report.architecture.clone(),
            title: Some("Arquitectura actual".into()),
        }))?;
        ctx.emit(BlockKind::Table(key_files_table(&report.key_files)))?;
        ctx.emit(sources_block(sources))?;

        let ids: Vec<String> = report.evidence.iter().map(ToString::to_string).collect();
        ctx.log(format!(
            "Repositorio recorrido{}: {} archivos clave, evidencia {}. {}",
            if git { " (con git log)" } else { "" },
            report.key_files.len(),
            ids.join(", "),
            report.summary
        ));
        Ok(NodeOutcome::Done)
    }
}

/// An agent over an in-memory workspace, for the tests of the nodes that
/// explore.
#[cfg(test)]
pub(crate) mod testing {
    use std::sync::{Arc, Mutex};

    use domain::model::ModelPolicy;
    use domain::permission::PermissionPolicy;
    use domain::research::ResearchState;
    use domain::workspace::WorkspaceRoot;

    use super::*;
    use crate::flows::research::node::NodeDeps;
    use crate::ports::events::DomainEvent;
    use crate::testing::fs::{InMemoryFs, ScriptedShell};
    use crate::testing::llm::ScriptedLlm;
    use crate::testing::system::{RecordingSink, RecordingTelemetry, ScriptedApproval};
    use crate::tools::files::{FileToolPorts, register_file_tools};
    use crate::tools::registry::ToolRegistry;
    use crate::usecases::run_turn::{AgentDeps, AgentService};

    /// A workspace with a small crate and its docs.
    pub(crate) fn workspace() -> Arc<InMemoryFs> {
        Arc::new(
            InMemoryFs::new(WorkspaceRoot::new("/ws").unwrap())
                .with_file("Cargo.toml", "[workspace]\n")
                .with_file("src/login.rs", "pub fn login() {\n    hash();\n}\n")
                .with_file("tests/login.rs", "#[test]\nfn logs_in() {}\n")
                .with_file("README.md", "# Demo\n"),
        )
    }

    /// The agent's tools over `fs`, with `shell` or without one.
    pub(crate) fn agent(
        llm: &Arc<ScriptedLlm>,
        fs: &Arc<InMemoryFs>,
        shell: Option<Arc<ScriptedShell>>,
        extra: impl FnOnce(&mut ToolRegistry),
    ) -> Arc<AgentService> {
        let mut tools = ToolRegistry::new();
        if let Some(shell) = shell {
            register_file_tools(
                &mut tools,
                FileToolPorts {
                    file_search: fs.clone(),
                    content_search: fs.clone(),
                    fs: fs.clone(),
                    shell,
                },
            )
            .unwrap();
        } else {
            use crate::tools::files::{ReadFileTool, TreeTool};
            let budget = tools.budget();
            tools
                .register_typed(TreeTool::new(fs.clone(), budget))
                .unwrap();
            tools
                .register_typed(ReadFileTool::new(fs.clone(), budget))
                .unwrap();
        }
        extra(&mut tools);
        let deps = AgentDeps {
            llm: llm.clone(),
            events: Arc::new(RecordingSink::new()),
            approval: Arc::new(ScriptedApproval::new()),
            telemetry: Arc::new(RecordingTelemetry::new()),
        };
        let permissions = Arc::new(Mutex::new(PermissionPolicy::read_only()));
        Arc::new(AgentService::new(
            deps,
            tools,
            ModelPolicy::default(),
            permissions,
        ))
    }

    /// A context of `stage` whose blocks go to `sink`.
    pub(crate) fn context(
        stage: Stage,
        llm: &Arc<ScriptedLlm>,
        sink: &Arc<RecordingSink>,
        agent: Option<Arc<AgentService>>,
    ) -> NodeContext {
        let mut deps =
            NodeDeps::new(llm.clone(), ModelPolicy::default(), sink.clone()).with_clock(|| 9);
        if let Some(agent) = agent {
            deps = deps.with_agent(agent);
        }
        NodeContext::new(
            stage,
            deps,
            "el login tarda mucho",
            ResearchState::default(),
        )
    }

    /// The brief Intake leaves.
    pub(crate) fn brief() -> IntakeBrief {
        IntakeBrief {
            kind: super::super::intake::RequestKind::Performance,
            restated: "El login tarda más de 5 s.".into(),
            questions: vec![],
            answers: vec!["desde la v2".into()],
        }
    }

    /// The blocks the node emitted.
    pub(crate) fn blocks(sink: &RecordingSink) -> Vec<BlockKind> {
        sink.events()
            .into_iter()
            .filter_map(|e| match e {
                DomainEvent::BlockAdded { block } => Some(block.kind),
                _ => None,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use domain::model::ModelRole;
    use serde_json::json;

    use super::testing::{agent, blocks, brief, context, workspace};
    use super::*;
    use crate::ports::shell::CommandOutput;
    use crate::testing::fs::ScriptedShell;
    use crate::testing::llm::{ScriptedLlm, ScriptedResponse};
    use crate::testing::system::RecordingSink;

    fn answer() -> String {
        json!({
            "summary": "Un crate con el login y sus tests.",
            "architecture": "```mermaid\nflowchart TD\n  login --> hash\n```",
            "key_files": [
                {"path": "src/login.rs", "kind": "código", "role": "inicia sesión"},
                {"path": "tests/login.rs", "kind": "test", "role": "prueba el login"}
            ],
            "observations": [
                {"claim": "login llama a hash en cada intento.",
                 "source": {"type": "file", "path": "src/login.rs", "line_start": 1, "line_end": 3}},
                {"claim": "El último cambio tocó el hash.",
                 "source": {"type": "commit", "sha": "abc123", "summary": "usa argon2"}}
            ]
        })
        .to_string()
    }

    #[tokio::test]
    async fn explores_and_records_evidence_and_blocks() {
        let llm = Arc::new(ScriptedLlm::new([
            ScriptedResponse::tool_use("t1", "tree", json!({})),
            ScriptedResponse::tool_use("t2", "read_file", json!({"path": "src/login.rs"})),
            ScriptedResponse::tool_use(
                "t3",
                "run_command",
                json!({"command": "git", "args": ["log", "--oneline", "-n", "20"]}),
            ),
            ScriptedResponse::text(answer()),
        ]));
        let shell = Arc::new(ScriptedShell::new().on(
            "git",
            ["log", "--oneline", "-n", "20"],
            CommandOutput {
                status: Some(0),
                stdout: "abc123 usa argon2\n".into(),
                ..CommandOutput::default()
            },
        ));
        let service = agent(&llm, &workspace(), Some(shell.clone()), |_| {});
        let sink = Arc::new(RecordingSink::new());
        let mut ctx = context(Stage::Observation, &llm, &sink, Some(service));
        ctx.set_data(BRIEF_KEY, brief()).unwrap();

        let outcome = Observation::new().run(&mut ctx).await.unwrap();

        assert_eq!(outcome, NodeOutcome::Done);
        assert_eq!(shell.calls().len(), 1);
        let first = &llm.requests()[0];
        assert_eq!(first.model.role, ModelRole::Explore);
        let sent = format!("{:?}", first.messages);
        assert!(sent.contains("El login tarda más de 5 s.") && sent.contains("run_command"));

        let state = ctx.state();
        assert_eq!(state.evidence.len(), 2);
        assert!(
            matches!(&state.evidence[0].source, SourceRef::File { path, .. } if path == "src/login.rs")
        );
        assert!(
            matches!(&state.evidence[1].source, SourceRef::Commit { sha, .. } if sha == "abc123")
        );
        assert_eq!(state.evidence[0].collected_at, 9);
        assert!(state.journal[0].text.contains("E1, E2"));

        let report: ObservationReport = ctx.data(REPORT_KEY).unwrap();
        assert_eq!(report.architecture, "flowchart TD\n  login --> hash");
        assert_eq!(report.evidence.len(), 2);

        let shown = blocks(&sink);
        assert_eq!(shown.len(), 3, "{shown:#?}");
        assert!(matches!(&shown[0], BlockKind::Mermaid(m) if m.source.starts_with("flowchart")));
        assert!(matches!(&shown[1], BlockKind::Table(t) if t.rows.len() == 2));
        assert!(matches!(&shown[2], BlockKind::Sources { sources } if sources.len() == 2));
    }

    #[tokio::test]
    async fn an_empty_diagram_is_retried_in_the_same_session() {
        let mut bad: serde_json::Value = serde_json::from_str(&answer()).unwrap();
        bad["architecture"] = json!("```mermaid\n```");
        let llm = Arc::new(ScriptedLlm::new([
            ScriptedResponse::text(bad.to_string()),
            ScriptedResponse::text(answer()),
        ]));
        let service = agent(&llm, &workspace(), None, |_| {});
        let sink = Arc::new(RecordingSink::new());
        let mut ctx = context(Stage::Observation, &llm, &sink, Some(service));
        ctx.set_data(BRIEF_KEY, brief()).unwrap();

        Observation::new().run(&mut ctx).await.unwrap();

        let retry = llm.last_request().unwrap();
        let sent = format!("{:?}", retry.messages);
        assert!(sent.contains("architecture") && sent.contains("No tienes acceso a git"));
        assert_eq!(retry.messages.len(), 3);
        assert_eq!(ctx.state().evidence.len(), 2);
    }

    #[tokio::test]
    async fn without_the_intake_brief_it_goes_back() {
        let llm = Arc::new(ScriptedLlm::new([]));
        let sink = Arc::new(RecordingSink::new());
        let mut ctx = context(Stage::Observation, &llm, &sink, None);

        let outcome = Observation::new().run(&mut ctx).await.unwrap();

        assert!(matches!(
            outcome,
            NodeOutcome::Back {
                to: Stage::Intake,
                ..
            }
        ));
        assert!(llm.requests().is_empty());
        assert!(blocks(&sink).is_empty());
    }

    #[test]
    fn mermaid_fences_are_removed() {
        assert_eq!(
            mermaid_source("```mermaid\ngraph LR\nA-->B\n```\n"),
            "graph LR\nA-->B"
        );
        assert_eq!(mermaid_source(" graph LR "), "graph LR");
        assert_eq!(mermaid_source("```\n```"), "");
    }
}
