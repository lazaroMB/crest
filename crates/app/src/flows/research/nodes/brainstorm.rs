//! Node 9, Brainstorm: unfiltered options to solve the problem.
//!
//! The model lists every option it can think of, each with a one-line
//! description, building on the prior solutions found by the research
//! node but without discarding any: filtering against the constraints is
//! the modeling node's job. The options are shown as a table and stored
//! under [`IDEAS_KEY`] for it.

use std::fmt::Write as _;

use async_trait::async_trait;
use domain::block::{BlockKind, Cell, Column, TableSpec};
use domain::research::{EvidenceId, Stage};
use serde::{Deserialize, Serialize};

use super::research::{OPTIONS_KEY, PriorSolution};
use crate::flows::research::node::{NodeContext, NodeError, NodeOutcome, ResearchNode};

/// Key of the options (`Vec<Idea>`), read by the modeling node.
pub const IDEAS_KEY: &str = "brainstorm.ideas";

const SYSTEM: &str = "\
You are the \"Brainstorm\" node of an investigation into a software project. Generate \
options to solve the problem WITHOUT FILTERING: include the obvious ones, the risky ones and \
the ones that seem to violate a constraint; discarding is the job of a later step. Build on \
the prior solutions found, combine them, and add new ideas. Each option has a one-line \
description and, if it rests on evidence, its ids (E1…).
Reply only with JSON:
{\"ideas\": [{\"name\": \"…\", \"description\": \"one line\", \"evidence_ids\": [\"E1\"]}]}";

/// One unfiltered option.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Idea {
    /// Short name.
    pub name: String,
    /// What it is, in one line.
    pub description: String,
    /// Evidence it builds on.
    #[serde(default)]
    pub evidence_ids: Vec<EvidenceId>,
}

#[derive(Debug, Deserialize)]
struct Answer {
    ideas: Vec<Idea>,
}

/// The Brainstorm node.
#[derive(Debug, Default)]
pub struct Brainstorm;

impl Brainstorm {
    /// The node.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ResearchNode for Brainstorm {
    fn stage(&self) -> Stage {
        Stage::Brainstorm
    }

    async fn run(&self, ctx: &mut NodeContext) -> Result<NodeOutcome, NodeError> {
        if ctx.state().problem_statements.is_empty() {
            return Ok(back(
                ctx,
                Stage::Definition,
                "there is no problem definition to generate options from",
            ));
        }
        let Some(prior) = ctx.data::<Vec<PriorSolution>>(OPTIONS_KEY) else {
            return Ok(back(
                ctx,
                Stage::Research,
                "the research of prior solutions is missing",
            ));
        };
        let prompt = prompt(ctx, &prior);
        let state = ctx.state();
        let answer: Answer = ctx
            .ask_json(SYSTEM, &prompt, |a: &Answer| {
                validate(a, |ids| state.cites_only_known_evidence(ids).is_ok())
            })
            .await?;
        let ideas = answer.ideas;
        ctx.emit(BlockKind::Table(table(&ideas)))?;
        ctx.set_data(IDEAS_KEY, &ideas)?;
        let names: Vec<&str> = ideas.iter().map(|i| i.name.as_str()).collect();
        ctx.log(format!(
            "{} unfiltered options: {}.",
            ideas.len(),
            names.join(", ")
        ));
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

fn prompt(ctx: &NodeContext, prior: &[PriorSolution]) -> String {
    let state = ctx.state();
    let mut out = String::from("Problem:\n");
    for p in &state.problem_statements {
        let _ = writeln!(out, "- {p}");
    }
    if !state.constraints.is_empty() {
        out.push_str("\nConstraints (do not discard because of them):\n");
        for c in &state.constraints {
            let _ = writeln!(out, "- {}: {}", c.id, c.statement);
        }
    }
    out.push_str("\nPrior solutions found:\n");
    if prior.is_empty() {
        out.push_str("- none\n");
    }
    for s in prior {
        let ids: Vec<String> = s.evidence_ids.iter().map(ToString::to_string).collect();
        let _ = writeln!(out, "- {} [{}]: {}", s.name, ids.join(", "), s.summary);
    }
    if let Some(reply) = ctx.user_reply() {
        let _ = write!(out, "\nUser instructions: {reply}\n");
    }
    out
}

/// At least one idea; each with a name and a one-line description, no
/// repeated names, and citing only known evidence.
fn validate(answer: &Answer, known: impl Fn(&[EvidenceId]) -> bool) -> Result<(), String> {
    if answer.ideas.is_empty() {
        return Err("`ideas` is empty: generate at least one option".into());
    }
    let mut seen: Vec<String> = Vec::new();
    for idea in &answer.ideas {
        let name = idea.name.trim().to_lowercase();
        if name.is_empty() || idea.description.trim().is_empty() {
            return Err("each idea needs `name` and `description`".into());
        }
        if idea.description.trim().contains('\n') {
            return Err(format!(
                "the description of \"{}\" must be a single line",
                idea.name
            ));
        }
        if seen.contains(&name) {
            return Err(format!("the idea \"{}\" is repeated", idea.name));
        }
        if !known(&idea.evidence_ids) {
            return Err(format!(
                "\"{}\" cites evidence that does not exist",
                idea.name
            ));
        }
        seen.push(name);
    }
    Ok(())
}

fn table(ideas: &[Idea]) -> TableSpec {
    TableSpec {
        columns: ["#", "Option", "Description", "Evidence"]
            .into_iter()
            .map(Column::new)
            .collect(),
        rows: ideas
            .iter()
            .zip(1u32..)
            .map(|(idea, n)| {
                let ids: Vec<String> = idea.evidence_ids.iter().map(ToString::to_string).collect();
                vec![
                    Cell::Number(f64::from(n)),
                    Cell::Text(idea.name.clone()),
                    Cell::Text(idea.description.trim().to_owned()),
                    Cell::Text(ids.join(", ")),
                ]
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use domain::block::SourceRef;
    use domain::conversation::ContentPart;
    use domain::model::{ModelPolicy, ModelRole};
    use domain::research::ResearchState;
    use serde_json::json;

    use super::*;
    use crate::flows::research::node::NodeDeps;
    use crate::flows::research::nodes::research::SolutionKind;
    use crate::ports::events::DomainEvent;
    use crate::testing::llm::{ScriptedLlm, ScriptedResponse};
    use crate::testing::system::RecordingSink;

    fn state() -> ResearchState {
        let mut state = ResearchState::default();
        state.problem_statements.push("login takes 2 s".into());
        state.add_evidence(
            "moka is a concurrent cache",
            SourceRef::Url {
                url: "https://github.com/moka-rs/moka".into(),
                title: None,
            },
            1,
        );
        state
    }

    fn prior() -> Vec<PriorSolution> {
        vec![PriorSolution {
            name: "moka".into(),
            kind: SolutionKind::Library,
            summary: "concurrent cache".into(),
            source: SourceRef::Url {
                url: "https://github.com/moka-rs/moka".into(),
                title: None,
            },
            evidence_ids: vec!["E1".parse().unwrap()],
        }]
    }

    fn ideas(list: serde_json::Value) -> ScriptedResponse {
        ScriptedResponse::text(json!({ "ideas": list }).to_string())
    }

    struct Run {
        outcome: NodeOutcome,
        blocks: Vec<BlockKind>,
        ideas: Option<Vec<Idea>>,
        llm: Arc<ScriptedLlm>,
        state: ResearchState,
    }

    async fn run(
        state: ResearchState,
        prior: Option<Vec<PriorSolution>>,
        script: Vec<ScriptedResponse>,
    ) -> Run {
        let llm = Arc::new(ScriptedLlm::new(script));
        let sink = Arc::new(RecordingSink::new());
        let deps = NodeDeps::new(
            Arc::clone(&llm) as _,
            ModelPolicy::default(),
            Arc::clone(&sink) as _,
        );
        let mut ctx = NodeContext::new(Stage::Brainstorm, deps, "slow login", state);
        if let Some(prior) = prior {
            ctx.set_data(OPTIONS_KEY, prior).unwrap();
        }
        let outcome = Brainstorm::new().run(&mut ctx).await.unwrap();
        let blocks = sink
            .events()
            .into_iter()
            .filter_map(|e| match e {
                DomainEvent::BlockAdded { block } => Some(block.kind),
                _ => None,
            })
            .collect();
        Run {
            outcome,
            blocks,
            ideas: ctx.data(IDEAS_KEY),
            llm,
            state: ctx.into_state(),
        }
    }

    #[tokio::test]
    async fn stores_unfiltered_ideas_for_modeling() {
        let answer = ideas(json!([
            { "name": "Cache with moka", "description": "sessions in moka with a TTL", "evidence_ids": ["E1"] },
            { "name": "Precompute sessions", "description": "a job builds them at startup" }
        ]));
        let run = run(state(), Some(prior()), vec![answer]).await;
        assert_eq!(run.outcome, NodeOutcome::Done);
        let ideas = run.ideas.unwrap();
        assert_eq!(ideas.len(), 2);
        assert_eq!(ideas[0].evidence_ids[0].to_string(), "E1");
        assert!(ideas[1].evidence_ids.is_empty());

        let [BlockKind::Table(table)] = run.blocks.as_slice() else {
            panic!("expected one Table, got {:?}", run.blocks);
        };
        assert_eq!(table.rows.len(), 2);
        assert_eq!(table.rows[1][0], Cell::Number(2.0));
        assert_eq!(table.rows[0][3], Cell::Text("E1".into()));

        let req = run.llm.last_request().unwrap();
        assert_eq!(req.model.role, ModelRole::Plan);
        let ContentPart::Text { text } = &req.messages[0].content[0] else {
            panic!("expected text");
        };
        assert!(text.contains("- moka [E1]: concurrent cache"));
        assert!(run.state.journal[0].text.contains("2 unfiltered options"));
    }

    #[tokio::test]
    async fn asks_again_for_multiline_or_repeated_ideas() {
        let bad = ideas(json!([
            { "name": "A", "description": "one\ntwo" },
        ]));
        let good = ideas(json!([{ "name": "A", "description": "one" }]));
        let run = run(state(), Some(prior()), vec![bad, good]).await;
        assert_eq!(run.ideas.unwrap()[0].description, "one");
        assert_eq!(run.llm.requests().len(), 2);

        let repeated = Answer {
            ideas: vec![
                Idea {
                    name: "A".into(),
                    description: "x".into(),
                    evidence_ids: vec![],
                },
                Idea {
                    name: "a".into(),
                    description: "y".into(),
                    evidence_ids: vec![],
                },
            ],
        };
        assert!(
            validate(&repeated, |_| true)
                .unwrap_err()
                .contains("repeated")
        );
        assert!(validate(&Answer { ideas: vec![] }, |_| true).is_err());
    }

    #[tokio::test]
    async fn goes_back_to_the_missing_stage() {
        let no_definition = run(ResearchState::default(), Some(prior()), vec![]).await;
        assert!(matches!(
            no_definition.outcome,
            NodeOutcome::Back {
                to: Stage::Definition,
                ..
            }
        ));
        let no_research = run(state(), None, vec![]).await;
        assert!(matches!(
            no_research.outcome,
            NodeOutcome::Back {
                to: Stage::Research,
                ..
            }
        ));
        assert!(no_research.llm.requests().is_empty());
    }
}
