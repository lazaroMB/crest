//! Node 10, Modeling: filters the brainstorm options against the
//! constraints, scores the survivors in a decision matrix and draws the
//! black box of the chosen one.
//!
//! # Inputs
//!
//! - The options of the brainstorm, read with
//!   [`NodeContext::data`] under [`BRAINSTORM_OPTIONS_KEY`] as a
//!   `Vec<`[`IdeaOption`]`>`. None or empty → `Back { to: Brainstorm }`.
//! - The constraints of [`ResearchState::constraints`](domain::research::ResearchState).
//!   None → `Back { to: Definition }`. Constraints are checked first:
//!   Definition comes before Brainstorm, so going back there replays both.
//! - The evidence, so the rationale can cite it, and the gate feedback
//!   ([`NodeContext::user_reply`]) on reruns.
//!
//! # Outputs
//!
//! - One [`Decision`](domain::research::Decision) `{ chosen, discarded,
//!   rationale, evidence_ids }`. A rerun (gate feedback or a return from a
//!   later stage) replaces the decision of the previous run, whose id is
//!   kept under [`DECISION_KEY`].
//! - The Mermaid source of the black box under [`BLACK_BOX_KEY`], for the
//!   "Proposed solution" of the Spec.
//! - Blocks: a `Table` (options × criteria with scores, total and state),
//!   a `Mermaid` black box and a `Markdown` with the rationale.
//!
//! When every option violates a constraint the node goes back to the
//! brainstorm instead of choosing one.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use async_trait::async_trait;
use domain::block::{BlockKind, Cell, Column, MermaidSpec, TableSpec};
use domain::research::{
    Constraint, ConstraintKind, DecisionId, EvidenceId, Stage, extract_citations,
};
use serde::{Deserialize, Serialize};

use super::brainstorm::IDEAS_KEY;
use crate::flows::research::json::parse_json;
use crate::flows::research::node::{NodeContext, NodeError, NodeOutcome, ResearchNode};

/// Flow-data key where the brainstorm (node 9) leaves its options
/// ([`IDEAS_KEY`]), read as a `Vec<IdeaOption>`.
pub const BRAINSTORM_OPTIONS_KEY: &str = IDEAS_KEY;

/// Flow-data key with the [`DecisionId`] written by the last run.
pub const DECISION_KEY: &str = "modeling.decision";

/// Flow-data key with the Mermaid source of the black box of the chosen
/// option.
pub const BLACK_BOX_KEY: &str = "modeling.black_box";

/// One option of the brainstorm, as stored under
/// [`BRAINSTORM_OPTIONS_KEY`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdeaOption {
    /// Short name, unique among the options (`name` in the brainstorm's
    /// [`Idea`](super::brainstorm::Idea)).
    #[serde(alias = "name")]
    pub title: String,
    /// What the option does, in a few sentences.
    pub description: String,
}

const SYSTEM: &str = "You are the Modeling stage of a software investigation. \
You get solution options, constraints with ids (C1…) and evidence with ids (E1…). \
1) Discard every option that violates at least one constraint, citing its ids. \
2) Pick 2 to 5 decision criteria and score each option that was not discarded from 1 to 5 \
on each criterion. 3) Choose the best viable option and justify it in 2-4 sentences citing \
evidence as [E1]. 4) Describe its black box: the new piece and which parts of the \
system it interacts with. Use the option titles exactly. Answer ONLY with JSON:\n\
{\"criteria\": [\"...\"],\n\
 \"discarded\": [{\"title\": \"...\", \"constraints\": [\"C1\"], \"reason\": \"...\"}],\n\
 \"scores\": [{\"title\": \"...\", \"values\": [1]}],\n\
 \"chosen\": \"title, or null if none is viable\",\n\
 \"rationale\": \"...\",\n\
 \"evidence_ids\": [\"E1\"],\n\
 \"black_box\": {\"piece\": \"...\", \"interactions\": [{\"with\": \"...\", \"how\": \"...\"}]}}";

/// The node of [`Stage::Modeling`].
#[derive(Debug, Default)]
pub struct Modeling;

impl Modeling {
    /// The node.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

#[derive(Debug, Deserialize)]
struct Answer {
    criteria: Vec<String>,
    #[serde(default)]
    discarded: Vec<Discarded>,
    #[serde(default)]
    scores: Vec<Scores>,
    chosen: Option<String>,
    #[serde(default)]
    rationale: String,
    #[serde(default)]
    evidence_ids: Vec<String>,
    black_box: Option<BlackBox>,
}

#[derive(Debug, Deserialize)]
struct Discarded {
    title: String,
    #[serde(default)]
    constraints: Vec<String>,
    #[serde(default)]
    reason: String,
}

#[derive(Debug, Deserialize)]
struct Scores {
    title: String,
    values: Vec<f64>,
}

#[derive(Debug, Deserialize)]
struct BlackBox {
    piece: String,
    #[serde(default)]
    interactions: Vec<Interaction>,
}

#[derive(Debug, Deserialize)]
struct Interaction {
    with: String,
    #[serde(default)]
    how: String,
}

/// An answer that passed validation.
#[derive(Debug)]
struct Model {
    answer: Answer,
    evidence: Vec<EvidenceId>,
}

#[async_trait]
impl ResearchNode for Modeling {
    fn stage(&self) -> Stage {
        Stage::Modeling
    }

    async fn run(&self, ctx: &mut NodeContext) -> Result<NodeOutcome, NodeError> {
        if ctx.state().constraints.is_empty() {
            return Ok(NodeOutcome::Back {
                to: Stage::Definition,
                reason: "there are no constraints to discard options against".into(),
            });
        }
        let options: Vec<IdeaOption> = ctx.data(BRAINSTORM_OPTIONS_KEY).unwrap_or_default();
        if options.is_empty() {
            return Ok(NodeOutcome::Back {
                to: Stage::Brainstorm,
                reason: "the brainstorm left no options to model".into(),
            });
        }
        let prompt = prompt(ctx, &options);
        let model = ask_validated(ctx, &prompt, &options).await?;
        let Some(chosen) = model.answer.chosen.clone() else {
            return Ok(NodeOutcome::Back {
                to: Stage::Brainstorm,
                reason: "every option violates some constraint".into(),
            });
        };
        let black_box = black_box_mermaid(model.answer.black_box.as_ref(), &chosen);
        ctx.emit(BlockKind::Table(matrix(&model.answer, &options, &chosen)))?;
        ctx.emit(BlockKind::Mermaid(MermaidSpec {
            source: black_box.clone(),
            title: Some(format!("Black box: {chosen}")),
        }))?;
        ctx.emit(BlockKind::Markdown {
            title: Some("Decision".into()),
            source: format!("**Chosen:** {chosen}\n\n{}", model.answer.rationale),
        })?;
        let discarded = discarded_labels(&model.answer);
        if let Some(previous) = ctx.data::<DecisionId>(DECISION_KEY) {
            ctx.state_mut().decisions.retain(|d| d.id != previous);
        }
        let id = ctx.state_mut().add_decision(
            chosen.clone(),
            discarded.clone(),
            model.answer.rationale.clone(),
            model.evidence,
        );
        ctx.set_data(DECISION_KEY, id)?;
        ctx.set_data(BLACK_BOX_KEY, &black_box)?;
        ctx.log(format!(
            "{id}: of {} options, {} were discarded by constraints; chose \"{chosen}\" with {} criteria.",
            options.len(),
            discarded.len(),
            model.answer.criteria.len(),
        ));
        Ok(NodeOutcome::Done)
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

fn prompt(ctx: &NodeContext, options: &[IdeaOption]) -> String {
    let state = ctx.state();
    let mut out = format!("Request: {}\n", ctx.problem());
    for statement in &state.problem_statements {
        let _ = writeln!(out, "Problem: {statement}");
    }
    out.push_str("\nConstraints:\n");
    for Constraint {
        id,
        kind,
        statement,
        ..
    } in &state.constraints
    {
        let _ = writeln!(out, "- {id} ({}): {statement}", kind_name(*kind));
    }
    out.push_str("\nOptions:\n");
    for option in options {
        let _ = writeln!(out, "- {}: {}", option.title, option.description);
    }
    if !state.evidence.is_empty() {
        out.push_str("\nEvidence:\n");
        for evidence in &state.evidence {
            let _ = writeln!(out, "- {}: {}", evidence.id, evidence.claim);
        }
    }
    if let Some(reply) = ctx.user_reply() {
        let _ = write!(out, "\nUser corrections: {reply}\n");
    }
    out
}

/// Asks the model and validates its answer, retrying once with the error.
async fn ask_validated(
    ctx: &NodeContext,
    prompt: &str,
    options: &[IdeaOption],
) -> Result<Model, NodeError> {
    ctx.ask_parsed(SYSTEM, prompt, |text| validate(ctx, text, options))
        .await
}

fn validate(ctx: &NodeContext, text: &str, options: &[IdeaOption]) -> Result<Model, String> {
    let answer: Answer = parse_json(text)?;
    let titles: BTreeSet<&str> = options.iter().map(|o| o.title.as_str()).collect();
    if answer.criteria.is_empty() {
        return Err("criteria are missing".into());
    }
    let mut seen = BTreeSet::new();
    let known_constraints: BTreeSet<String> = ctx
        .state()
        .constraints
        .iter()
        .map(|c| c.id.to_string())
        .collect();
    for d in &answer.discarded {
        check_title(&titles, &mut seen, &d.title)?;
        if d.constraints.is_empty() {
            return Err(format!(
                "\"{}\" is discarded without citing constraints",
                d.title
            ));
        }
        if let Some(c) = d
            .constraints
            .iter()
            .find(|c| !known_constraints.contains(*c))
        {
            return Err(format!("constraint {c} does not exist"));
        }
    }
    for s in &answer.scores {
        check_title(&titles, &mut seen, &s.title)?;
        if s.values.len() != answer.criteria.len() {
            return Err(format!(
                "\"{}\" has {} scores and there are {} criteria",
                s.title,
                s.values.len(),
                answer.criteria.len()
            ));
        }
        if s.values.iter().any(|v| !(1.0..=5.0).contains(v)) {
            return Err(format!("the scores of \"{}\" must be from 1 to 5", s.title));
        }
    }
    if let Some(missing) = titles.iter().find(|t| !seen.contains(**t)) {
        return Err(format!("option \"{missing}\" is not classified"));
    }
    if let Some(chosen) = &answer.chosen {
        if !answer.scores.iter().any(|s| &s.title == chosen) {
            return Err(format!(
                "the chosen \"{chosen}\" is not a scored viable option"
            ));
        }
        if answer.rationale.trim().is_empty() {
            return Err("the rationale is missing".into());
        }
        if answer
            .black_box
            .as_ref()
            .is_none_or(|b| b.piece.trim().is_empty())
        {
            return Err("the black box of the chosen option is missing".into());
        }
    }
    let mut evidence = Vec::new();
    for raw in &answer.evidence_ids {
        let id: EvidenceId = raw
            .parse()
            .map_err(|_| format!("invalid evidence id {raw}"))?;
        evidence.push(id);
    }
    for id in extract_citations(&answer.rationale) {
        if !evidence.contains(&id) {
            evidence.push(id);
        }
    }
    if let Err(unknown) = ctx.state().cites_only_known_evidence(&evidence) {
        let unknown: Vec<String> = unknown.iter().map(ToString::to_string).collect();
        return Err(format!(
            "cites evidence that does not exist: {}",
            unknown.join(", ")
        ));
    }
    Ok(Model { answer, evidence })
}

fn check_title<'a>(
    titles: &BTreeSet<&str>,
    seen: &mut BTreeSet<&'a str>,
    title: &'a str,
) -> Result<(), String> {
    if !titles.contains(title) {
        return Err(format!("\"{title}\" is not one of the options"));
    }
    if !seen.insert(title) {
        return Err(format!("\"{title}\" appears twice"));
    }
    Ok(())
}

fn discarded_labels(answer: &Answer) -> Vec<String> {
    answer
        .discarded
        .iter()
        .map(|d| {
            let reason = if d.reason.trim().is_empty() {
                String::new()
            } else {
                format!(": {}", d.reason.trim())
            };
            format!(
                "{} (violates {}){reason}",
                d.title,
                d.constraints.join(", ")
            )
        })
        .collect()
}

/// Options × criteria, with the total and whether each was chosen,
/// viable or discarded; in the order of the brainstorm.
fn matrix(answer: &Answer, options: &[IdeaOption], chosen: &str) -> TableSpec {
    let mut columns = vec![Column::new("Option")];
    columns.extend(answer.criteria.iter().map(Column::new));
    columns.push(Column::new("Total"));
    columns.push(Column::new("Status"));
    let rows = options
        .iter()
        .map(|option| {
            let mut row = vec![Cell::Text(option.title.clone())];
            if let Some(s) = answer.scores.iter().find(|s| s.title == option.title) {
                row.extend(s.values.iter().map(|v| Cell::Number(*v)));
                row.push(Cell::Number(s.values.iter().sum()));
                let state = if option.title == chosen {
                    "chosen"
                } else {
                    "viable"
                };
                row.push(Cell::Text(state.into()));
            } else {
                row.extend(answer.criteria.iter().map(|_| Cell::Null));
                row.push(Cell::Null);
                let violated = answer
                    .discarded
                    .iter()
                    .find(|d| d.title == option.title)
                    .map(|d| d.constraints.join(", "))
                    .unwrap_or_default();
                row.push(Cell::Text(format!("discarded ({violated})")));
            }
            row
        })
        .collect();
    TableSpec { columns, rows }
}

/// Mermaid label text, without characters that break the syntax.
fn label(text: &str) -> String {
    text.replace('"', "'")
        .replace(['[', ']', '{', '}', '|', '\n'], " ")
        .trim()
        .to_owned()
}

/// A flowchart with the chosen piece in the middle and an edge to each
/// part of the system it interacts with.
fn black_box_mermaid(black_box: Option<&BlackBox>, chosen: &str) -> String {
    let piece = black_box.map_or(chosen, |b| b.piece.as_str());
    let mut out = format!("flowchart LR\n  P[\"{}\"]\n", label(piece));
    for (i, interaction) in black_box
        .map(|b| b.interactions.as_slice())
        .unwrap_or_default()
        .iter()
        .enumerate()
    {
        let how = label(&interaction.how);
        let edge = if how.is_empty() {
            "---".to_owned()
        } else {
            format!("---|\"{how}\"|")
        };
        let _ = writeln!(
            out,
            "  P {edge} I{}[\"{}\"]",
            i + 1,
            label(&interaction.with)
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use domain::block::SourceRef;
    use domain::model::ModelPolicy;
    use domain::research::ResearchState;
    use serde_json::json;

    use super::*;

    #[test]
    fn the_options_are_the_ideas_of_the_brainstorm() {
        let idea = super::super::brainstorm::Idea {
            name: "Cache".into(),
            description: "one line".into(),
            evidence_ids: vec![EvidenceId::new(1).unwrap()],
        };
        let options: Vec<IdeaOption> =
            serde_json::from_value(serde_json::to_value(vec![idea]).unwrap()).unwrap();
        assert_eq!(options[0].title, "Cache");
        assert_eq!(options[0].description, "one line");
    }
    use crate::flows::research::node::NodeDeps;
    use crate::ports::events::DomainEvent;
    use crate::testing::llm::{ScriptedLlm, ScriptedResponse};
    use crate::testing::system::RecordingSink;

    fn state() -> ResearchState {
        let mut state = ResearchState::default();
        state
            .problem_statements
            .push("login takes 2 s at 100 rps".into());
        state.add_constraint(ConstraintKind::Platform, "no external services", None);
        state.add_evidence(
            "the session is read from disk on every request",
            SourceRef::File {
                path: "src/session.rs".into(),
                line_start: Some(10),
                line_end: Some(20),
            },
            1,
        );
        state
    }

    fn options() -> Vec<IdeaOption> {
        ["Redis", "In-memory cache", "SQLite index"]
            .into_iter()
            .map(|t| IdeaOption {
                title: t.into(),
                description: format!("use {t}"),
            })
            .collect()
    }

    fn answer() -> String {
        json!({
            "criteria": ["Performance", "Simplicity"],
            "discarded": [{"title": "Redis", "constraints": ["C1"], "reason": "it is an external service"}],
            "scores": [
                {"title": "In-memory cache", "values": [5, 4]},
                {"title": "SQLite index", "values": [3, 3]}
            ],
            "chosen": "In-memory cache",
            "rationale": "Avoids reading disk on every request [E1].",
            "evidence_ids": [],
            "black_box": {"piece": "SessionCache", "interactions": [
                {"with": "SessionStore", "how": "reads on a miss"},
                {"with": "Login", "how": "queries"}
            ]}
        })
        .to_string()
    }

    fn context(
        llm: Arc<ScriptedLlm>,
        sink: Arc<RecordingSink>,
        state: ResearchState,
    ) -> NodeContext {
        let deps = NodeDeps::new(llm, ModelPolicy::default(), sink).with_clock(|| 9);
        NodeContext::new(Stage::Modeling, deps, "login is slow", state)
    }

    fn blocks(sink: &RecordingSink) -> Vec<BlockKind> {
        sink.events()
            .into_iter()
            .filter_map(|e| match e {
                DomainEvent::BlockAdded { block } => Some(block.kind),
                _ => None,
            })
            .collect()
    }

    #[tokio::test]
    async fn discards_violations_scores_and_decides() {
        let llm = Arc::new(ScriptedLlm::new([ScriptedResponse::text(format!(
            "```json\n{}\n```",
            answer()
        ))]));
        let sink = Arc::new(RecordingSink::new());
        let mut ctx = context(Arc::clone(&llm), Arc::clone(&sink), state());
        ctx.set_data(BRAINSTORM_OPTIONS_KEY, options()).unwrap();

        assert_eq!(
            Modeling::new().run(&mut ctx).await.unwrap(),
            NodeOutcome::Done
        );

        let prompt = format!("{:?}", llm.last_request().unwrap().messages);
        assert!(prompt.contains("C1 (platform): no external services"));
        assert!(prompt.contains("SQLite index: use SQLite index"));
        assert_eq!(
            llm.last_request().unwrap().model.role,
            Stage::Modeling.default_role()
        );

        let blocks = blocks(&sink);
        let BlockKind::Table(table) = &blocks[0] else {
            panic!("{blocks:?}")
        };
        let names: Vec<_> = table.columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            names,
            ["Option", "Performance", "Simplicity", "Total", "Status"]
        );
        assert_eq!(table.rows[0][4], Cell::Text("discarded (C1)".into()));
        assert_eq!(table.rows[1][3], Cell::Number(9.0));
        assert_eq!(table.rows[1][4], Cell::Text("chosen".into()));
        let BlockKind::Mermaid(mermaid) = &blocks[1] else {
            panic!("{blocks:?}")
        };
        assert!(
            mermaid
                .source
                .starts_with("flowchart LR\n  P[\"SessionCache\"]")
        );
        assert!(
            mermaid
                .source
                .contains("P ---|\"reads on a miss\"| I1[\"SessionStore\"]")
        );
        assert!(matches!(blocks[2], BlockKind::Markdown { .. }));

        let decision_key: DecisionId = ctx.data(DECISION_KEY).unwrap();
        assert_eq!(ctx.data::<String>(BLACK_BOX_KEY).unwrap(), mermaid.source);
        let state = ctx.into_state();
        let decision = &state.decisions[0];
        assert_eq!(decision.id, decision_key);
        assert_eq!(decision.chosen, "In-memory cache");
        assert_eq!(
            decision.discarded,
            ["Redis (violates C1): it is an external service"]
        );
        assert_eq!(decision.evidence_ids, [EvidenceId::new(1).unwrap()]);
        assert!(state.journal[0].text.contains("chose \"In-memory cache\""));
    }

    #[tokio::test]
    async fn retries_once_and_replaces_its_decision_on_rerun() {
        let llm = Arc::new(ScriptedLlm::new([
            ScriptedResponse::text(r#"{"criteria": ["x"], "scores": [], "chosen": "Redis"}"#),
            ScriptedResponse::text(answer()),
        ]));
        let sink = Arc::new(RecordingSink::new());
        let mut ctx = context(Arc::clone(&llm), sink, state());
        ctx.set_data(BRAINSTORM_OPTIONS_KEY, options()).unwrap();
        let old = ctx
            .state_mut()
            .add_decision("other", vec![], "previous", vec![]);
        ctx.set_data(DECISION_KEY, old).unwrap();

        Modeling::new().run(&mut ctx).await.unwrap();

        let retry = llm.last_request().unwrap();
        assert_eq!(retry.messages.len(), 3);
        assert!(format!("{:?}", retry.messages[2]).contains("is not valid"));
        let state = ctx.into_state();
        assert_eq!(state.decisions.len(), 1);
        assert_eq!(state.decisions[0].chosen, "In-memory cache");
    }

    #[tokio::test]
    async fn fails_after_two_invalid_answers() {
        let llm = Arc::new(ScriptedLlm::new([
            ScriptedResponse::text("no idea"),
            ScriptedResponse::text("neither"),
        ]));
        let mut ctx = context(llm, Arc::new(RecordingSink::new()), state());
        ctx.set_data(BRAINSTORM_OPTIONS_KEY, options()).unwrap();
        let err = Modeling::new().run(&mut ctx).await.unwrap_err();
        assert!(matches!(err, NodeError::Failed(_)), "{err}");
    }

    #[tokio::test]
    async fn goes_back_when_inputs_are_missing_or_nothing_is_viable() {
        let node = Modeling::new();
        let llm = Arc::new(ScriptedLlm::new([]));
        let mut ctx = context(Arc::clone(&llm), Arc::new(RecordingSink::new()), state());
        assert!(matches!(
            node.run(&mut ctx).await.unwrap(),
            NodeOutcome::Back {
                to: Stage::Brainstorm,
                ..
            }
        ));

        let mut ctx = context(
            Arc::clone(&llm),
            Arc::new(RecordingSink::new()),
            ResearchState::default(),
        );
        ctx.set_data(BRAINSTORM_OPTIONS_KEY, options()).unwrap();
        assert!(matches!(
            node.run(&mut ctx).await.unwrap(),
            NodeOutcome::Back {
                to: Stage::Definition,
                ..
            }
        ));
        assert!(llm.requests().is_empty());

        let all_out = json!({
            "criteria": ["x"],
            "discarded": options().iter().map(|o| json!({"title": o.title, "constraints": ["C1"]})).collect::<Vec<_>>(),
            "chosen": null
        });
        let llm = Arc::new(ScriptedLlm::new([ScriptedResponse::text(
            all_out.to_string(),
        )]));
        let mut ctx = context(llm, Arc::new(RecordingSink::new()), state());
        ctx.set_data(BRAINSTORM_OPTIONS_KEY, options()).unwrap();
        assert!(matches!(
            node.run(&mut ctx).await.unwrap(),
            NodeOutcome::Back {
                to: Stage::Brainstorm,
                ..
            }
        ));
        assert!(ctx.into_state().decisions.is_empty());
    }
}
