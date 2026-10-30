//! Stage 3, Data (optional): gathers evidence about the problem.
//!
//! - The read-only agent reads the history of the files Observation marked
//!   as key (`git log` when the shell is available), logs and metrics in
//!   the repository and, when MCP tools (`mcp__*`) are offered, issues.
//! - When the agent has the web tools, it looks up the documentation of
//!   the libraries involved with `web_search` and `web_fetch`; the pages it
//!   read become evidence with
//!   [`citations_to_evidence`](crate::usecases::evidence::citations_to_evidence).
//!
//! The node shows a `Chart` of commits per week when the history has
//! dates, a `Table` of the evidence it collected and a `Sources` block,
//! and stores a [`DataReport`] under [`REPORT_KEY`]. Without the outputs
//! of Intake or Observation it goes back to them.

use std::fmt::Write as _;

use async_trait::async_trait;
use domain::block::{BlockKind, Cell, ChartKind, ChartSpec, Column, Series, SourceRef, TableSpec};
use domain::research::{EvidenceId, Stage};
use serde::{Deserialize, Serialize};

use super::intake::{BRIEF_KEY, IntakeBrief};
use super::observation::{
    self, ObservationReport, explore_json, explore_web, has_shell, has_web, sources_block,
    validate_source,
};
use super::spec::record_charts;
use crate::flows::research::node::{NodeContext, NodeError, NodeOutcome, ResearchNode};
use crate::ports::events::NoticeLevel;
use crate::ports::llm::Citation;
use crate::usecases::evidence::citations_to_evidence;

/// Flow data key of the [`DataReport`].
pub const REPORT_KEY: &str = "data.report";

/// Commits recorded as evidence; the chart counts all of them.
const MAX_COMMIT_EVIDENCE: usize = 20;

/// Weeks shown in the chart, the most recent ones.
const MAX_WEEKS: usize = 52;

const TASK: &str = "Data stage of an investigation of this repository. Gather \
evidence about the problem READ-ONLY, without changing anything: the change history of the \
files involved, logs, metrics or benchmarks that exist in the repository and, if there are \
issue tools, the related issues. Do not propose solutions.";

const GIT: &str = "Use `run_command` only with read-only git commands; for the history: \
`git log --date=short --format=%H%x09%ad%x09%s -n 50 -- <files>`.";

const NO_GIT: &str = "You have no access to git: leave `commits` empty.";

const NO_FILES: &str = "The files involved are not known yet: find them first (with `tree`, \
`glob` or `grep`), then gather the data about them.\n";

const FORMAT: &str = "When you finish, answer only with a JSON object, with no text around it:
{\"commits\": [{\"sha\": \"…\", \"date\": \"YYYY-MM-DD\", \"summary\": \"first line\"}],
 \"findings\": [{\"claim\": \"fact found, in one sentence\", \"source\": \
{\"type\": \"file\", \"path\": \"…\", \"line_start\": 1} | {\"type\": \"url\", \"url\": \"…\", \
\"title\": \"…\"} | {\"type\": \"command\", \"command\": \"…\", \"output_excerpt\": \"…\"}}],
 \"libraries\": [\"external libraries involved whose documentation is worth checking\"]}
Use empty lists when there is nothing.";

const WEB_SYSTEM: &str = "You are investigating a software problem. Search the web with \
web_search for the official documentation, changelogs and known issues of the given libraries \
that are relevant to the problem, and read the most relevant pages with web_fetch (up to 4). \
Answer in 3 to 5 short bullets and give the URL of each source.";

/// What Data gathered, read by Analysis.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataReport {
    /// Evidence recorded by the stage.
    pub evidence: Vec<EvidenceId>,
    /// Commits per week: the Monday of each week (`YYYY-MM-DD`) and its
    /// count, oldest first.
    pub commits_per_week: Vec<(String, u32)>,
    /// Libraries whose documentation was looked up.
    pub libraries: Vec<String>,
    /// Summary of the web research, when there was one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_summary: Option<String>,
}

/// A commit of the history.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Commit {
    sha: String,
    #[serde(default)]
    date: String,
    summary: String,
}

/// A fact found by the agent.
#[derive(Debug, Serialize, Deserialize)]
struct Finding {
    claim: String,
    source: SourceRef,
}

/// What the agent answers.
#[derive(Debug, Serialize, Deserialize)]
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
                return Err(format!("commit {i} has no `sha` or `summary`"));
            }
            if !c.date.trim().is_empty() && day_number(&c.date).is_none() {
                return Err(format!("commit {i} has an invalid date: {}", c.date));
            }
        }
        for (i, f) in self.findings.iter().enumerate() {
            if f.claim.trim().is_empty() {
                return Err(format!("finding {i} has no `claim`"));
            }
            validate_source(&f.source).map_err(|e| format!("finding {i}: {e}"))?;
        }
        Ok(())
    }
}

/// What the agent found in the repository, before it is recorded
/// ([`Data::gather_repo`]).
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct DataAnswer {
    answer: Answer,
    /// Whether issue tools (MCP) were offered.
    mcp: bool,
}

impl DataAnswer {
    /// The libraries whose documentation is worth checking.
    pub(crate) fn libraries(&self) -> Vec<String> {
        self.answer
            .libraries
            .iter()
            .map(|l| l.trim().to_owned())
            .filter(|l| !l.is_empty())
            .collect()
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

    /// Gathers history, logs, metrics and issues with the read-only agent,
    /// about the files `observed` marks (or, without them, the ones the
    /// agent finds). Only reads the context.
    ///
    /// # Errors
    ///
    /// [`NodeError::Unavailable`] without an agent, the turn's errors, or
    /// the invalid answer.
    pub(crate) async fn gather_repo(
        &self,
        ctx: &NodeContext,
        brief: &IntakeBrief,
        observed: Option<&ObservationReport>,
    ) -> Result<DataAnswer, NodeError> {
        let git = has_shell(ctx);
        let mcp: Vec<String> = ctx
            .explore_agent()?
            .tools()
            .names()
            .into_iter()
            .filter(|name| name.starts_with("mcp__"))
            .collect();
        let mcp_names: Vec<&str> = mcp.iter().map(String::as_str).collect();
        let answer: Answer = explore_json(
            ctx,
            &input(brief, observed, git, &mcp_names),
            Answer::validate,
        )
        .await?;
        Ok(DataAnswer {
            answer,
            mcp: !mcp.is_empty(),
        })
    }

    /// Looks up the documentation of `libraries` on the web; `None` when
    /// there are none or the agent has no web tools.
    async fn library_docs(
        ctx: &NodeContext,
        brief: &IntakeBrief,
        libraries: &[String],
    ) -> Option<Result<(String, Vec<Citation>), NodeError>> {
        if libraries.is_empty() || !has_web(ctx) {
            return None;
        }
        let prompt = format!(
            "{WEB_SYSTEM}\n\nProblem: {}\nLibraries: {}",
            brief.restated,
            libraries.join(", ")
        );
        Some(explore_web(ctx, &prompt).await)
    }

    /// Records what [`Data::gather_repo`] found and, when it ran, the web
    /// research about the libraries: evidence, the chart, the table and
    /// sources, the journal entry and the [`DataReport`] under
    /// [`REPORT_KEY`]. A failed web search is only a warning.
    ///
    /// # Errors
    ///
    /// A block or the report that cannot be stored.
    pub(crate) fn apply(
        &self,
        ctx: &mut NodeContext,
        gathered: DataAnswer,
        docs: Option<Result<(String, Vec<Citation>), NodeError>>,
    ) -> Result<DataReport, NodeError> {
        let libraries = gathered.libraries();
        let DataAnswer { answer, mcp } = gathered;
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
                    "Change in the files involved{date}: {}",
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

        let mut web_summary = None;
        let mut web = 0;
        match docs {
            Some(Ok((text, citations))) => {
                web = citations.len();
                for id in citations_to_evidence(&citations, ctx.state_mut(), now) {
                    if !evidence.contains(&id) {
                        evidence.push(id);
                    }
                }
                web_summary = Some(text).filter(|t| !t.is_empty());
            }
            Some(Err(err)) => {
                let message = format!("Could not search the web: {err}");
                ctx.notice(NoticeLevel::Warning, &message);
                ctx.log(message);
            }
            None => {}
        }

        let weeks = commits_per_week(&answer.commits);
        if !weeks.is_empty() {
            let chart = chart(&weeks);
            ctx.emit(BlockKind::Chart(chart.clone()))?;
            record_charts(ctx, &[chart])?;
        }
        let records: Vec<_> = evidence
            .iter()
            .filter_map(|id| ctx.state().evidence(*id).cloned())
            .collect();
        if records.is_empty() {
            ctx.emit(BlockKind::Markdown {
                title: None,
                source: "No additional data found: no history, logs, metrics or \
                         relevant documentation."
                    .into(),
            })?;
        } else {
            ctx.emit(BlockKind::Table(TableSpec {
                columns: vec![
                    Column::new("Id"),
                    Column::new("Kind"),
                    Column::new("Fact"),
                    Column::new("Source"),
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
            "Data gathered: {} commits over {} weeks, {findings} findings in the repo{}, \
             {web} web citations{}; evidence {}.",
            answer.commits.len(),
            weeks.len(),
            if mcp { " (with MCP)" } else { "" },
            if libraries.is_empty() {
                String::new()
            } else {
                format!(" about {}", libraries.join(", "))
            },
            if ids.is_empty() {
                "none".to_owned()
            } else {
                ids.join(", ")
            },
        ));
        let report = DataReport {
            evidence,
            commits_per_week: weeks,
            libraries,
            web_summary,
        };
        ctx.set_data(REPORT_KEY, &report)?;
        Ok(report)
    }
}

fn input(
    brief: &IntakeBrief,
    observed: Option<&ObservationReport>,
    git: bool,
    mcp: &[&str],
) -> String {
    let mut out = format!(
        "{TASK}\n\nProblem ({}): {}\n",
        brief.kind.name(),
        brief.restated,
    );
    match observed {
        Some(observed) => {
            let _ = writeln!(
                out,
                "Observed architecture: {}\nFiles involved:",
                observed.summary
            );
            for file in &observed.key_files {
                let _ = writeln!(out, "- {} ({})", file.path, file.role);
            }
        }
        None => out.push_str(NO_FILES),
    }
    let _ = write!(out, "\n{}\n", if git { GIT } else { NO_GIT });
    if !mcp.is_empty() {
        let _ = writeln!(
            out,
            "To look for related issues, use only queries of these MCP tools: {}.",
            mcp.join(", ")
        );
    }
    let _ = write!(out, "\n{FORMAT}");
    out
}

/// Days since 1970-01-01 of a `YYYY-MM-DD` date (anything after the day,
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

/// The `YYYY-MM-DD` date of a day number.
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
        title: "Commits per week in the files involved".into(),
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
        other => other.reference(),
    }
}

fn source_kind(source: &SourceRef) -> &'static str {
    match source {
        SourceRef::File { .. } => "file",
        SourceRef::Commit { .. } => "commit",
        SourceRef::Url { .. } => "web",
        SourceRef::Command { .. } => "command",
        SourceRef::Paper { .. } => "paper",
        SourceRef::Book { .. } => "book",
        SourceRef::Standard { .. } => "standard",
        SourceRef::Docs { .. } => "docs",
        SourceRef::Repo { .. } => "repo",
        SourceRef::Article { .. } => "article",
    }
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
                reason: "the problem is not defined: the restated request from Intake is missing"
                    .into(),
            });
        };
        let Some(observed) = ctx.data::<ObservationReport>(observation::REPORT_KEY) else {
            return Ok(NodeOutcome::Back {
                to: Stage::Observation,
                reason: "the files involved that Observation marks are missing".into(),
            });
        };
        let gathered = self.gather_repo(ctx, &brief, Some(&observed)).await?;
        let docs = Self::library_docs(ctx, &brief, &gathered.libraries()).await;
        self.apply(ctx, gathered, docs)?;
        Ok(NodeOutcome::Done)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::ports::llm::LlmError;

    use domain::model::ModelRole;
    use domain::permission::ToolEffect;
    use serde_json::json;

    use super::observation::testing::{agent, blocks, brief, context, workspace};
    use super::observation::{KeyFile, ObservationReport};
    use super::*;
    use crate::ports::mcp::McpTool as McpSpec;
    use crate::ports::shell::CommandOutput;
    use crate::ports::web::{SearchHit, WebPage};
    use crate::testing::fs::ScriptedShell;
    use crate::testing::llm::{FakeMcp, ScriptedLlm, ScriptedResponse};
    use crate::testing::system::RecordingSink;
    use crate::testing::web::ScriptedWeb;
    use crate::tools::mcp::McpTool;
    use crate::tools::web::register_web_tools;

    fn observed() -> ObservationReport {
        ObservationReport {
            summary: "A crate with the login.".into(),
            architecture: "flowchart TD\n  login --> hash".into(),
            key_files: vec![KeyFile {
                path: "src/login.rs".into(),
                kind: "code".into(),
                role: "logs in".into(),
            }],
            evidence: vec![],
        }
    }

    const DOCS: &str = "https://docs.rs/argon2";

    fn web() -> Arc<ScriptedWeb> {
        Arc::new(
            ScriptedWeb::new()
                .on_search(
                    "argon2 parameters",
                    vec![SearchHit {
                        title: "argon2 - docs.rs".into(),
                        url: DOCS.into(),
                        snippet: "Pure Rust Argon2".into(),
                    }],
                )
                .on_fetch(
                    DOCS,
                    WebPage {
                        url: DOCS.into(),
                        title: Some("argon2 - docs.rs".into()),
                        text: "The default parameters use 19 MiB of memory.".into(),
                    },
                ),
        )
    }

    /// The web turn: a search, a page, then the bullets.
    fn web_turn() -> [ScriptedResponse; 3] {
        [
            ScriptedResponse::tool_use("w1", "web_search", json!({"query": "argon2 parameters"})),
            ScriptedResponse::tool_use("w2", "web_fetch", json!({"url": DOCS})),
            ScriptedResponse::text("- argon2 is slow by design."),
        ]
    }

    fn agent_answer() -> String {
        json!({
            "commits": [
                {"sha": "aaa1111222", "date": "2026-09-28", "summary": "use argon2"},
                {"sha": "bbb3333444", "date": "2026-10-01", "summary": "raise the cost"},
                {"sha": "ccc5555666", "date": "2026-10-14", "summary": "add log"}
            ],
            "findings": [
                {"claim": "Issue #12 reports 6 s logins.",
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
        ]));
        for response in web_turn() {
            llm.push(response);
        }
        let web = web();
        let shell = Arc::new(ScriptedShell::new().on(
            "git",
            git_log,
            CommandOutput {
                status: Some(0),
                stdout: "aaa1111222\t2026-09-28\tuse argon2\n".into(),
                ..CommandOutput::default()
            },
        ));
        let mcp =
            Arc::new(FakeMcp::new().with_text_tool("github", "search_issues", "#12 slow login"));
        let tools_web = web.clone();
        let service = agent(&llm, &workspace(), Some(shell.clone()), |tools| {
            register_web_tools(tools, tools_web.clone(), 5).unwrap();
            let spec = McpSpec {
                server: "github".into(),
                name: "search_issues".into(),
                description: "Searches issues".into(),
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
        assert_eq!(web.fetches(), [DOCS]);
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
        assert!(matches!(&state.evidence[4].source, SourceRef::Url { url, .. } if url == DOCS));
        assert_eq!(
            state.evidence[4].claim,
            "The default parameters use 19 MiB of memory."
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
            Some("- argon2 is slow by design.")
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
        assert!(format!("{:?}", llm.requests()[0].messages).contains("You have no access to git"));
        assert!(ctx.state().evidence.is_empty());
        let shown = blocks(&sink);
        assert!(
            matches!(shown.as_slice(), [BlockKind::Markdown { .. }]),
            "{shown:#?}"
        );
        assert!(ctx.data::<DataReport>(REPORT_KEY).is_some());
    }

    #[tokio::test]
    async fn without_observation_the_agent_finds_the_files_itself() {
        let llm = Arc::new(ScriptedLlm::new([ScriptedResponse::text(
            r#"{"commits": [], "findings": [{"claim": "login hashes twice", "source": {"type": "file", "path": "src/login.rs", "line_start": 2}}], "libraries": ["bcrypt"]}"#,
        )]));
        let service = agent(&llm, &workspace(), None, |_| {});
        let sink = Arc::new(RecordingSink::new());
        let mut ctx = context(Stage::Data, &llm, &sink, Some(service));

        let data = Data::new();
        let gathered = data.gather_repo(&ctx, &brief(), None).await.unwrap();
        let prompt = format!("{:?}", llm.requests()[0].messages);
        assert!(prompt.contains("find them first"), "{prompt}");
        assert!(!prompt.contains("Files involved"));
        assert_eq!(gathered.libraries(), ["bcrypt"]);

        let report = data.apply(&mut ctx, gathered, None).unwrap();
        assert_eq!(report.evidence.len(), 1);
        assert_eq!(report.libraries, ["bcrypt"]);
        assert_eq!(ctx.data::<DataReport>(REPORT_KEY), Some(report));
    }

    #[tokio::test]
    async fn a_failed_web_search_is_a_warning() {
        let llm = Arc::new(ScriptedLlm::new([
            ScriptedResponse::text(r#"{"libraries": ["argon2"]}"#),
            ScriptedResponse::open_error(LlmError::Overloaded),
        ]));
        let service = agent(&llm, &workspace(), None, |tools| {
            register_web_tools(tools, web(), 5).unwrap();
        });
        let sink = Arc::new(RecordingSink::new());
        let mut ctx = context(Stage::Data, &llm, &sink, Some(service));
        ctx.set_data(BRIEF_KEY, brief()).unwrap();
        ctx.set_data(observation::REPORT_KEY, observed()).unwrap();

        assert_eq!(Data::new().run(&mut ctx).await.unwrap(), NodeOutcome::Done);
        assert!(
            ctx.state().journal[0]
                .text
                .contains("Could not search the web")
        );
    }

    #[tokio::test]
    async fn without_web_tools_the_docs_are_not_searched() {
        let llm = Arc::new(ScriptedLlm::new([ScriptedResponse::text(
            r#"{"libraries": ["argon2"]}"#,
        )]));
        let service = agent(&llm, &workspace(), None, |_| {});
        let sink = Arc::new(RecordingSink::new());
        let mut ctx = context(Stage::Data, &llm, &sink, Some(service));
        ctx.set_data(BRIEF_KEY, brief()).unwrap();
        ctx.set_data(observation::REPORT_KEY, observed()).unwrap();

        assert_eq!(Data::new().run(&mut ctx).await.unwrap(), NodeOutcome::Done);
        assert_eq!(llm.requests().len(), 1, "no web turn");
        let report: DataReport = ctx.data(REPORT_KEY).unwrap();
        assert_eq!(report.web_summary, None);
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
        assert_eq!(day_number("yesterday"), None);
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
