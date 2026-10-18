//! Node 4, Analysis: contrasts the ideas gathered so far with the data.
//!
//! It sends the collected evidence to the `main` model and asks for a JSON
//! report: findings (profiles, benchmarks, counts, timelines), each citing
//! known evidence ids, plus numeric series for charts. Findings are shown
//! as a `Table` and stored under [`KEY_FINDINGS`]; every series becomes a
//! `Chart`. Without evidence the flow goes back to Data (or Observation
//! when the graph has no Data).

use std::fmt::Write as _;

use async_trait::async_trait;
use domain::block::{BlockKind, Cell, ChartKind, ChartSpec, Column, Series, SourceRef, TableSpec};
use domain::research::{EvidenceId, ResearchState, Stage};
use serde::{Deserialize, Serialize};

use super::spec::record_charts;
use crate::flows::research::node::{NodeContext, NodeError, NodeOutcome, ResearchNode};

/// Flow data key of the findings (`Vec<Finding>`).
pub const KEY_FINDINGS: &str = "analysis.findings";

/// Journal entries of earlier stages sent as context.
const MAX_JOURNAL_ENTRIES: usize = 12;

const SYSTEM: &str = "\
You are the Analysis stage of an investigation of a software project. \
Contrast the ideas (the request and what was observed) with the data: profiles, benchmarks, \
counts and timelines. Do not make up data: every finding and every chart cites evidence ids \
from the list (E1, E2…). Answer only with a JSON object, with no extra text.";

const SCHEMA: &str = r#"Answer format:
{
  "findings": [
    {
      "idea": "idea or assumption being contrasted",
      "statement": "finding in one sentence, with numbers if there are any",
      "verdict": "supports | contradicts | nuances",
      "kind": "profile | benchmark | count | timeline | other",
      "evidence_ids": ["E1"]
    }
  ],
  "charts": [
    {
      "title": "title",
      "kind": "bar | line | area | pie",
      "labels": ["label"],
      "series": [{ "name": "series", "values": [1.0] }],
      "evidence_ids": ["E1"]
    }
  ]
}
"findings" has at least one element; "charts" may be empty if there are no numbers. \
Each series has one value per label; "pie" takes a single series."#;

/// Whether the data backs the idea it is contrasted with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Verdict {
    /// The data backs the idea.
    #[serde(rename = "supports", alias = "confirma")]
    Supports,
    /// The data contradicts the idea.
    #[serde(rename = "contradicts", alias = "contradice")]
    Contradicts,
    /// The data backs it only in part.
    #[serde(rename = "nuances", alias = "matiza")]
    Nuances,
}

impl Verdict {
    fn label(self) -> &'static str {
        match self {
            Self::Supports => "Supports",
            Self::Contradicts => "Contradicts",
            Self::Nuances => "Nuances",
        }
    }
}

/// What kind of analysis produced a finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum FindingKind {
    /// Where the time or memory goes.
    #[serde(rename = "profile", alias = "perfil")]
    Profile,
    /// A measurement under load.
    #[serde(rename = "benchmark")]
    Benchmark,
    /// How often something happens.
    #[serde(rename = "count", alias = "conteo")]
    Count,
    /// When something changed.
    #[serde(rename = "timeline", alias = "linea_de_tiempo")]
    Timeline,
    /// Anything else.
    #[default]
    #[serde(rename = "other", alias = "otro", other)]
    Other,
}

impl FindingKind {
    fn label(self) -> &'static str {
        match self {
            Self::Profile => "Profile",
            Self::Benchmark => "Benchmark",
            Self::Count => "Count",
            Self::Timeline => "Timeline",
            Self::Other => "Other",
        }
    }
}

/// A finding of the analysis, backed by evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    /// Idea or assumption contrasted with the data.
    #[serde(default)]
    pub idea: String,
    /// The finding, in one sentence.
    pub statement: String,
    /// Whether the data backs the idea.
    pub verdict: Verdict,
    /// Kind of analysis.
    #[serde(default)]
    pub kind: FindingKind,
    /// Evidence it relies on; never empty.
    pub evidence_ids: Vec<EvidenceId>,
}

#[derive(Debug, Clone, Deserialize)]
struct ChartOut {
    title: String,
    kind: ChartKind,
    labels: Vec<String>,
    series: Vec<Series>,
    evidence_ids: Vec<EvidenceId>,
}

#[derive(Debug, Clone, Deserialize)]
struct Report {
    findings: Vec<Finding>,
    #[serde(default)]
    charts: Vec<ChartOut>,
}

/// The Analysis node.
#[derive(Debug, Default)]
pub struct Analysis;

impl Analysis {
    /// The node.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ResearchNode for Analysis {
    fn stage(&self) -> Stage {
        Stage::Analysis
    }

    async fn run(&self, ctx: &mut NodeContext) -> Result<NodeOutcome, NodeError> {
        if ctx.state().evidence.is_empty() {
            let to = ctx.first_stage_of(&[Stage::Data, Stage::Observation]);
            return Ok(NodeOutcome::Back {
                to,
                reason: "there is no evidence to analyze; data about the project is needed".into(),
            });
        }
        let prompt = format!(
            "{}\n\n{SCHEMA}",
            context_prompt(ctx.problem(), ctx.state(), ctx.user_reply())
        );
        let state = ctx.state().clone();
        let report: Report = ctx.ask_json(SYSTEM, &prompt, |r| check(r, &state)).await?;

        ctx.emit(BlockKind::Table(findings_table(&report.findings)))?;
        let mut charts: Vec<ChartSpec> = report
            .charts
            .into_iter()
            .map(|c| ChartSpec {
                title: c.title,
                kind: c.kind,
                labels: c.labels,
                series: c.series,
            })
            .collect();
        if charts.is_empty() {
            charts.push(evidence_by_source(ctx.state()));
        }
        for chart in &charts {
            ctx.emit(BlockKind::Chart(chart.clone()))?;
        }
        record_charts(ctx, &charts)?;
        ctx.set_data(KEY_FINDINGS, &report.findings)?;

        let mut cited: Vec<EvidenceId> = Vec::new();
        for id in report.findings.iter().flat_map(|f| &f.evidence_ids) {
            if !cited.contains(id) {
                cited.push(*id);
            }
        }
        let cited: Vec<String> = cited.iter().map(ToString::to_string).collect();
        ctx.log(format!(
            "Analysis: {} findings and {} charts; evidence cited: {}.",
            report.findings.len(),
            charts.len(),
            cited.join(", ")
        ));
        Ok(NodeOutcome::Done)
    }
}

/// Checks that the report has findings, cites only known evidence and
/// that its charts validate.
fn check(report: &Report, state: &ResearchState) -> Result<(), String> {
    if report.findings.is_empty() {
        return Err("\"findings\" is empty".into());
    }
    for (i, finding) in report.findings.iter().enumerate() {
        if finding.statement.trim().is_empty() {
            return Err(format!("finding {} has no \"statement\"", i + 1));
        }
        cites_known(state, &finding.evidence_ids, &format!("finding {}", i + 1))?;
    }
    for chart in &report.charts {
        cites_known(
            state,
            &chart.evidence_ids,
            &format!("chart {:?}", chart.title),
        )?;
        ChartSpec {
            title: chart.title.clone(),
            kind: chart.kind,
            labels: chart.labels.clone(),
            series: chart.series.clone(),
        }
        .validate()
        .map_err(|e| format!("chart {:?} is invalid: {e}", chart.title))?;
    }
    Ok(())
}

/// Checks that `ids` is not empty and names only collected evidence.
pub(super) fn cites_known(
    state: &ResearchState,
    ids: &[EvidenceId],
    what: &str,
) -> Result<(), String> {
    if ids.is_empty() {
        return Err(format!("{what} cites no evidence"));
    }
    state.cites_only_known_evidence(ids).map_err(|unknown| {
        let unknown: Vec<String> = unknown.iter().map(ToString::to_string).collect();
        format!(
            "{what} cites evidence that does not exist: {}",
            unknown.join(", ")
        )
    })
}

/// The problem, the journal of earlier stages and the evidence, as text.
pub(super) fn context_prompt(problem: &str, state: &ResearchState, reply: Option<&str>) -> String {
    let mut out = format!("User request: {problem}\n");
    if !state.problem_statements.is_empty() {
        let _ = writeln!(out, "Problem: {}", state.problem_statements.join(" / "));
    }
    let skip = state.journal.len().saturating_sub(MAX_JOURNAL_ENTRIES);
    if state.journal.len() > skip {
        out.push_str("\nResearch journal:\n");
        for entry in &state.journal[skip..] {
            let _ = writeln!(out, "- [{}] {}", entry.stage.name(), entry.text);
        }
    }
    out.push_str("\nEvidence:\n");
    for evidence in &state.evidence {
        let _ = writeln!(
            out,
            "- {} ({}): {}",
            evidence.id,
            describe_source(&evidence.source),
            evidence.claim
        );
    }
    if let Some(reply) = reply {
        let _ = writeln!(out, "\nUser instructions: {reply}");
    }
    out
}

/// One-line description of where evidence came from.
pub(super) fn describe_source(source: &SourceRef) -> String {
    match source {
        SourceRef::File {
            path,
            line_start: Some(start),
            line_end,
        } => match line_end {
            Some(end) if end != start => format!("file {path}:{start}-{end}"),
            _ => format!("file {path}:{start}"),
        },
        SourceRef::File { path, .. } => format!("file {path}"),
        SourceRef::Commit { sha, summary } => format!("commit {sha} \"{summary}\""),
        SourceRef::Url { url, .. } => format!("web {url}"),
        SourceRef::Command {
            command,
            output_excerpt,
        } => {
            let excerpt: String = output_excerpt.chars().take(300).collect();
            format!("command `{command}`, output: {excerpt}")
        }
    }
}

fn text(s: impl Into<String>) -> Cell {
    Cell::Text(s.into())
}

fn ids_text(ids: &[EvidenceId]) -> String {
    ids.iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

fn findings_table(findings: &[Finding]) -> TableSpec {
    TableSpec {
        columns: ["Idea", "Finding", "Verdict", "Kind", "Evidence"]
            .into_iter()
            .map(Column::new)
            .collect(),
        rows: findings
            .iter()
            .map(|f| {
                vec![
                    text(f.idea.clone()),
                    text(f.statement.clone()),
                    text(f.verdict.label()),
                    text(f.kind.label()),
                    text(ids_text(&f.evidence_ids)),
                ]
            })
            .collect(),
    }
}

/// A count computed from the state itself, shown when the model has no
/// series to chart: how much evidence each kind of source gave.
fn evidence_by_source(state: &ResearchState) -> ChartSpec {
    let mut counts = [0_u32; 4];
    for evidence in &state.evidence {
        let i = match evidence.source {
            SourceRef::File { .. } => 0,
            SourceRef::Commit { .. } => 1,
            SourceRef::Url { .. } => 2,
            SourceRef::Command { .. } => 3,
        };
        counts[i] += 1;
    }
    let (labels, values): (Vec<String>, Vec<f64>) = ["Files", "Commits", "Web", "Commands"]
        .into_iter()
        .zip(counts)
        .filter(|(_, n)| *n > 0)
        .map(|(label, n)| (label.to_owned(), f64::from(n)))
        .unzip();
    ChartSpec {
        title: "Evidence by source kind".into(),
        kind: ChartKind::Bar,
        labels,
        series: vec![Series {
            name: "evidence".into(),
            values,
        }],
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use domain::model::{ModelPolicy, ModelRole};
    use serde_json::json;

    use super::*;
    use crate::flows::research::node::NodeDeps;
    use crate::ports::events::DomainEvent;
    use crate::testing::llm::{ScriptedLlm, ScriptedResponse};
    use crate::testing::system::RecordingSink;

    pub(crate) fn blocks(sink: &RecordingSink) -> Vec<BlockKind> {
        sink.events()
            .into_iter()
            .filter_map(|e| match e {
                DomainEvent::BlockAdded { block } => Some(block.kind),
                _ => None,
            })
            .collect()
    }

    fn state_with_evidence() -> ResearchState {
        let mut state = ResearchState::default();
        state.log(1, Stage::Data, "two pieces of evidence");
        state.add_evidence(
            "login runs 3 queries per user",
            SourceRef::File {
                path: "src/login.rs".into(),
                line_start: Some(10),
                line_end: Some(20),
            },
            1,
        );
        state.add_evidence(
            "login p95: 2000 ms",
            SourceRef::Command {
                command: "make bench".into(),
                output_excerpt: "p95=2000ms".into(),
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
        .with_clock(|| 5);
        NodeContext::new(Stage::Analysis, deps, "login takes 2 s", state)
    }

    fn report(evidence: &str) -> String {
        json!({
            "findings": [{
                "idea": "login is slow because of the database",
                "statement": "3 queries per user add up to a 2000 ms p95",
                "verdict": "supports",
                "kind": "benchmark",
                "evidence_ids": [evidence, "E2"]
            }],
            "charts": [{
                "title": "login p95",
                "kind": "bar",
                "labels": ["current", "target"],
                "series": [{ "name": "ms", "values": [2000.0, 200.0] }],
                "evidence_ids": ["E2"]
            }]
        })
        .to_string()
    }

    #[tokio::test]
    async fn findings_become_a_table_and_series_charts() {
        let llm = Arc::new(ScriptedLlm::new([ScriptedResponse::text(format!(
            "```json\n{}\n```",
            report("E1")
        ))]));
        let sink = Arc::new(RecordingSink::new());
        let mut ctx = context(&llm, &sink, state_with_evidence());
        assert_eq!(
            Analysis::new().run(&mut ctx).await.unwrap(),
            NodeOutcome::Done
        );

        let req = llm.last_request().unwrap();
        assert_eq!(req.model.role, ModelRole::Main);
        let sent = serde_json::to_string(&req.messages).unwrap();
        assert!(sent.contains("E1 (file src/login.rs:10-20)"), "{sent}");

        let blocks = blocks(&sink);
        let BlockKind::Table(table) = &blocks[0] else {
            panic!("{blocks:?}")
        };
        assert_eq!(table.rows[0][2], Cell::Text("Supports".into()));
        assert_eq!(table.rows[0][4], Cell::Text("E1, E2".into()));
        let BlockKind::Chart(chart) = &blocks[1] else {
            panic!("{blocks:?}")
        };
        assert_eq!(chart.series[0].values, [2000.0, 200.0]);
        let recorded: Vec<ChartSpec> = ctx.data(super::super::spec::CHARTS_KEY).unwrap();
        assert_eq!(&recorded[0], chart);

        let findings: Vec<Finding> = ctx.data(KEY_FINDINGS).unwrap();
        assert_eq!(findings[0].kind, FindingKind::Benchmark);
        let entry = ctx.state().journal.last().unwrap();
        assert_eq!(entry.stage, Stage::Analysis);
        assert!(entry.text.contains("1 findings"), "{}", entry.text);
    }

    #[tokio::test]
    async fn unknown_evidence_is_retried_once_then_fails() {
        let llm = Arc::new(ScriptedLlm::new([
            ScriptedResponse::text(report("E9")),
            ScriptedResponse::text(report("E1")),
        ]));
        let sink = Arc::new(RecordingSink::new());
        let mut ctx = context(&llm, &sink, state_with_evidence());
        assert_eq!(
            Analysis::new().run(&mut ctx).await.unwrap(),
            NodeOutcome::Done
        );
        let retry = serde_json::to_string(&llm.last_request().unwrap().messages).unwrap();
        assert!(
            retry.contains("evidence that does not exist: E9"),
            "{retry}"
        );

        let llm = Arc::new(ScriptedLlm::new([
            ScriptedResponse::text("no idea"),
            ScriptedResponse::text(report("E9")),
        ]));
        let mut ctx = context(&llm, &sink, state_with_evidence());
        let err = Analysis::new().run(&mut ctx).await.unwrap_err();
        assert!(err.to_string().contains("E9"), "{err}");
    }

    #[tokio::test]
    async fn without_charts_it_counts_the_evidence_by_source() {
        let answer = json!({
            "findings": [{
                "statement": "login queries three times",
                "verdict": "nuances",
                "evidence_ids": ["E1"]
            }]
        });
        let llm = Arc::new(ScriptedLlm::new([ScriptedResponse::text(
            answer.to_string(),
        )]));
        let sink = Arc::new(RecordingSink::new());
        let mut ctx = context(&llm, &sink, state_with_evidence());
        Analysis::new().run(&mut ctx).await.unwrap();
        let BlockKind::Chart(chart) = &blocks(&sink)[1] else {
            panic!()
        };
        assert_eq!(chart.labels, ["Files", "Commands"]);
        assert_eq!(chart.series[0].values, [1.0, 1.0]);
    }

    #[test]
    fn spanish_verdicts_and_kinds_still_parse() {
        let finding: Finding = serde_json::from_value(json!({
            "statement": "s",
            "verdict": "contradice",
            "kind": "linea_de_tiempo",
            "evidence_ids": ["E1"]
        }))
        .unwrap();
        assert_eq!(finding.verdict, Verdict::Contradicts);
        assert_eq!(finding.kind, FindingKind::Timeline);
    }

    #[tokio::test]
    async fn without_evidence_it_goes_back_to_the_data() {
        let llm = Arc::new(ScriptedLlm::new([]));
        let sink = Arc::new(RecordingSink::new());
        let mut ctx = context(&llm, &sink, ResearchState::default());
        let outcome = Analysis::new().run(&mut ctx).await.unwrap();
        assert!(matches!(
            outcome,
            NodeOutcome::Back {
                to: Stage::Data,
                ..
            }
        ));

        // A graph without Data goes back to Observation.
        let stages: Arc<[Stage]> = Stage::ALL
            .into_iter()
            .filter(|s| *s != Stage::Data)
            .collect();
        let mut ctx = context(&llm, &sink, ResearchState::default()).with_stages(stages);
        let outcome = Analysis::new().run(&mut ctx).await.unwrap();
        assert!(matches!(
            outcome,
            NodeOutcome::Back {
                to: Stage::Observation,
                ..
            }
        ));
        assert!(blocks(&sink).is_empty());
        assert_eq!(llm.requests().len(), 0);
    }
}
