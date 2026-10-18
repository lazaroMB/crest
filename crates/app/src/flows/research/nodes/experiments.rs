//! Node 6, Experimentos: reproducible protocols that test the hypotheses.
//!
//! An agent turn with tools (`main` role, the usual approvals: writing a
//! test or running a benchmark asks the user) designs and runs one
//! protocol per open hypothesis (a failing test, a benchmark script,
//! reproduction steps) and ends with a JSON report. Each command the agent
//! really ran becomes [`Evidence`](domain::research::Evidence) with a
//! [`SourceRef::Command`] holding the tail of its output, and the
//! hypothesis turns `Supported`, `Refuted` or `Untested`; a protocol whose
//! command never ran stays `Untested`. The agent's own `ToolCall` and
//! `Diff` blocks show the work; the node adds a `Chart` per experiment
//! with measurements and a summary `Table`. Without hypotheses the flow
//! goes back to Hipótesis.

use std::fmt::Write as _;

use async_trait::async_trait;
use domain::block::{BlockKind, Cell, ChartKind, ChartSpec, Column, Series, SourceRef, TableSpec};
use domain::conversation::{ContentPart, Session, ToolResultContent};
use domain::research::{
    EvidenceId, Hypothesis, HypothesisId, HypothesisStatus, ResearchState, Stage,
};
use serde::{Deserialize, Serialize};

use super::analysis;
use super::hypotheses::{by_priority, status_label};
use crate::flows::research::node::{NodeContext, NodeError, NodeOutcome, ResearchNode};
use crate::ports::shell::RunCommandRequest;

/// Flow data key of the protocols run (`Vec<Protocol>`), for the test
/// protocol of the Spec and the `hypothesis_tests` of the plan.
pub const KEY_PROTOCOLS: &str = "experiments.protocols";

/// Name of the agent tool whose runs become evidence.
const RUN_COMMAND: &str = "run_command";
/// Lines and characters kept from the end of a command's output.
const EXCERPT_LINES: usize = 40;
const EXCERPT_CHARS: usize = 2000;

const INSTRUCTIONS: &str = "\
Eres la etapa de Experimentos de una investigación sobre este repositorio. Para cada \
hipótesis de la lista diseña un protocolo reproducible que la confirme o la refute: un \
test que falle mientras el problema exista, un script de benchmark o pasos de \
reproducción. Escribe los archivos que necesites con write_file o edit_file (el usuario \
aprueba los cambios) y corre cada protocolo con run_command. No inventes resultados: \
si no puedes correr un protocolo, márcalo \"untested\".";

const SCHEMA: &str = r#"Cuando termines, responde solo con este JSON, sin texto extra:
{
  "experiments": [
    {
      "hypothesis_id": "H1",
      "protocol": "qué hace el protocolo y cómo repetirlo",
      "command": "el comando exactamente como lo corriste con run_command, o null",
      "expect": "qué resultado confirmaría la hipótesis",
      "result": "supported | refuted | untested",
      "observation": "lo que mostró la salida, en una oración",
      "measurements": [{ "label": "p95 actual", "value": 2000.0, "unit": "ms" }]
    }
  ]
}
Incluye un experimento por cada hipótesis de la lista."#;

/// What the experiment says about its hypothesis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// The result backs the hypothesis.
    Supported,
    /// The result contradicts it.
    Refuted,
    /// It could not be tested.
    Untested,
}

/// A measurement taken by an experiment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Measurement {
    /// What was measured.
    pub label: String,
    /// Value.
    pub value: f64,
    /// Unit, e.g. `ms`.
    #[serde(default)]
    pub unit: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct ExperimentOut {
    hypothesis_id: HypothesisId,
    protocol: String,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    expect: Option<String>,
    result: Outcome,
    #[serde(default)]
    observation: String,
    #[serde(default)]
    measurements: Vec<Measurement>,
}

#[derive(Debug, Clone, Deserialize)]
struct Report {
    experiments: Vec<ExperimentOut>,
}

/// A protocol as it was run, stored under [`KEY_PROTOCOLS`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Protocol {
    /// Hypothesis it tests.
    pub hypothesis_id: HypothesisId,
    /// What it does and how to repeat it.
    pub protocol: String,
    /// Command that runs it, when it ran.
    pub command: Option<String>,
    /// Result that would back the hypothesis.
    pub expect: Option<String>,
    /// Status given to the hypothesis.
    pub status: HypothesisStatus,
    /// Evidence recorded from its output.
    pub evidence_id: Option<EvidenceId>,
    /// Measurements taken.
    pub measurements: Vec<Measurement>,
}

/// A `run_command` call of the agent and the text of its result.
struct Run {
    command: String,
    output: String,
}

/// The Experimentos node.
#[derive(Debug, Default)]
pub struct Experiments;

impl Experiments {
    /// The node.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ResearchNode for Experiments {
    fn stage(&self) -> Stage {
        Stage::Experiments
    }

    async fn run(&self, ctx: &mut NodeContext) -> Result<NodeOutcome, NodeError> {
        if ctx.state().hypotheses.is_empty() {
            return Ok(NodeOutcome::Back {
                to: Stage::Hypotheses,
                reason: "no hay hipótesis que probar".into(),
            });
        }
        let targets: Vec<Hypothesis> = by_priority(&ctx.state().hypotheses)
            .into_iter()
            .filter(|h| {
                matches!(
                    h.status,
                    HypothesisStatus::Open | HypothesisStatus::Untested
                )
            })
            .collect();
        if targets.is_empty() {
            ctx.log("Experimentos: todas las hipótesis ya tienen resultado.");
            return Ok(NodeOutcome::Done);
        }

        let agent = std::sync::Arc::clone(ctx.agent()?);
        let mut session = ctx.new_session();
        let input = prompt(ctx.problem(), ctx.state(), &targets, ctx.user_reply());
        let report: Report = ctx
            .agent_json(&agent, &mut session, input, |r| check(r, &targets))
            .await?;

        let runs = command_runs(&session);
        let now = ctx.now();
        let protocols: Vec<Protocol> = report
            .experiments
            .into_iter()
            .map(|e| record(ctx.state_mut(), &runs, e, now))
            .collect();
        for p in protocols.iter().filter(|p| !p.measurements.is_empty()) {
            ctx.emit(BlockKind::Chart(chart(p)))?;
        }
        ctx.emit(BlockKind::Table(table(&protocols)))?;
        ctx.set_data(KEY_PROTOCOLS, &protocols)?;

        let mut journal = format!("Experimentos: {} protocolos.", protocols.len());
        for p in &protocols {
            let _ = write!(
                journal,
                "\n- {}: {} ({})",
                p.hypothesis_id,
                status_label(p.status),
                p.command.as_deref().unwrap_or("sin comando corrido")
            );
        }
        ctx.log(journal);
        Ok(NodeOutcome::Done)
    }
}

fn prompt(
    problem: &str,
    state: &ResearchState,
    targets: &[Hypothesis],
    reply: Option<&str>,
) -> String {
    let mut out = format!("{INSTRUCTIONS}\n\n");
    out.push_str(&analysis::context_prompt(problem, state, reply));
    out.push_str("\nHipótesis a probar, por prioridad:\n");
    for h in targets {
        let _ = writeln!(
            out,
            "- {} (prioridad {}): {} Causas: {}",
            h.id,
            h.priority,
            h.statement,
            h.causes.join("; ")
        );
    }
    let _ = write!(out, "\n{SCHEMA}");
    out
}

fn check(report: &Report, targets: &[Hypothesis]) -> Result<(), String> {
    for e in &report.experiments {
        if !targets.iter().any(|h| h.id == e.hypothesis_id) {
            return Err(format!(
                "{} no está en la lista de hipótesis",
                e.hypothesis_id
            ));
        }
        if e.protocol.trim().is_empty() {
            return Err(format!(
                "el experimento de {} no describe el protocolo",
                e.hypothesis_id
            ));
        }
        if let Some(m) = e.measurements.iter().find(|m| !m.value.is_finite()) {
            return Err(format!("la medición {:?} no es un número finito", m.label));
        }
    }
    let missing: Vec<String> = targets
        .iter()
        .filter(|h| !report.experiments.iter().any(|e| e.hypothesis_id == h.id))
        .map(|h| h.id.to_string())
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!("faltan experimentos para {}", missing.join(", ")))
    }
}

/// Every `run_command` the agent ran and got a result for.
fn command_runs(session: &Session) -> Vec<Run> {
    let parts: Vec<&ContentPart> = session
        .turns
        .iter()
        .flat_map(|t| &t.messages)
        .flat_map(|m| &m.content)
        .collect();
    parts
        .iter()
        .filter_map(|part| match part {
            ContentPart::ToolUse { id, name, input } if name == RUN_COMMAND => {
                let request: RunCommandRequest = serde_json::from_value(input.clone()).ok()?;
                let output = parts.iter().find_map(|p| match p {
                    ContentPart::ToolResult {
                        tool_use_id,
                        content,
                        is_error: false,
                    } if tool_use_id == id => Some(
                        content
                            .iter()
                            .filter_map(|c| match c {
                                ToolResultContent::Text { text } => Some(text.as_str()),
                                ToolResultContent::Image { .. } => None,
                            })
                            .collect::<Vec<_>>()
                            .join("\n"),
                    ),
                    _ => None,
                })?;
                Some(Run {
                    command: request.display(),
                    output,
                })
            }
            _ => None,
        })
        .collect()
}

fn normalize(command: &str) -> String {
    command.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The end of `output`: its last lines, cut to a character limit.
fn excerpt(output: &str) -> String {
    let lines: Vec<&str> = output.trim_end().lines().collect();
    let tail = lines[lines.len().saturating_sub(EXCERPT_LINES)..].join("\n");
    let count = tail.chars().count();
    if count <= EXCERPT_CHARS {
        tail
    } else {
        let kept: String = tail.chars().skip(count - EXCERPT_CHARS).collect();
        format!("…{kept}")
    }
}

/// Records the result of one experiment: evidence when its command ran,
/// and the new status of its hypothesis.
fn record(state: &mut ResearchState, runs: &[Run], e: ExperimentOut, now: u64) -> Protocol {
    let run = e.command.as_deref().and_then(|c| {
        let wanted = normalize(c);
        runs.iter().rev().find(|r| normalize(&r.command) == wanted)
    });
    let status = match (run, e.result) {
        (Some(_), Outcome::Supported) => HypothesisStatus::Supported,
        (Some(_), Outcome::Refuted) => HypothesisStatus::Refuted,
        _ => HypothesisStatus::Untested,
    };
    let evidence_id = run.map(|run| {
        let claim = if e.observation.trim().is_empty() {
            format!("{}: {}", e.hypothesis_id, e.protocol.trim())
        } else {
            format!("{}: {}", e.hypothesis_id, e.observation.trim())
        };
        state.add_evidence(
            claim,
            SourceRef::Command {
                command: run.command.clone(),
                output_excerpt: excerpt(&run.output),
            },
            now,
        )
    });
    if let Some(h) = state
        .hypotheses
        .iter_mut()
        .find(|h| h.id == e.hypothesis_id)
    {
        h.status = status;
    }
    Protocol {
        hypothesis_id: e.hypothesis_id,
        protocol: e.protocol,
        command: run.map(|r| r.command.clone()),
        expect: e.expect,
        status,
        evidence_id,
        measurements: e.measurements,
    }
}

fn chart(p: &Protocol) -> ChartSpec {
    let unit = p
        .measurements
        .iter()
        .find_map(|m| m.unit.clone())
        .unwrap_or_else(|| "valor".into());
    ChartSpec {
        title: format!("{}: mediciones", p.hypothesis_id),
        kind: ChartKind::Bar,
        labels: p.measurements.iter().map(|m| m.label.clone()).collect(),
        series: vec![Series {
            name: unit,
            values: p.measurements.iter().map(|m| m.value).collect(),
        }],
    }
}

fn table(protocols: &[Protocol]) -> TableSpec {
    let text = |s: &str| Cell::Text(s.to_owned());
    TableSpec {
        columns: [
            "Hipótesis",
            "Protocolo",
            "Comando",
            "Resultado",
            "Evidencia",
        ]
        .into_iter()
        .map(Column::new)
        .collect(),
        rows: protocols
            .iter()
            .map(|p| {
                vec![
                    Cell::Text(p.hypothesis_id.to_string()),
                    text(&p.protocol),
                    p.command.as_deref().map_or(Cell::Null, text),
                    text(status_label(p.status)),
                    p.evidence_id
                        .map_or(Cell::Null, |id| Cell::Text(id.to_string())),
                ]
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use domain::model::ModelPolicy;
    use domain::permission::{PermissionMode, PermissionPolicy};
    use domain::workspace::WorkspaceRoot;
    use serde_json::json;

    use super::*;
    use crate::flows::research::node::NodeDeps;
    use crate::ports::approval::ApprovalResponse;
    use crate::ports::events::DomainEvent;
    use crate::ports::shell::CommandOutput;
    use crate::testing::fs::{InMemoryFs, ScriptedShell};
    use crate::testing::llm::{ScriptedLlm, ScriptedResponse};
    use crate::testing::system::{RecordingSink, RecordingTelemetry, ScriptedApproval};
    use crate::tools::files::{FileToolPorts, register_file_tools};
    use crate::tools::registry::ToolRegistry;
    use crate::usecases::run_turn::{AgentDeps, AgentService};

    struct Harness {
        llm: Arc<ScriptedLlm>,
        fs: Arc<InMemoryFs>,
        approval: Arc<ScriptedApproval>,
        sink: Arc<RecordingSink>,
        deps: NodeDeps,
    }

    impl Harness {
        fn new(script: Vec<ScriptedResponse>) -> Self {
            let llm = Arc::new(ScriptedLlm::new(script));
            let fs = Arc::new(
                InMemoryFs::new(WorkspaceRoot::new("/ws").unwrap())
                    .with_file("src/login.rs", "pub fn login() {}\n"),
            );
            let shell = Arc::new(ScriptedShell::new().on(
                "cargo",
                ["test", "login_index"],
                CommandOutput {
                    status: Some(101),
                    stdout: "test login_index ... FAILED\nseq scan on users: 1900 ms\n".into(),
                    ..CommandOutput::default()
                },
            ));
            let sink = Arc::new(RecordingSink::new());
            let approval = Arc::new(ScriptedApproval::always(ApprovalResponse::Approve));
            let mut tools = ToolRegistry::new();
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
            let agent = AgentService::new(
                AgentDeps {
                    llm: llm.clone(),
                    events: sink.clone(),
                    approval: approval.clone(),
                    telemetry: Arc::new(RecordingTelemetry::new()),
                },
                tools,
                ModelPolicy::default(),
                Arc::new(Mutex::new(PermissionPolicy::new(PermissionMode::Ask))),
            );
            let deps = NodeDeps::new(llm.clone(), ModelPolicy::default(), sink.clone())
                .with_agent(Arc::new(agent))
                .with_clock(|| 3);
            Self {
                llm,
                fs,
                approval,
                sink,
                deps,
            }
        }

        fn context(&self, state: ResearchState) -> NodeContext {
            NodeContext::new(
                Stage::Experiments,
                self.deps.clone(),
                "el login tarda 2 s",
                state,
            )
        }

        fn kinds(&self) -> Vec<String> {
            self.sink
                .events()
                .into_iter()
                .filter_map(|e| match e {
                    DomainEvent::BlockAdded { block } => Some(
                        serde_json::to_value(&block.kind).unwrap()["type"]
                            .as_str()
                            .unwrap()
                            .to_owned(),
                    ),
                    _ => None,
                })
                .collect()
        }
    }

    fn state() -> ResearchState {
        let mut state = ResearchState::default();
        state.add_hypothesis(
            "Falta un índice en users.email.",
            vec!["Datos: sin índice".into()],
            1,
        );
        state.add_hypothesis("El pool se agota.", vec!["Infraestructura: pool".into()], 2);
        state
    }

    fn report(command: &str) -> String {
        json!({
            "experiments": [
                {
                    "hypothesis_id": "H1",
                    "protocol": "test que mide la consulta de login",
                    "command": command,
                    "expect": "el test falla con un seq scan",
                    "result": "supported",
                    "observation": "el test falla: seq scan de 1900 ms",
                    "measurements": [
                        { "label": "consulta", "value": 1900.0, "unit": "ms" },
                        { "label": "objetivo", "value": 50.0, "unit": "ms" }
                    ]
                },
                {
                    "hypothesis_id": "H2",
                    "protocol": "benchmark con 50 usuarios",
                    "command": null,
                    "result": "refuted"
                }
            ]
        })
        .to_string()
    }

    fn agent_script(final_answer: String) -> Vec<ScriptedResponse> {
        vec![
            ScriptedResponse::tool_use(
                "toolu_w",
                "write_file",
                json!({ "path": "tests/login_index.rs", "content": "#[test]\nfn login_index() {}\n" }),
            ),
            ScriptedResponse::tool_use(
                "toolu_r",
                "run_command",
                json!({ "command": "cargo", "args": ["test", "login_index"] }),
            ),
            ScriptedResponse::text(final_answer),
        ]
    }

    #[tokio::test]
    async fn commands_the_agent_ran_become_evidence_and_statuses() {
        let h = Harness::new(agent_script(report("cargo  test login_index")));
        let mut ctx = h.context(state());
        assert_eq!(
            Experiments::new().run(&mut ctx).await.unwrap(),
            NodeOutcome::Done
        );

        assert!(
            h.fs.file("tests/login_index.rs").is_some(),
            "the test was written"
        );
        assert_eq!(
            h.approval.requests().len(),
            2,
            "write and command were approved"
        );

        let state = ctx.state();
        let e = &state.evidence[0];
        assert_eq!(e.id.to_string(), "E1");
        let SourceRef::Command {
            command,
            output_excerpt,
        } = &e.source
        else {
            panic!("{e:?}")
        };
        assert_eq!(command, "cargo test login_index");
        assert!(
            output_excerpt.contains("exit code: 101"),
            "{output_excerpt}"
        );
        assert!(output_excerpt.contains("FAILED"), "{output_excerpt}");
        assert_eq!(state.evidence.len(), 1, "H2 ran nothing");
        assert_eq!(state.hypotheses[0].status, HypothesisStatus::Supported);
        assert_eq!(state.hypotheses[1].status, HypothesisStatus::Untested);

        let kinds = h.kinds();
        for kind in ["tool_call", "diff", "chart", "table"] {
            assert!(kinds.contains(&kind.to_owned()), "{kind} in {kinds:?}");
        }
        let protocols: Vec<Protocol> = ctx.data(KEY_PROTOCOLS).unwrap();
        assert_eq!(protocols[0].evidence_id, Some(e.id));
        assert!(ctx.state().journal[0].text.contains("H1: respaldada"));
        assert!(
            h.llm.requests()[0].messages[0].content.iter().any(
                |p| matches!(p, ContentPart::Text { text } if text.contains("H2 (prioridad 2)"))
            )
        );
    }

    #[tokio::test]
    async fn a_bad_report_is_asked_again_in_the_same_session() {
        let mut script = agent_script(json!({ "experiments": [] }).to_string());
        script.push(ScriptedResponse::text(report("cargo test login_index")));
        let h = Harness::new(script);
        let mut ctx = h.context(state());
        Experiments::new().run(&mut ctx).await.unwrap();
        let retry = serde_json::to_string(&h.llm.last_request().unwrap().messages).unwrap();
        assert!(retry.contains("faltan experimentos para H1, H2"), "{retry}");
        assert_eq!(
            ctx.state().hypotheses[0].status,
            HypothesisStatus::Supported
        );
    }

    #[tokio::test]
    async fn without_hypotheses_it_goes_back_to_them() {
        let h = Harness::new(vec![]);
        let mut ctx = h.context(ResearchState::default());
        let outcome = Experiments::new().run(&mut ctx).await.unwrap();
        assert!(matches!(
            outcome,
            NodeOutcome::Back {
                to: Stage::Hypotheses,
                ..
            }
        ));
        assert!(h.llm.requests().is_empty());
    }

    #[test]
    fn excerpts_keep_the_end_of_the_output() {
        let long: String = (0..100).map(|i| format!("línea {i}\n")).collect();
        let cut = excerpt(&long);
        assert!(
            cut.starts_with("línea 60") && cut.ends_with("línea 99"),
            "{cut}"
        );
        let wide = "é".repeat(EXCERPT_CHARS + 5);
        assert_eq!(excerpt(&wide).chars().count(), EXCERPT_CHARS + 1);
    }
}
