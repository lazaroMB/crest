//! `simulate_solution`: turns a Spec into an executable model, runs every
//! scenario and records the results in the Spec folder. On demand, not a
//! stage of the research flow: `/simulate <spec>`, `crest --headless
//! --simulate <spec>` and the `simulate_solution` tool all land here.
//!
//! 1. Reads `.crest/specs/NNNN-slug/spec.md` (title, black-box, sequence
//!    and state diagrams, test protocol) and its `sources.json`.
//! 2. **Assertions first**: a separate structured call derives the
//!    scenarios and their expectations from the test protocol, before any
//!    model exists. They are frozen.
//! 3. A second call writes the `crest.sysmodel/v1` model from the
//!    diagrams. Its scenarios get the frozen expectations back whatever it
//!    wrote; any change is reported as an attempt to loosen them.
//! 4. Schema, lint and a dry run; up to [`MAX_REPAIRS`] repair rounds send
//!    the errors back. Failing assertions are findings, not errors: they
//!    are never repaired away.
//! 5. Runs every scenario and writes `simulation/sysmodel.json`,
//!    `simulation/assertions.json`, `simulation/traces/<id>.json` and
//!    (with a [`SimExport`]) `simulation/board.html`; adds one `Evidence`
//!    per scenario to `sources.json` and a "Simulation" section to
//!    `journal.md`.

mod ask;
mod prompts;
pub mod spec;

#[cfg(test)]
mod tests;

use std::fmt::Write as _;
use std::sync::Arc;

use domain::block::SourceRef;
use domain::model::{ModelPolicy, ModelRole, ResolveRequest};
use domain::research::{Evidence, EvidenceId, IdAllocator};
use domain::sysmodel::lint::{Severity, uncovered};
use domain::sysmodel::model::{ComponentKind, Expectation, SpecRef};
use domain::sysmodel::{Board, Issue, ScenarioRun, SysModel, lint, run_all};
use domain::workspace::WorkspacePath;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::flows::research::nodes::spec::render::utc;
use crate::flows::research::nodes::spec::sink::{FsSpecSink, SPECS_DIR, SpecSink};
use crate::ports::events::{DomainEvent, EventSink, NoticeLevel};
use crate::ports::fs::{FileSystemPort, ReadFileRequest};
use crate::ports::llm::{LlmError, LlmPort, ToolDefinition};
use crate::ports::search::FileSearchPort;
use crate::ports::sim::SimExport;
use spec::SpecDoc;

/// Repair rounds after the first model.
pub const MAX_REPAIRS: usize = 2;

/// Folder of the simulation inside a Spec folder.
pub const SIMULATION_DIR: &str = "simulation";

/// Lines read at most from one Spec file.
const READ_LIMIT: usize = 200_000;

/// A component the assertions name, decided before the model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlannedComponent {
    /// Id, `[a-z][a-z0-9_]*`.
    pub id: String,
    /// Kind.
    pub kind: ComponentKind,
    /// The element of the black-box diagram it stands for.
    pub stands_for: String,
}

/// A scenario whose expectations are frozen before the model is written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FrozenScenario {
    /// Id, `[a-z][a-z0-9_]*`.
    pub id: String,
    /// What it shows.
    pub description: String,
    /// Test-protocol items and hypotheses it checks (`T1`, `H2`).
    #[serde(default)]
    pub covers: Vec<String>,
    /// The assertions.
    pub expect: Vec<Expectation>,
}

/// The assertions derived from the test protocol.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AssertionPlan {
    /// Components of the black box.
    pub components: Vec<PlannedComponent>,
    /// Scenarios.
    pub scenarios: Vec<FrozenScenario>,
}

impl AssertionPlan {
    /// Problems that make the plan unusable.
    #[must_use]
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.scenarios.is_empty() {
            out.push("there are no scenarios".to_owned());
        }
        let known = |id: &str| self.components.iter().any(|c| c.id == id);
        let mut ids = Vec::new();
        for s in &self.scenarios {
            if ids.contains(&s.id.as_str()) {
                out.push(format!("scenario `{}` is repeated", s.id));
            }
            ids.push(&s.id);
            if s.expect.is_empty() {
                out.push(format!("scenario `{}` has no assertions", s.id));
            }
            for e in &s.expect {
                let (texts, comps): (Vec<&str>, Vec<&str>) = match e {
                    Expectation::Always(e) => (vec![e.always.as_str()], vec![]),
                    Expectation::Never(e) => (vec![e.never.as_str()], vec![]),
                    Expectation::Eventually(e) => (vec![e.eventually.as_str()], vec![]),
                    Expectation::Count(e) => (vec![], vec![e.at.as_str()]),
                    Expectation::Path(e) => (vec![], e.path.iter().map(String::as_str).collect()),
                };
                for text in texts {
                    match domain::sysmodel::expr::parse(text) {
                        Err(err) => out.push(format!("scenario `{}`: {err}", s.id)),
                        Ok(expr) => {
                            for (scope, name) in expr.names() {
                                match scope {
                                    Some(c) if !known(c) => out.push(format!(
                                        "scenario `{}`: `{c}.{name}` names no planned component",
                                        s.id
                                    )),
                                    None if name != "now" => out.push(format!(
                                        "scenario `{}`: `{name}` must be written component.{name}",
                                        s.id
                                    )),
                                    _ => {}
                                }
                            }
                        }
                    }
                }
                for c in comps {
                    if !known(c) {
                        out.push(format!(
                            "scenario `{}`: `{c}` is not a planned component",
                            s.id
                        ));
                    }
                }
            }
        }
        out
    }
}

/// What the simulation needs.
pub struct SimulateDeps {
    /// The model that derives assertions and writes the sysmodel.
    pub llm: Arc<dyn LlmPort>,
    /// Model policy (the `plan` role is used).
    pub models: ModelPolicy,
    /// Reads and writes the Spec folder.
    pub fs: Arc<dyn FileSystemPort>,
    /// Lists the Spec folders.
    pub search: Arc<dyn FileSearchPort>,
    /// Progress notices and the finished board.
    pub events: Option<Arc<dyn EventSink>>,
    /// Builds `board.html`.
    pub export: Option<Arc<dyn SimExport>>,
}

/// One simulation request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimulateRequest {
    /// Spec folder (`0007-session-cache`), its number (`7`) or its path.
    pub spec: String,
    /// Now, in Unix milliseconds (evidence and journal).
    pub now_ms: u64,
    /// Emit [`DomainEvent::SimulationReady`] so a window opens the board.
    pub open_board: bool,
}

/// What a simulation produced.
#[derive(Debug, Clone, PartialEq)]
pub struct SimulationReport {
    /// Spec folder.
    pub folder: String,
    /// Model and runs.
    pub board: Board,
    /// The frozen assertions.
    pub plan: AssertionPlan,
    /// Files written, relative to the workspace.
    pub files: Vec<String>,
    /// Evidence added to `sources.json`, one per scenario.
    pub evidence: Vec<EvidenceId>,
    /// Lint warnings of the final model.
    pub warnings: Vec<Issue>,
    /// Repair rounds used.
    pub repairs: usize,
    /// Changes the model made to the frozen assertions (all reverted).
    pub loosening: Vec<String>,
}

impl SimulationReport {
    /// Path of `board.html`, when written.
    #[must_use]
    pub fn board_html(&self) -> Option<&str> {
        self.files
            .iter()
            .find(|f| f.ends_with("/board.html"))
            .map(String::as_str)
    }

    /// Verdicts as text, one line per scenario and one per failure.
    #[must_use]
    pub fn summary(&self) -> String {
        let runs = &self.board.runs;
        let passed = runs.iter().filter(|r| r.passed).count();
        let mut out = format!(
            "Simulation of {}: {passed} of {} scenarios passed (model {}).\n",
            self.folder,
            runs.len(),
            self.board.model.hash()
        );
        for run in runs {
            let ok = run.verdicts.iter().filter(|v| v.passed).count();
            let _ = writeln!(
                out,
                "{} {} ({ok}/{} checks, seed {})",
                if run.passed { "PASS" } else { "FAIL" },
                run.scenario,
                run.verdicts.len(),
                run.trace.seed
            );
            for v in run.failures() {
                let _ = writeln!(out, "  ✗ {}: {}", v.property, v.detail);
            }
        }
        if self.repairs > 0 {
            let _ = writeln!(out, "Repair rounds: {}.", self.repairs);
        }
        for l in &self.loosening {
            let _ = writeln!(out, "Frozen assertions restored: {l}");
        }
        let warnings = self.warnings.len();
        if warnings > 0 {
            let _ = writeln!(out, "Lint warnings: {warnings}.");
        }
        if let Some(html) = self.board_html() {
            let _ = writeln!(out, "Board: {html}");
        }
        out
    }
}

/// Why a simulation could not be produced.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum SimulateError {
    /// The Spec folder does not exist or cannot be read.
    #[error("{0}")]
    Spec(String),
    /// The model call failed.
    #[error("model: {0}")]
    Llm(#[from] LlmError),
    /// The assertions could not be derived.
    #[error("no valid assertions after a retry: {}", .0.join("; "))]
    Plan(Vec<String>),
    /// No valid model after the repair rounds.
    #[error("no valid model after {rounds} repair rounds: {}", issues.join("; "))]
    Model {
        /// Repair rounds used.
        rounds: usize,
        /// The last problems.
        issues: Vec<String>,
    },
    /// A file of the Spec folder could not be written.
    #[error("writing the Spec folder: {0}")]
    Write(String),
}

/// Runs simulations; cheap to clone.
#[derive(Clone)]
pub struct Simulator {
    deps: Arc<SimulateDeps>,
}

impl std::fmt::Debug for Simulator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Simulator").finish_non_exhaustive()
    }
}

/// Folder of `target` among `folders`: an exact name, a number (`7`,
/// `0007`) or a path ending in the folder.
#[must_use]
pub fn resolve_folder(folders: &[String], target: &str) -> Option<String> {
    let t = target.trim().trim_end_matches('/');
    let t = t
        .rsplit('/')
        .find(|p| !p.is_empty() && *p != "spec.md")
        .unwrap_or(t);
    if let Some(f) = folders.iter().find(|f| f.as_str() == t) {
        return Some(f.clone());
    }
    if let Ok(n) = t.parse::<u32>() {
        let prefix = format!("{n:04}-");
        return folders.iter().find(|f| f.starts_with(&prefix)).cloned();
    }
    let mut matches = folders.iter().filter(|f| f.contains(t));
    match (matches.next(), matches.next()) {
        (Some(f), None) => Some(f.clone()),
        _ => None,
    }
}

impl Simulator {
    /// A simulator over `deps`.
    #[must_use]
    pub fn new(deps: SimulateDeps) -> Self {
        Self {
            deps: Arc::new(deps),
        }
    }

    fn notice(&self, message: impl Into<String>) {
        if let Some(events) = &self.deps.events {
            events.emit(DomainEvent::Notice {
                level: NoticeLevel::Info,
                message: message.into(),
            });
        }
    }

    fn sink(&self) -> FsSpecSink {
        FsSpecSink::new(Arc::clone(&self.deps.fs), Arc::clone(&self.deps.search))
    }

    async fn read(&self, folder: &str, file: &str) -> Result<Option<String>, SimulateError> {
        let path = WorkspacePath::parse(&format!("{SPECS_DIR}/{folder}/{file}"))
            .map_err(|e| SimulateError::Spec(e.to_string()))?;
        if !self
            .deps
            .fs
            .exists(&path)
            .await
            .map_err(|e| SimulateError::Spec(e.to_string()))?
        {
            return Ok(None);
        }
        let content = self
            .deps
            .fs
            .read_file(ReadFileRequest {
                path,
                offset: None,
                limit: Some(READ_LIMIT),
            })
            .await
            .map_err(|e| SimulateError::Spec(e.to_string()))?;
        let mut text = content.lines.join("\n");
        text.push('\n');
        Ok(Some(text))
    }

    async fn ask<T: for<'de> Deserialize<'de>>(
        &self,
        system: &str,
        prompt: &str,
        tool: &ToolDefinition,
    ) -> Result<Result<T, String>, SimulateError> {
        let model = self
            .deps
            .models
            .resolve(&ResolveRequest::new(ModelRole::Plan));
        let answer = ask::ask_json(self.deps.llm.as_ref(), model, system, prompt, tool).await?;
        Ok(answer.and_then(|v| {
            serde_json::from_value(v)
                .map_err(|e| format!("the JSON does not match the schema: {e}"))
        }))
    }

    async fn plan(&self, doc: &SpecDoc) -> Result<AssertionPlan, SimulateError> {
        let tool =
            ToolDefinition::for_input::<AssertionPlan>("submit_assertions", prompts::PLAN_TOOL);
        let base = prompts::plan_prompt(doc);
        let mut errors: Vec<String> = Vec::new();
        for _ in 0..2 {
            let prompt = if errors.is_empty() {
                base.clone()
            } else {
                let list: String = errors.iter().map(|e| format!("\n- {e}")).collect();
                format!(
                    "{base}\n\nYour previous answer is not valid:{list}\n\nSend it again, fixed."
                )
            };
            match self
                .ask::<AssertionPlan>(prompts::PLAN_SYSTEM, &prompt, &tool)
                .await?
            {
                Ok(plan) => {
                    errors = plan.problems();
                    if errors.is_empty() {
                        return Ok(plan);
                    }
                }
                Err(e) => errors = vec![e],
            }
        }
        Err(SimulateError::Plan(errors))
    }

    /// Simulates the Spec named by `req.spec`.
    ///
    /// # Errors
    ///
    /// See [`SimulateError`].
    pub async fn run(&self, req: SimulateRequest) -> Result<SimulationReport, SimulateError> {
        let sink = self.sink();
        let folders = sink
            .folders()
            .await
            .map_err(|e| SimulateError::Spec(e.to_string()))?;
        let folder = resolve_folder(&folders, &req.spec).ok_or_else(|| {
            SimulateError::Spec(format!(
                "no Spec `{}` in {SPECS_DIR}/ (found: {})",
                req.spec,
                if folders.is_empty() {
                    "none".to_owned()
                } else {
                    folders.join(", ")
                }
            ))
        })?;
        let spec_md = self.read(&folder, "spec.md").await?.ok_or_else(|| {
            SimulateError::Spec(format!("{SPECS_DIR}/{folder}/spec.md is missing"))
        })?;
        let doc = SpecDoc::parse(&spec_md);
        if !doc.has_diagrams() {
            return Err(SimulateError::Spec(format!(
                "{folder}/spec.md has no black-box or sequence diagram to model"
            )));
        }
        let sources = self.read(&folder, "sources.json").await?;
        let evidence: Option<Vec<Evidence>> = match &sources {
            None => Some(Vec::new()),
            Some(text) => serde_json::from_str(text).ok(),
        };
        let evidence_ids: Vec<String> = evidence
            .iter()
            .flatten()
            .map(|e| e.id.to_string())
            .collect();

        self.notice(format!(
            "simulate {folder}: deriving the assertions from the test protocol"
        ));
        let plan = self.plan(&doc).await?;
        let plan_json = serde_json::to_string_pretty(&plan).unwrap_or_default();
        self.notice(format!(
            "simulate {folder}: {} scenarios frozen; writing the model",
            plan.scenarios.len()
        ));

        let tool = ToolDefinition::new(
            "submit_sysmodel",
            prompts::MODEL_TOOL,
            domain::sysmodel::schema(),
        );
        let first = prompts::model_prompt(&doc, &plan_json, &evidence_ids);
        let mut prompt = first.clone();
        let mut loosening = Vec::new();
        let mut last_errors = Vec::new();
        let mut accepted = None;
        for round in 0..=MAX_REPAIRS {
            if round > 0 {
                self.notice(format!("simulate {folder}: repair round {round}"));
            }
            let (model, errors) = match self
                .ask::<SysModel>(prompts::MODEL_SYSTEM, &prompt, &tool)
                .await?
            {
                Err(e) => (None, vec![e]),
                Ok(mut model) => {
                    let mut errors = freeze(&mut model, &plan, &mut loosening);
                    if model.spec_ref.is_none() {
                        model.spec_ref = Some(SpecRef {
                            solution: folder.clone(),
                            evidence: Vec::new(),
                        });
                    }
                    let issues = lint(&model);
                    errors.extend(
                        issues
                            .iter()
                            .filter(|i| i.severity == Severity::Error)
                            .map(ToString::to_string),
                    );
                    let runs = if errors.is_empty() {
                        match run_all(&model) {
                            Ok(runs) => Some(runs),
                            Err(e) => {
                                errors.push(e.to_string());
                                None
                            }
                        }
                    } else {
                        None
                    };
                    if let Some(runs) = runs {
                        accepted = Some((model, runs, issues, round));
                        break;
                    }
                    (Some(model), errors)
                }
            };
            let previous = model.map_or_else(|| "(no valid JSON)".to_owned(), |m| m.to_json());
            prompt = prompts::repair_prompt(&first, &previous, &errors);
            last_errors = errors;
        }
        let Some((model, runs, issues, repairs)) = accepted else {
            return Err(SimulateError::Model {
                rounds: MAX_REPAIRS,
                issues: last_errors,
            });
        };
        let mut warnings: Vec<Issue> = issues
            .into_iter()
            .filter(|i| i.severity == Severity::Warning)
            .collect();
        warnings.extend(uncovered(&model, &doc.test_items));
        let board = Board {
            spec: folder.clone(),
            title: if doc.title.is_empty() {
                folder.clone()
            } else {
                doc.title.clone()
            },
            model,
            runs,
        };
        let mut report = SimulationReport {
            folder,
            board,
            plan,
            files: Vec::new(),
            evidence: Vec::new(),
            warnings,
            repairs,
            loosening,
        };
        self.write(&sink, &mut report, evidence, req.now_ms).await?;
        if req.open_board
            && let Some(events) = &self.deps.events
        {
            events.emit(DomainEvent::SimulationReady {
                board: Box::new(report.board.clone()),
            });
        }
        Ok(report)
    }

    async fn write(
        &self,
        sink: &FsSpecSink,
        report: &mut SimulationReport,
        evidence: Option<Vec<Evidence>>,
        now_ms: u64,
    ) -> Result<(), SimulateError> {
        let folder = report.folder.clone();
        let mut put = async |file: String, content: String| -> Result<(), SimulateError> {
            sink.write(&folder, &file, &content)
                .await
                .map_err(|e| SimulateError::Write(e.to_string()))?;
            report.files.push(format!("{SPECS_DIR}/{folder}/{file}"));
            Ok(())
        };
        put(
            format!("{SIMULATION_DIR}/sysmodel.json"),
            report.board.model.to_json(),
        )
        .await?;
        put(
            format!("{SIMULATION_DIR}/assertions.json"),
            serde_json::to_string_pretty(&report.plan).unwrap_or_default() + "\n",
        )
        .await?;
        for run in &report.board.runs {
            put(
                trace_file(&run.scenario),
                serde_json::to_string_pretty(run).unwrap_or_default() + "\n",
            )
            .await?;
        }
        if let Some(export) = &self.deps.export {
            match export.board_html(&report.board) {
                Ok(html) => put(format!("{SIMULATION_DIR}/board.html"), html).await?,
                Err(e) => self.notice(format!("simulate {folder}: no board.html: {e}")),
            }
        }
        let model_hash = report.board.model.hash();
        let mut journal_lines = vec![format!(
            "Simulated model {model_hash} ({} components, {} scenarios, {} repair rounds).",
            report.board.model.components.len(),
            report.board.runs.len(),
            report.repairs
        )];
        for l in &report.loosening {
            journal_lines.push(format!(
                "Attempt to change the frozen assertions, reverted: {l}"
            ));
        }
        match evidence {
            Some(mut all) => {
                let mut ids = IdAllocator::after(
                    all.iter().map(|e| &e.id),
                    std::iter::empty(),
                    std::iter::empty(),
                    std::iter::empty(),
                );
                for run in &report.board.runs {
                    let id = ids.next_evidence();
                    all.push(Evidence {
                        id,
                        claim: claim(run, &model_hash),
                        source: SourceRef::File {
                            path: format!("{SPECS_DIR}/{folder}/{}", trace_file(&run.scenario)),
                            line_start: None,
                            line_end: None,
                        },
                        collected_at: now_ms,
                    });
                    journal_lines.push(format!(
                        "Scenario `{}` {} [{id}].",
                        run.scenario,
                        if run.passed { "passed" } else { "failed" }
                    ));
                    report.evidence.push(id);
                }
                put(
                    "sources.json".to_owned(),
                    serde_json::to_string_pretty(&all).unwrap_or_default() + "\n",
                )
                .await?;
            }
            None => {
                journal_lines.push(
                    "sources.json could not be read, so no evidence was recorded.".to_owned(),
                );
            }
        }
        let mut journal = self
            .read(&folder, "journal.md")
            .await?
            .unwrap_or_else(|| "# Research journal\n".to_owned());
        if !journal.ends_with('\n') {
            journal.push('\n');
        }
        journal.push_str("\n## Simulation\n\n");
        for line in journal_lines {
            let _ = writeln!(journal, "- `{}` {line}", utc(now_ms));
        }
        put("journal.md".to_owned(), journal).await?;
        Ok(())
    }
}

/// `simulation/traces/<scenario>.json`.
#[must_use]
pub fn trace_file(scenario: &str) -> String {
    format!("{SIMULATION_DIR}/traces/{scenario}.json")
}

/// The evidence claim of one run.
fn claim(run: &ScenarioRun, model_hash: &str) -> String {
    let total = run.verdicts.len();
    let ok = run.verdicts.iter().filter(|v| v.passed).count();
    let mut text = format!(
        "Simulated scenario `{}` (seed {}, model {model_hash}, trace {}) {}: {ok} of {total} checks held",
        run.scenario,
        run.trace.seed,
        run.trace.hash(),
        if run.passed { "passed" } else { "failed" },
    );
    let failed: Vec<String> = run
        .failures()
        .map(|v| format!("{} ({})", v.property, v.detail))
        .collect();
    if !failed.is_empty() {
        let _ = write!(text, "; failed: {}", failed.join("; "));
    }
    text.push('.');
    text
}

/// Puts the frozen scenarios back into `model`: missing ones are errors,
/// changed expectations or coverage are restored and reported.
fn freeze(model: &mut SysModel, plan: &AssertionPlan, loosening: &mut Vec<String>) -> Vec<String> {
    let mut errors = Vec::new();
    for frozen in &plan.scenarios {
        let Some(s) = model.scenarios.iter_mut().find(|s| s.id == frozen.id) else {
            errors.push(format!(
                "scenario `{}` of the frozen plan is missing; add it with its frozen expect",
                frozen.id
            ));
            continue;
        };
        if s.expect != frozen.expect {
            let removed: Vec<String> = frozen
                .expect
                .iter()
                .filter(|e| !s.expect.contains(e))
                .map(Expectation::describe)
                .collect();
            let added: Vec<String> = s
                .expect
                .iter()
                .filter(|e| !frozen.expect.contains(e))
                .map(Expectation::describe)
                .collect();
            let note = format!(
                "scenario `{}`: {}{}",
                frozen.id,
                if removed.is_empty() {
                    String::new()
                } else {
                    format!("dropped or changed [{}]", removed.join("; "))
                },
                if added.is_empty() {
                    String::new()
                } else {
                    format!(" added [{}]", added.join("; "))
                }
            );
            if !loosening.contains(&note) {
                loosening.push(note);
            }
            s.expect.clone_from(&frozen.expect);
        }
        s.covers.clone_from(&frozen.covers);
        if s.description.is_empty() {
            s.description.clone_from(&frozen.description);
        }
    }
    errors
}
