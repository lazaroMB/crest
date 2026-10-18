//! Stage 3, Data (optional): gathers evidence about the problem.
//!
//! - The read-only agent reads the history of the files Observation marked
//!   as key (`git log` when the shell is available), logs and metrics in
//!   the repository and, when MCP tools (`mcp__*`) are offered, issues.
//! - The stage's model (`explore`, which the adapter gives web search)
//!   looks up the documentation of the libraries involved; its citations
//!   become evidence with
//!   [`citations_to_evidence`](crate::usecases::evidence::citations_to_evidence).
//!
//! The node shows a `Chart` of commits per week when the history has
//! dates, a `Table` of the evidence it collected and a `Sources` block,
//! and stores a [`DataReport`] under [`REPORT_KEY`]. Without the outputs
//! of Intake or Observation it goes back to them.

use std::fmt::Write as _;

use async_trait::async_trait;
use domain::block::{BlockKind, Cell, ChartKind, ChartSpec, Column, Series, SourceRef, TableSpec};
use domain::conversation::Message;
use domain::research::{EvidenceId, Stage};
use futures::StreamExt;
use serde::{Deserialize, Serialize};

use super::intake::{BRIEF_KEY, IntakeBrief};
use super::observation::{
    self, ObservationReport, explore_json, has_shell, sources_block, validate_source,
};
use crate::flows::research::node::{NodeContext, NodeError, NodeOutcome, ResearchNode};
use crate::ports::events::NoticeLevel;
use crate::ports::llm::{Citation, LlmError, MessageAssembler};
use crate::usecases::evidence::citations_to_evidence;

/// Flow data key of the [`DataReport`].
pub const REPORT_KEY: &str = "data.report";

/// Commits recorded as evidence; the chart counts all of them.
const MAX_COMMIT_EVIDENCE: usize = 20;

/// Weeks shown in the chart, the most recent ones.
const MAX_WEEKS: usize = 52;

const TASK: &str = "Etapa de datos de una investigación sobre este repositorio. Reúne \
evidencia sobre el problema SOLO LEYENDO, sin modificar nada: historial de cambios de los \
archivos implicados, logs, métricas o benchmarks que existan en el repositorio y, si hay \
herramientas de issues, los issues relacionados. No propongas soluciones.";

const GIT: &str = "Usa `run_command` solo con comandos git de lectura; para el historial: \
`git log --date=short --format=%H%x09%ad%x09%s -n 50 -- <archivos>`.";

const NO_GIT: &str = "No tienes acceso a git: deja `commits` vacío.";

const FORMAT: &str = "Al terminar, responde solo con un objeto JSON, sin texto alrededor:
{\"commits\": [{\"sha\": \"…\", \"date\": \"AAAA-MM-DD\", \"summary\": \"primera línea\"}],
 \"findings\": [{\"claim\": \"dato encontrado, en una oración\", \"source\": \
{\"type\": \"file\", \"path\": \"…\", \"line_start\": 1} | {\"type\": \"url\", \"url\": \"…\", \
\"title\": \"…\"} | {\"type\": \"command\", \"command\": \"…\", \"output_excerpt\": \"…\"}}],
 \"libraries\": [\"librerías externas implicadas cuya documentación convenga consultar\"]}
Usa listas vacías cuando no haya nada.";

const WEB_SYSTEM: &str = "Investigas un problema de software. Busca en la web la documentación \
oficial, changelogs e issues conocidos de las librerías indicadas que sean relevantes para el \
problema. Responde en 3 a 5 viñetas breves y cita las fuentes.";

/// What Data gathered, read by Analysis.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataReport {
    /// Evidence recorded by the stage.
    pub evidence: Vec<EvidenceId>,
    /// Commits per week: the Monday of each week (`AAAA-MM-DD`) and its
    /// count, oldest first.
    pub commits_per_week: Vec<(String, u32)>,
    /// Libraries whose documentation was looked up.
    pub libraries: Vec<String>,
    /// Summary of the web research, when there was one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_summary: Option<String>,
}

/// A commit of the history.
#[derive(Debug, Clone, Deserialize)]
struct Commit {
    sha: String,
    #[serde(default)]
    date: String,
    summary: String,
}

/// A fact found by the agent.
#[derive(Debug, Deserialize)]
struct Finding {
    claim: String,
    source: SourceRef,
}

/// What the agent answers.
#[derive(Debug, Deserialize)]
struct Answer {
    #[serde(default)]
    commits: Vec<Commit>,
    #[serde(default)]
    findings: Vec<Finding>,
    #[serde(default)]
    libraries: Vec<String>,
}

impl Answer {
    fn validate(&self) -> Result<(), String> {
        for (i, c) in self.commits.iter().enumerate() {
            if c.sha.trim().is_empty() || c.summary.trim().is_empty() {
                return Err(format!("el commit {i} no tiene `sha` o `summary`"));
            }
            if !c.date.trim().is_empty() && day_number(&c.date).is_none() {
                return Err(format!(
                    "el commit {i} tiene una fecha inválida: {}",
                    c.date
                ));
            }
        }
        for (i, f) in self.findings.iter().enumerate() {
            if f.claim.trim().is_empty() {
                return Err(format!("el hallazgo {i} no tiene `claim`"));
            }
            validate_source(&f.source).map_err(|e| format!("el hallazgo {i}: {e}"))?;
        }
        Ok(())
    }
}

/// The Data node.
#[derive(Debug, Default)]
pub struct Data;

impl Data {
    /// The node.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

fn input(brief: &IntakeBrief, observed: &ObservationReport, git: bool, mcp: &[&str]) -> String {
    let mut out = format!(
        "{TASK}\n\nProblema ({}): {}\nArquitectura observada: {}\nArchivos implicados:\n",
        brief.kind.name_es(),
        brief.restated,
        observed.summary
    );
    for file in &observed.key_files {
        let _ = writeln!(out, "- {} ({})", file.path, file.role);
    }
    let _ = write!(out, "\n{}\n", if git { GIT } else { NO_GIT });
    if !mcp.is_empty() {
        let _ = writeln!(
            out,
            "Para buscar issues relacionados usa solo consultas de estas herramientas MCP: {}.",
            mcp.join(", ")
        );
    }
    let _ = write!(out, "\n{FORMAT}");
    out
}

/// Days since 1970-01-01 of an `AAAA-MM-DD` date (anything after the day,
/// like a time, is ignored).
fn day_number(date: &str) -> Option<i64> {
    let date = date.trim().get(..10)?;
    let mut parts = date.split('-');
    let y: i64 = parts.next()?.parse().ok()?;
    let m: i64 = parts.next()?.parse().ok()?;
    let d: i64 = parts.next()?.parse().ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    // Days from civil, H. Hinnant.
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + doe - 719_468)
}

/// The `AAAA-MM-DD` date of a day number.
fn civil(days: i64) -> String {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Commits per week (weeks start on Monday), with empty weeks in between,
/// oldest first and at most [`MAX_WEEKS`].
fn commits_per_week(commits: &[Commit]) -> Vec<(String, u32)> {
    // 1970-01-01 was a Thursday, three days after a Monday.
    let mondays: Vec<i64> = commits
        .iter()
        .filter_map(|c| day_number(&c.date))
        .map(|day| day - (day + 3).rem_euclid(7))
        .collect();
    let (Some(&first), Some(&last)) = (mondays.iter().min(), mondays.iter().max()) else {
        return Vec::new();
    };
    let weeks = usize::try_from((last - first) / 7 + 1).unwrap_or(1);
    let skip = weeks.saturating_sub(MAX_WEEKS);
    (0..weeks)
        .skip(skip)
        .map(|w| {
            let monday = first + 7 * i64::try_from(w).unwrap_or(0);
            let count = mondays.iter().filter(|&&m| m == monday).count();
            (civil(monday), u32::try_from(count).unwrap_or(u32::MAX))
        })
        .collect()
}

fn chart(weeks: &[(String, u32)]) -> ChartSpec {
    ChartSpec {
        title: "Commits por semana en los archivos implicados".into(),
        kind: ChartKind::Bar,
        labels: weeks.iter().map(|(week, _)| week.clone()).collect(),
        series: vec![Series {
            name: "commits".into(),
            values: weeks.iter().map(|(_, n)| f64::from(*n)).collect(),
        }],
    }
}

/// A short label of `source` for a table cell.
fn source_label(source: &SourceRef) -> String {
    match source {
        SourceRef::File {
            path,
            line_start: Some(start),
            line_end,
        } => match line_end {
            Some(end) if end != start => format!("{path}:{start}-{end}"),
            _ => format!("{path}:{start}"),
        },
        SourceRef::File { path, .. } => path.clone(),
        SourceRef::Commit { sha, summary } => {
            format!("{} {summary}", sha.get(..7).unwrap_or(sha))
        }
        SourceRef::Url { url, title } => title.clone().unwrap_or_else(|| url.clone()),
        SourceRef::Command { command, .. } => format!("$ {command}"),
    }
}

fn source_kind(source: &SourceRef) -> &'static str {
    match source {
        SourceRef::File { .. } => "archivo",
        SourceRef::Commit { .. } => "commit",
        SourceRef::Url { .. } => "web",
        SourceRef::Command { .. } => "comando",
    }
}

/// Asks the stage's model, which the adapter gives web search, and
/// returns its answer and citations.
async fn search_web(ctx: &NodeContext, prompt: &str) -> Result<(String, Vec<Citation>), NodeError> {
    let req = ctx.request(WEB_SYSTEM, vec![Message::user_text(prompt)]);
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
    let text = assembled
        .message
        .content
        .iter()
        .filter_map(|part| match part {
            domain::conversation::ContentPart::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<String>()
        .trim()
        .to_owned();
    Ok((text, assembled.citations))
}

#[async_trait]
impl ResearchNode for Data {
    fn stage(&self) -> Stage {
        Stage::Data
    }

    async fn run(&self, ctx: &mut NodeContext) -> Result<NodeOutcome, NodeError> {
        let Some(brief) = ctx.data::<IntakeBrief>(BRIEF_KEY) else {
            return Ok(NodeOutcome::Back {
                to: Stage::Intake,
                reason: "el problema no está definido: falta el pedido reformulado de Intake"
                    .into(),
            });
        };
        let Some(observed) = ctx.data::<ObservationReport>(observation::REPORT_KEY) else {
            return Ok(NodeOutcome::Back {
                to: Stage::Observation,
                reason: "faltan los archivos implicados que marca la observación".into(),
            });
        };
        let git = has_shell(ctx);
        let mcp: Vec<String> = ctx
            .agent()?
            .tools()
            .names()
            .filter(|name| name.starts_with("mcp__"))
            .map(str::to_owned)
            .collect();
        let mcp_names: Vec<&str> = mcp.iter().map(String::as_str).collect();
        let answer: Answer = explore_json(
            ctx,
            &input(&brief, &observed, git, &mcp_names),
            Answer::validate,
        )
        .await?;

        let now = ctx.now();
        let mut evidence = Vec::new();
        for commit in answer.commits.iter().take(MAX_COMMIT_EVIDENCE) {
            let date = if commit.date.trim().is_empty() {
                String::new()
            } else {
                format!(" ({})", commit.date.trim())
            };
            evidence.push(ctx.state_mut().add_evidence(
                format!(
                    "Cambio en los archivos implicados{date}: {}",
                    commit.summary.trim()
                ),
                SourceRef::Commit {
                    sha: commit.sha.trim().to_owned(),
                    summary: commit.summary.trim().to_owned(),
                },
                now,
            ));
        }
        let findings = answer.findings.len();
        for finding in answer.findings {
            evidence.push(
                ctx.state_mut()
                    .add_evidence(finding.claim.trim(), finding.source, now),
            );
        }

        let libraries: Vec<String> = answer
            .libraries
            .iter()
            .map(|l| l.trim().to_owned())
            .filter(|l| !l.is_empty())
            .collect();
        let mut web_summary = None;
        let mut web = 0;
        if !libraries.is_empty() {
            let prompt = format!(
                "Problema: {}\nLibrerías: {}",
                brief.restated,
                libraries.join(", ")
            );
            match search_web(ctx, &prompt).await {
                Ok((text, citations)) => {
                    web = citations.len();
                    for id in citations_to_evidence(&citations, ctx.state_mut(), now) {
                        if !evidence.contains(&id) {
                            evidence.push(id);
                        }
                    }
                    web_summary = Some(text).filter(|t| !t.is_empty());
                }
                Err(err) => {
                    let message = format!("No se pudo consultar la web: {err}");
                    ctx.notice(NoticeLevel::Warning, &message);
                    ctx.log(message);
                }
            }
        }

        let weeks = commits_per_week(&answer.commits);
        if !weeks.is_empty() {
            ctx.emit(BlockKind::Chart(chart(&weeks)))?;
        }
        let records: Vec<_> = evidence
            .iter()
            .filter_map(|id| ctx.state().evidence(*id).cloned())
            .collect();
        if records.is_empty() {
            ctx.emit(BlockKind::Markdown {
                title: None,
                source: "No se encontraron datos adicionales: ni historial, ni logs, ni \
                         métricas, ni documentación relevante."
                    .into(),
            })?;
        } else {
            ctx.emit(BlockKind::Table(TableSpec {
                columns: vec![
                    Column::new("Id"),
                    Column::new("Tipo"),
                    Column::new("Dato"),
                    Column::new("Fuente"),
                ],
                rows: records
                    .iter()
                    .map(|e| {
                        vec![
                            Cell::Text(e.id.to_string()),
                            Cell::Text(source_kind(&e.source).into()),
                            Cell::Text(e.claim.clone()),
                            Cell::Text(source_label(&e.source)),
                        ]
                    })
                    .collect(),
            }))?;
            ctx.emit(sources_block(records.iter().map(|e| e.source.clone())))?;
        }

        let ids: Vec<String> = evidence.iter().map(ToString::to_string).collect();
        ctx.log(format!(
            "Datos reunidos: {} commits en {} semanas, {findings} hallazgos del repo{}, \
             {web} citas web{}; evidencia {}.",
            answer.commits.len(),
            weeks.len(),
            if mcp.is_empty() { "" } else { " (con MCP)" },
            if libraries.is_empty() {
                String::new()
            } else {
                format!(" sobre {}", libraries.join(", "))
            },
            if ids.is_empty() {
                "ninguna".to_owned()
            } else {
                ids.join(", ")
            },
        ));
        ctx.set_data(
            REPORT_KEY,
            DataReport {
                evidence,
                commits_per_week: weeks,
                libraries,
                web_summary,
            },
        )?;
        Ok(NodeOutcome::Done)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use domain::conversation::{StopReason, Usage};
    use domain::model::{ModelId, ModelRole};
    use domain::permission::ToolEffect;
    use serde_json::json;

    use super::observation::testing::{agent, blocks, brief, context, workspace};
    use super::observation::{KeyFile, ObservationReport};
    use super::*;
    use crate::ports::llm::LlmEvent;
    use crate::ports::mcp::McpTool as McpSpec;
    use crate::ports::shell::CommandOutput;
    use crate::testing::fs::ScriptedShell;
    use crate::testing::llm::{FakeMcp, ScriptedLlm, ScriptedResponse};
    use crate::testing::system::RecordingSink;
    use crate::tools::mcp::McpTool;

    fn observed() -> ObservationReport {
        ObservationReport {
            summary: "Un crate con el login.".into(),
            architecture: "flowchart TD\n  login --> hash".into(),
            key_files: vec![KeyFile {
                path: "src/login.rs".into(),
                kind: "código".into(),
                role: "inicia sesión".into(),
            }],
            evidence: vec![],
        }
    }

    fn web_answer() -> ScriptedResponse {
        let citation = Citation {
            url: "https://docs.rs/argon2".into(),
            title: Some("argon2 - docs.rs".into()),
            cited_text: "Los parámetros por defecto usan 19 MiB de memoria.".into(),
        };
        ScriptedResponse::events(vec![
            LlmEvent::MessageStart {
                model: ModelId::new("fake"),
            },
            LlmEvent::ServerToolUse {
                index: 0,
                name: "web_search".into(),
            },
            LlmEvent::BlockStop { index: 0 },
            LlmEvent::TextDelta {
                index: 1,
                text: "- argon2 es lento por diseño.".into(),
            },
            LlmEvent::Citation { index: 1, citation },
            LlmEvent::BlockStop { index: 1 },
            LlmEvent::Usage(Usage::default()),
            LlmEvent::MessageStop {
                stop_reason: StopReason::EndTurn,
            },
        ])
    }

    fn agent_answer() -> String {
        json!({
            "commits": [
                {"sha": "aaa1111222", "date": "2026-09-28", "summary": "usa argon2"},
                {"sha": "bbb3333444", "date": "2026-10-01", "summary": "sube el costo"},
                {"sha": "ccc5555666", "date": "2026-10-14", "summary": "agrega log"}
            ],
            "findings": [
                {"claim": "El issue #12 reporta logins de 6 s.",
                 "source": {"type": "url", "url": "https://github.com/o/r/issues/12", "title": "#12"}}
            ],
            "libraries": ["argon2"]
        })
        .to_string()
    }

    #[tokio::test]
    async fn gathers_history_issues_and_docs() {
        let git_log = [
            "log",
            "--date=short",
            "--format=%H%x09%ad%x09%s",
            "-n",
            "50",
            "--",
            "src/login.rs",
        ];
        let llm = Arc::new(ScriptedLlm::new([
            ScriptedResponse::tool_use(
                "t1",
                "run_command",
                json!({"command": "git", "args": git_log}),
            ),
            ScriptedResponse::tool_use("t2", "mcp__github__search_issues", json!({"q": "login"})),
            ScriptedResponse::text(agent_answer()),
            web_answer(),
        ]));
        let shell = Arc::new(ScriptedShell::new().on(
            "git",
            git_log,
            CommandOutput {
                status: Some(0),
                stdout: "aaa1111222\t2026-09-28\tusa argon2\n".into(),
                ..CommandOutput::default()
            },
        ));
        let mcp =
            Arc::new(FakeMcp::new().with_text_tool("github", "search_issues", "#12 login lento"));
        let service = agent(&llm, &workspace(), Some(shell.clone()), |tools| {
            let spec = McpSpec {
                server: "github".into(),
                name: "search_issues".into(),
                description: "Busca issues".into(),
                input_schema: json!({"type": "object"}),
            };
            tools
                .register(Arc::new(
                    McpTool::new(spec, mcp.clone()).with_effect(ToolEffect::Read),
                ))
                .unwrap();
        });
        let sink = Arc::new(RecordingSink::new());
        let mut ctx = context(Stage::Data, &llm, &sink, Some(service));
        ctx.set_data(BRIEF_KEY, brief()).unwrap();
        ctx.set_data(observation::REPORT_KEY, observed()).unwrap();

        assert_eq!(Data::new().run(&mut ctx).await.unwrap(), NodeOutcome::Done);

        assert_eq!(shell.calls().len(), 1);
        assert_eq!(mcp.calls().len(), 1);
        let requests = llm.requests();
        let sent = format!("{:?}", requests[0].messages);
        assert!(sent.contains("src/login.rs") && sent.contains("mcp__github__search_issues"));
        let web = requests.last().unwrap();
        assert_eq!(web.model.role, ModelRole::Explore);
        assert!(format!("{:?}", web.messages).contains("argon2"));

        let state = ctx.state();
        assert_eq!(state.evidence.len(), 5);
        assert!(
            matches!(&state.evidence[0].source, SourceRef::Commit { sha, .. } if sha == "aaa1111222")
        );
        assert!(
            matches!(&state.evidence[3].source, SourceRef::Url { url, .. } if url.ends_with("/12"))
        );
        assert!(
            matches!(&state.evidence[4].source, SourceRef::Url { url, .. } if url == "https://docs.rs/argon2")
        );
        assert!(state.journal[0].text.contains("E1, E2, E3, E4, E5"));

        let report: DataReport = ctx.data(REPORT_KEY).unwrap();
        assert_eq!(
            report.commits_per_week,
            [
                ("2026-09-28".to_owned(), 2),
                ("2026-10-05".to_owned(), 0),
                ("2026-10-12".to_owned(), 1)
            ]
        );
        assert_eq!(
            report.web_summary.as_deref(),
            Some("- argon2 es lento por diseño.")
        );

        let shown = blocks(&sink);
        assert_eq!(shown.len(), 3, "{shown:#?}");
        assert!(matches!(&shown[0], BlockKind::Chart(c) if c.labels.len() == 3));
        assert!(matches!(&shown[1], BlockKind::Table(t) if t.rows.len() == 5));
        assert!(matches!(&shown[2], BlockKind::Sources { sources } if sources.len() == 5));
    }

    #[tokio::test]
    async fn without_data_it_says_so_and_skips_the_web() {
        let llm = Arc::new(ScriptedLlm::new([ScriptedResponse::text(
            r#"{"commits": [], "findings": [], "libraries": []}"#,
        )]));
        let service = agent(&llm, &workspace(), None, |_| {});
        let sink = Arc::new(RecordingSink::new());
        let mut ctx = context(Stage::Data, &llm, &sink, Some(service));
        ctx.set_data(BRIEF_KEY, brief()).unwrap();
        ctx.set_data(observation::REPORT_KEY, observed()).unwrap();

        Data::new().run(&mut ctx).await.unwrap();

        assert_eq!(llm.requests().len(), 1);
        assert!(format!("{:?}", llm.requests()[0].messages).contains("No tienes acceso a git"));
        assert!(ctx.state().evidence.is_empty());
        let shown = blocks(&sink);
        assert!(
            matches!(shown.as_slice(), [BlockKind::Markdown { .. }]),
            "{shown:#?}"
        );
        assert!(ctx.data::<DataReport>(REPORT_KEY).is_some());
    }

    #[tokio::test]
    async fn a_failed_web_search_is_a_warning() {
        let llm = Arc::new(ScriptedLlm::new([
            ScriptedResponse::text(r#"{"libraries": ["argon2"]}"#),
            ScriptedResponse::open_error(LlmError::Overloaded),
        ]));
        let service = agent(&llm, &workspace(), None, |_| {});
        let sink = Arc::new(RecordingSink::new());
        let mut ctx = context(Stage::Data, &llm, &sink, Some(service));
        ctx.set_data(BRIEF_KEY, brief()).unwrap();
        ctx.set_data(observation::REPORT_KEY, observed()).unwrap();

        assert_eq!(Data::new().run(&mut ctx).await.unwrap(), NodeOutcome::Done);
        assert!(
            ctx.state().journal[0]
                .text
                .contains("No se pudo consultar la web")
        );
    }

    #[tokio::test]
    async fn missing_earlier_outputs_send_the_flow_back() {
        let llm = Arc::new(ScriptedLlm::new([]));
        let sink = Arc::new(RecordingSink::new());
        let mut ctx = context(Stage::Data, &llm, &sink, None);
        let outcome = Data::new().run(&mut ctx).await.unwrap();
        assert!(matches!(
            outcome,
            NodeOutcome::Back {
                to: Stage::Intake,
                ..
            }
        ));

        ctx.set_data(BRIEF_KEY, brief()).unwrap();
        let outcome = Data::new().run(&mut ctx).await.unwrap();
        assert!(matches!(
            outcome,
            NodeOutcome::Back {
                to: Stage::Observation,
                ..
            }
        ));
        assert!(llm.requests().is_empty());
    }

    #[test]
    fn weeks_start_on_monday_and_dates_round_trip() {
        for date in ["1970-01-01", "2000-02-29", "2026-10-17", "1969-12-29"] {
            assert_eq!(civil(day_number(date).unwrap()), date);
        }
        assert_eq!(day_number("2026-13-01"), None);
        assert_eq!(day_number("ayer"), None);
        let commit = |date: &str| Commit {
            sha: "x".into(),
            date: date.into(),
            summary: "s".into(),
        };
        // Saturday 2026-10-17 belongs to the week of Monday 2026-10-12.
        assert_eq!(
            commits_per_week(&[commit("2026-10-17T10:00:00"), commit("")]),
            [("2026-10-12".to_owned(), 1)]
        );
        assert!(commits_per_week(&[]).is_empty());
    }
}
