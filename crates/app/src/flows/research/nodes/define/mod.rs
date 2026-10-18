//! Node 7, Definition and constraints: the problem in measurable
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
You are the \"Definition and constraints\" node of an investigation into a software project. \
Define the problem in one measurable sentence (if there are several independent problems, one \
sentence per problem; each will be a Spec) and the table of constraints the solution must \
respect.
- A measurable definition has a magnitude with a unit, a comparison with a number or a \
verifiable condition. Good: \"at 500 requests per second the response time rises from 10 ms \
to 500 ms\". Bad: \"the response time depends on the load\".
- Rely only on the hypotheses and evidence given; cite the evidence ids (E1…) that back each \
definition.
- Each constraint has a kind (platform, performance, compatibility, security or other) and, if \
known, the test that verifies it (kind test or command, and ref with the command).
Reply only with JSON:
{\"problems\": [{\"statement\": \"…\", \"evidence_ids\": [\"E1\"]}],
 \"constraints\": [{\"kind\": \"performance\", \"statement\": \"…\", \
\"test\": {\"kind\": \"command\", \"ref\": \"…\"}}]}
\"test\" may be null.";

const JUDGE_SYSTEM: &str = "\
Judge whether each problem definition is measurable: it must be checkable with a concrete \
measurement or test (a magnitude with a unit, a numeric threshold or a verifiable condition) \
and be a single sentence. Be strict: \"the response time depends on the load\" is not \
measurable.
Reply only with JSON: {\"verdicts\": [{\"index\": 0, \"measurable\": true, \"reason\": \"…\"}]} \
with one verdict per definition; in reason say what is missing when it is not measurable.";

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

/// The Definition and constraints node.
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
            let to = ctx.first_stage_of(&[Stage::Data, Stage::Observation]);
            return Ok(back(ctx, to, "there is no evidence to define the problem"));
        }
        if ctx.state().hypotheses.is_empty() {
            return Ok(back(
                ctx,
                Stage::Hypotheses,
                "there are no hypotheses to base the definition on",
            ));
        }
        let base = context_prompt(ctx);
        let mut draft = ask_draft(ctx, &base).await?;
        let mut rejected = rejections(ctx, &draft).await?;
        if !rejected.is_empty() {
            ctx.log(format!(
                "Definitions are not measurable, asking again:\n{}",
                list(&rejected)
            ));
            let retry = format!(
                "{base}\n\nThese definitions are not measurable; rewrite them:\n{}",
                list(&rejected)
            );
            draft = ask_draft(ctx, &retry).await?;
            rejected = rejections(ctx, &draft).await?;
        }
        if !rejected.is_empty() {
            ctx.log(format!(
                "Still no measurable definition; asking the user:\n{}",
                list(&rejected)
            ));
            return Ok(NodeOutcome::NeedsInput {
                prompt: format!(
                    "I could not reach a measurable definition of the problem. Missing:\n{}\nSay \
                     what is observed and how much (number and unit, e.g. \"at 500 requests per \
                     second the response time rises from 10 ms to 500 ms\") or a condition that \
                     can be checked (e.g. \"running X makes test Y fail\").",
                    list(&rejected)
                ),
            });
        }
        save(ctx, draft)?;
        Ok(NodeOutcome::Done)
    }
}

fn back(ctx: &mut NodeContext, to: Stage, reason: &str) -> NodeOutcome {
    ctx.log(format!("Goes back to {}: {reason}.", to.name()));
    NodeOutcome::Back {
        to,
        reason: reason.into(),
    }
}

/// The problem, hypotheses, evidence and previous definition, for the
/// draft prompt.
fn context_prompt(ctx: &NodeContext) -> String {
    let state = ctx.state();
    let mut out = format!("User request: {}\n\nHypotheses:\n", ctx.problem());
    for h in &state.hypotheses {
        let _ = writeln!(
            out,
            "- {} (priority {}, {:?}): {}",
            h.id, h.priority, h.status, h.statement
        );
    }
    out.push_str("\nEvidence:\n");
    for e in &state.evidence {
        let _ = writeln!(out, "- {}: {}", e.id, e.claim);
    }
    if !state.problem_statements.is_empty() {
        out.push_str("\nPrevious definition:\n");
        for p in &state.problem_statements {
            let _ = writeln!(out, "- {p}");
        }
    }
    if !state.constraints.is_empty() {
        out.push_str("\nPrevious constraints (keep or correct them):\n");
        for c in &state.constraints {
            let _ = writeln!(out, "- {} ({:?}): {}", c.id, c.kind, c.statement);
        }
    }
    if let Some(reply) = ctx.user_reply() {
        let _ = write!(out, "\nUser instructions: {reply}\n");
    }
    out
}

async fn ask_draft(ctx: &NodeContext, prompt: &str) -> Result<Draft, NodeError> {
    let state = ctx.state();
    ctx.ask_json(DRAFT_SYSTEM, prompt, |d: &Draft| {
        if d.problems.is_empty() {
            return Err("`problems` is empty: at least one definition is needed".into());
        }
        if d.problems.iter().any(|p| p.statement.trim().is_empty()) {
            return Err("`problems` has an empty definition".into());
        }
        if d.constraints.iter().any(|c| c.statement.trim().is_empty()) {
            return Err("`constraints` has an empty constraint".into());
        }
        let cited: Vec<EvidenceId> = d
            .problems
            .iter()
            .flat_map(|p| p.evidence_ids.iter().copied())
            .collect();
        state.cites_only_known_evidence(&cited).map_err(|unknown| {
            let ids: Vec<String> = unknown.iter().map(ToString::to_string).collect();
            format!("cites evidence that does not exist: {}", ids.join(", "))
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
    let mut prompt = String::from("Definitions:\n");
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
                    "a verdict is missing for some index from 0 to {}",
                    count - 1
                ))
            }
        })
        .await?;
    for (i, statement) in candidates.into_iter().enumerate() {
        let verdict = verdicts.verdicts.iter().find(|v| v.index == i);
        if let Some(v) = verdict.filter(|v| !v.measurable) {
            let reason = if v.reason.trim().is_empty() {
                "the model judged it not measurable".to_owned()
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
        .map(|(s, r)| format!("- \"{s}\": {r}"))
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
    let mut markdown = String::from("## Problem\n\n");
    for (i, p) in draft.problems.iter().enumerate() {
        let _ = write!(markdown, "{}. {}", i + 1, p.statement);
        if !p.evidence_ids.is_empty() {
            let ids: Vec<String> = p.evidence_ids.iter().map(ToString::to_string).collect();
            let _ = write!(markdown, " [{}]", ids.join(", "));
        }
        markdown.push('\n');
    }
    ctx.emit(BlockKind::Markdown {
        title: Some("Problem definition".into()),
        source: markdown,
    })?;
    ctx.emit(BlockKind::Table(constraint_table(ctx)))?;
    ctx.set_data(PROBLEMS_KEY, &draft.problems)?;
    let summary = format!(
        "{} measurable definition(s) and {} constraint(s):\n{}",
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
                    TestKind::Command => "command",
                };
                Cell::Text(format!("{kind}: {}", t.reference))
            });
            vec![
                Cell::Text(c.id.to_string()),
                Cell::Text(kind_name(c.kind).into()),
                Cell::Text(c.statement.clone()),
                test,
            ]
        })
        .collect();
    TableSpec {
        columns: ["Id", "Kind", "Constraint", "Test"]
            .into_iter()
            .map(Column::new)
            .collect(),
        rows,
    }
}

fn kind_name(kind: ConstraintKind) -> &'static str {
    match kind {
        ConstraintKind::Platform => "platform",
        ConstraintKind::Performance => "performance",
        ConstraintKind::Compatibility => "compatibility",
        ConstraintKind::Security => "security",
        ConstraintKind::Other => "other",
    }
}

#[cfg(test)]
mod tests;
