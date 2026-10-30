//! Node 8, Research: prior solutions to the defined problem.
//!
//! Two sources, both with the `explore` role:
//!
//! - **the repo**: a turn of the agent ([`NodeContext::agent`]) told to
//!   only read, looking for patterns and pieces already present; each
//!   finding becomes evidence with its file and lines;
//! - **the web**: a turn of the read-only agent with `web_search` and
//!   `web_fetch` ([`explore_web`]), looking for libraries, articles and
//!   issues; every page it read becomes evidence ([`citations_to_evidence`]),
//!   and so does the page of each option it did not read. Without the web
//!   tools (no curl, `app.opt.web.enabled = false`) the web is skipped.
//!
//! The options found are shown as a table with their sources and stored
//! under [`OPTIONS_KEY`] for the brainstorm and the modeling. Without a
//! problem definition the flow goes back to it.

use std::fmt::Write as _;

use async_trait::async_trait;
use domain::block::{BlockKind, Cell, Column, SourceRef, TableSpec};
use domain::conversation::Message;
use domain::research::{EvidenceId, Stage};
use serde::{Deserialize, Serialize};

use super::observation::{explore_web, has_web};
use crate::flows::research::json::final_text;
use crate::flows::research::node::{NodeContext, NodeError, NodeOutcome, ResearchNode};
use crate::ports::llm::Citation;
use crate::usecases::evidence::citations_to_evidence;

/// Key of the prior solutions found (`Vec<PriorSolution>`).
pub const OPTIONS_KEY: &str = "investigation.options";

const REPO_PROMPT: &str = "\
You are the \"Research\" node of an investigation of this repository. YOU MAY ONLY READ: \
use list_files, find_file, grep, tree and read_file; do not edit, write files or run \
commands. Look in the repo for prior solutions to the problem: patterns, modules, utilities or \
dependencies already present that partly solve it or can serve as a model.
When you finish, answer only with JSON:
{\"findings\": [{\"name\": \"…\", \"summary\": \"one line\", \"path\": \"relative/path.rs\", \
\"line_start\": 10, \"line_end\": 40}]}
line_start and line_end may be null. If there is nothing, return \"findings\": [].";

const WEB_SYSTEM: &str = "\
You are the \"Research\" node of an investigation of a software project. Search the web \
with web_search for prior solutions to the problem: libraries, articles and issues of similar \
projects. Read the most relevant results with web_fetch (up to 5 pages) and search again with \
other words if the first results fall short. Do not make up URLs: include only pages you found.
When you finish, answer only with JSON:
{\"options\": [{\"name\": \"…\", \"kind\": \"library\", \"summary\": \"one line\", \
\"url\": \"https://…\"}]}
kind is library, article, issue or pattern. If there is nothing, return \"options\": [].";

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
    fn name(self) -> &'static str {
        match self {
            Self::Library => "library",
            Self::Article => "article",
            Self::Issue => "issue",
            Self::Pattern => "pattern",
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

/// What the repo turn found, before it is recorded
/// ([`Research::gather_repo`]).
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct RepoAnswer {
    findings: Vec<RepoFinding>,
}

#[derive(Debug, Serialize, Deserialize)]
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

#[derive(Debug, Serialize, Deserialize)]
struct WebAnswer {
    options: Vec<WebOption>,
}

/// What the web turn found, before it is recorded
/// ([`Research::gather_web`]).
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct WebGathered {
    answer: WebAnswer,
    citations: Vec<Citation>,
}

#[derive(Debug, Serialize, Deserialize)]
struct WebOption {
    name: String,
    kind: SolutionKind,
    summary: String,
    url: String,
}

/// The Research node.
#[derive(Debug, Default)]
pub struct Research;

impl Research {
    /// The node.
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Looks in the repo for prior solutions to `brief` with a read-only
    /// agent turn; `None` when there is no agent with tools. Only reads the
    /// context.
    ///
    /// # Errors
    ///
    /// The turn's errors or the invalid answer.
    pub(crate) async fn gather_repo(
        &self,
        ctx: &NodeContext,
        brief: &str,
    ) -> Result<Option<RepoAnswer>, NodeError> {
        let Ok(agent) = ctx.explore_agent() else {
            return Ok(None);
        };
        let prompt = format!("{REPO_PROMPT}\n\n{brief}");
        let mut session = ctx.new_session();
        ctx.run_agent_turn(agent, &mut session, prompt.clone())
            .await?;
        let report = final_text(&session);
        let answer: RepoAnswer = ctx
            .fix_json(
                "",
                vec![Message::user_text(prompt)],
                report,
                |a: &RepoAnswer| {
                    a.findings.iter().try_for_each(|f| {
                        if f.name.trim().is_empty() || f.path.trim().is_empty() {
                            return Err("every finding needs `name` and `path`".to_owned());
                        }
                        f.source().validate().map_err(|e| e.to_string())
                    })
                },
            )
            .await?;
        Ok(Some(answer))
    }

    /// Records the findings of [`Research::gather_repo`] as evidence and
    /// returns them as prior solutions.
    pub(crate) fn apply_repo(
        &self,
        ctx: &mut NodeContext,
        gathered: Option<RepoAnswer>,
    ) -> Vec<PriorSolution> {
        let Some(answer) = gathered else {
            ctx.log("No agent with tools: the repo was not explored.");
            return Vec::new();
        };
        let now = ctx.now();
        answer
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
            .collect()
    }

    /// Searches the web for libraries, articles and issues about `brief`;
    /// `None` without web tools. Only reads the context.
    ///
    /// # Errors
    ///
    /// The turn's errors or the invalid answer.
    pub(crate) async fn gather_web(
        &self,
        ctx: &NodeContext,
        brief: &str,
    ) -> Result<Option<WebGathered>, NodeError> {
        if !has_web(ctx) {
            return Ok(None);
        }
        let prompt = format!("{WEB_SYSTEM}\n\n{brief}");
        let (text, citations) = explore_web(ctx, &prompt).await?;
        let messages = vec![Message::user_text(prompt)];
        let answer: WebAnswer = ctx
            .fix_json("", messages, text, |a: &WebAnswer| {
                a.options.iter().try_for_each(|o| {
                    if o.name.trim().is_empty() || o.summary.trim().is_empty() {
                        Err("every option needs `name` and `summary`".to_owned())
                    } else if !o.url.starts_with("http") {
                        Err(format!(
                            "the URL of \"{}\" is not valid: {:?}",
                            o.name, o.url
                        ))
                    } else {
                        Ok(())
                    }
                })
            })
            .await?;
        Ok(Some(WebGathered { answer, citations }))
    }

    /// Records what [`Research::gather_web`] found: the pages read and the
    /// page of each option not read become evidence; returns the options.
    pub(crate) fn apply_web(
        &self,
        ctx: &mut NodeContext,
        gathered: Option<WebGathered>,
    ) -> Vec<PriorSolution> {
        let Some(WebGathered { answer, citations }) = gathered else {
            ctx.log("No web tools (curl or app.opt.web): the web was not searched.");
            return Vec::new();
        };
        let now = ctx.now();
        let cited = citations_to_evidence(&citations, ctx.state_mut(), now);
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
        options
    }
}

#[async_trait]
impl ResearchNode for Research {
    fn stage(&self) -> Stage {
        Stage::Research
    }

    async fn run(&self, ctx: &mut NodeContext) -> Result<NodeOutcome, NodeError> {
        if ctx.state().problem_statements.is_empty() {
            let reason = "there is no problem definition to research";
            ctx.log(format!(
                "Going back to {}: {reason}.",
                Stage::Definition.name()
            ));
            return Ok(NodeOutcome::Back {
                to: Stage::Definition,
                reason: reason.into(),
            });
        }
        let brief = brief(ctx);
        let first_evidence = ctx.state().evidence.len();
        let repo = self.gather_repo(ctx, &brief).await?;
        let mut options = self.apply_repo(ctx, repo);
        let repo_count = options.len();
        let web = self.gather_web(ctx, &brief).await?;
        options.extend(self.apply_web(ctx, web));

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
            "{} prior solution(s): {repo_count} in the repo and {} on the web; {new_evidence} \
             new piece(s) of evidence.",
            options.len(),
            options.len() - repo_count,
        ));
        Ok(NodeOutcome::Done)
    }
}

/// The definition, constraints and hypotheses, for both prompts.
pub(crate) fn brief(ctx: &NodeContext) -> String {
    let state = ctx.state();
    let mut out = String::from("Problem:\n");
    for p in &state.problem_statements {
        let _ = writeln!(out, "- {p}");
    }
    if !state.constraints.is_empty() {
        out.push_str("\nConstraints:\n");
        for c in &state.constraints {
            let _ = writeln!(out, "- {}: {}", c.id, c.statement);
        }
    }
    if !state.hypotheses.is_empty() {
        out.push_str("\nHypotheses:\n");
        for h in &state.hypotheses {
            let _ = writeln!(out, "- {}: {}", h.id, h.statement);
        }
    }
    out
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
        other => other.reference(),
    }
}

/// The table of prior solutions shown to the user.
pub(crate) fn table(options: &[PriorSolution]) -> TableSpec {
    TableSpec {
        columns: ["Option", "Kind", "Summary", "Source", "Evidence"]
            .into_iter()
            .map(Column::new)
            .collect(),
        rows: options
            .iter()
            .map(|o| {
                let ids: Vec<String> = o.evidence_ids.iter().map(ToString::to_string).collect();
                vec![
                    Cell::Text(o.name.clone()),
                    Cell::Text(o.kind.name().into()),
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
