//! Opening the solution gate of a deep discovery: the risks record and a
//! spike with frozen criteria, run by the acting agent in a throwaway
//! worktree and scored against them.

use domain::conversation::Session;
use domain::discovery::{DiscoveryEvent, RecordRef, Section, Spike, SpikeId, SpikeScore};
use domain::model::ModelRole;
use serde::{Deserialize, Serialize};

use super::engine::{DiscoveryEngine, Loaded, StepOutcome};
use super::error::DiscoveryError;
use super::explore::check_record_text;
use super::records;
use crate::flows::research::json::{final_text, parse_valid, retry_message};
use crate::ports::discovery::DiscoveryId;
use crate::ports::events::NoticeLevel;
use crate::ports::shell::RunCommandRequest;
use crate::usecases::run_turn::TurnOptions;

/// The artifact name of spike `id`.
#[must_use]
pub fn spike_artifact(id: SpikeId) -> String {
    format!("spikes/{id}.json")
}

/// A spike as the proposer answers it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpikeProposal {
    /// What to test.
    pub hypothesis: String,
    /// The criteria.
    pub criteria: Vec<String>,
    /// How many must hold.
    pub k: u32,
    /// What the agent should do, in a few sentences.
    pub instructions: String,
}

/// The proposer's instructions.
pub const PROPOSER_SYSTEM: &str = "\
You design one spike for a discovery: a short experiment, at most an hour of work for a coding \
agent in a throwaway git worktree of this repository, that tests the riskiest assumption of the \
solution the Breakdown points to. Answer with only a JSON object {\"hypothesis\": \"...\", \
\"criteria\": [\"...\"], \"k\": <int>, \"instructions\": \"...\"}: 2 to 5 criteria that can be \
observed (a test passes, a number is below a threshold, a file is produced), `k` of them must \
hold, and the instructions tell the agent exactly what to build and measure. The criteria are \
frozen before the run and cannot be weakened afterwards.";

/// What the runner reports after the spike.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpikeOutcome {
    /// What was observed.
    pub facts: String,
    /// Whether each frozen criterion held, in order.
    pub met: Vec<bool>,
}

/// Opens the gate of `Solution proposal`: the risks record, then a spike.
pub(crate) async fn open_solution_gate(
    engine: &DiscoveryEngine,
    id: &DiscoveryId,
    loaded: &Loaded,
    section: Section,
) -> Result<StepOutcome, DiscoveryError> {
    let mut produced = Vec::new();
    let scope = loaded.state.approved_at(Section::Scope).unwrap_or(0);
    if !loaded.state.confirmed_after(RecordRef::Risks, scope) {
        produced.push(write_risks(engine, id).await?);
    }
    let breakdown = loaded.state.approved_at(Section::Breakdown).unwrap_or(0);
    if loaded.state.spikes_satisfactory_after(breakdown) == 0 {
        match run_spike(engine, id, loaded).await? {
            Some(line) => produced.push(line),
            None => {
                let reloaded = engine.load(id).await?;
                let mut missing = reloaded.state.gate_missing(section);
                missing.push("no acting agent or shell is configured to run a spike".to_owned());
                return Ok(StepOutcome::GateClosed {
                    section,
                    missing,
                    produced,
                });
            }
        }
    }
    let reloaded = engine.load(id).await?;
    engine.render(id).await?;
    let still = reloaded.state.gate_missing(section);
    if still.is_empty() {
        Ok(StepOutcome::GateOpened { section, produced })
    } else {
        Ok(StepOutcome::GateClosed {
            section,
            missing: still,
            produced,
        })
    }
}

/// Writes the risks record from the CLD and the findings and has it
/// checked.
async fn write_risks(engine: &DiscoveryEngine, id: &DiscoveryId) -> Result<String, DiscoveryError> {
    let cld = engine
        .deps()
        .log
        .artifact(id, records::names::CLD)
        .await?
        .unwrap_or_default();
    let variables = engine
        .deps()
        .log
        .artifact(id, records::names::VARIABLES)
        .await?
        .unwrap_or_default();
    let findings = engine
        .section_text(id, Section::Findings)
        .await?
        .unwrap_or_default();
    let prompt = format!(
        "# Causal loop diagram\n\n{cld}\n\n# Variables\n\n{variables}\n\n# Findings\n\n{findings}\n\n\
         Write the `risks` record: one `### <Risk>` subsection per loop or pathway of the diagram \
         (and per open question that matters), each saying which loop or pathway it comes from, what \
         would make it worse, and what a solution must therefore avoid. Markdown only, no preface."
    );
    let text = engine
        .model()
        .ask(
            ModelRole::Plan,
            "You write one record of a deep discovery from the material given; you invent nothing.",
            &prompt,
            "writing risks",
        )
        .await?;
    engine
        .deps()
        .log
        .put_artifact(id, records::names::RISKS, text.trim())
        .await?;
    let verdict = check_record_text(engine, RecordRef::Risks, &text).await?;
    engine
        .append(
            id,
            DiscoveryEvent::RecordChecked {
                record: RecordRef::Risks,
                verdict,
                by: engine.actor_for(ModelRole::Explore),
            },
        )
        .await?;
    Ok(format!("risks: {verdict:?}"))
}

/// Proposes, starts, runs and scores one spike. `None` when there is no
/// agent to run it or no shell to give it a worktree: a spike never runs
/// in the workspace itself.
async fn run_spike(
    engine: &DiscoveryEngine,
    id: &DiscoveryId,
    loaded: &Loaded,
) -> Result<Option<String>, DiscoveryError> {
    let Some(worker) = engine.deps().worker.clone() else {
        return Ok(None);
    };
    if engine.deps().shell.is_none() {
        return Ok(None);
    }
    let mut material = format!("# Problem\n\n{}\n", loaded.state.problem.trim());
    for s in [
        Section::Findings,
        Section::DesiredState,
        Section::Gap,
        Section::Breakdown,
    ] {
        if let Some(text) = engine.section_text(id, s).await? {
            material.push_str(&format!("\n## {s}\n\n{}\n", text.trim()));
        }
    }
    let proposal = engine
        .model()
        .ask_json::<SpikeProposal>(
            ModelRole::Plan,
            PROPOSER_SYSTEM,
            &material,
            "proposing a spike",
            |p| {
                if p.criteria.len() < 2 || p.criteria.len() > 5 {
                    return Err("2 to 5 criteria".into());
                }
                if p.k == 0 || p.k as usize > p.criteria.len() {
                    return Err("k must be between 1 and the number of criteria".into());
                }
                if p.hypothesis.trim().is_empty() || p.instructions.trim().is_empty() {
                    return Err("hypothesis and instructions cannot be empty".into());
                }
                Ok(())
            },
        )
        .await?;
    let number = u32::try_from(loaded.state.spikes.len()).unwrap_or(0) + 1;
    let spike_id = SpikeId(number);
    let spike = Spike::frozen(
        spike_id,
        proposal.hypothesis.clone(),
        proposal.criteria.clone(),
        proposal.k,
    )
    .map_err(|e| DiscoveryError::Model(format!("spike proposal: {e}")))?;
    engine
        .put_json(id, &spike_artifact(spike_id), &spike)
        .await?;
    let worktree = format!(".crest/spikes/{id}-{spike_id}");
    let branch = format!("spike/{id}-{spike_id}");
    engine
        .append(
            id,
            DiscoveryEvent::SpikeStarted {
                spike: spike_id,
                worktree: worktree.clone(),
                branch: branch.clone(),
            },
        )
        .await?;
    if !add_worktree(engine, &worktree, &branch).await {
        // Started is in the log; score it so the trail stays consistent
        // and the gate says why it is still closed.
        engine.notice(
            NoticeLevel::Warning,
            format!(
                "spike {spike_id}: `git worktree add {worktree}` failed; the spike did not run"
            ),
        );
        return finish_spike(
            engine,
            id,
            spike,
            "the git worktree could not be created; the spike did not run".to_owned(),
            None,
        )
        .await
        .map(Some);
    }
    let where_ = format!(
        "Work only inside the git worktree `{worktree}` (branch `{branch}`); it is thrown away afterwards."
    );
    let instructions = format!(
        "You run spike {spike_id} of a discovery.\n\n## Hypothesis\n\n{}\n\n## Frozen criteria ({} of {} must hold)\n\n{}\n\n## Instructions\n\n{}\n\n{where_}\n\
         When done, answer with only a JSON object {{\"facts\": \"<what you observed, with numbers>\", \"met\": [true|false per criterion, in order]}}.",
        spike.hypothesis,
        spike.k,
        spike.criteria.len(),
        spike
            .criteria
            .iter()
            .enumerate()
            .map(|(i, c)| format!("{}. {c}", i + 1))
            .collect::<Vec<_>>()
            .join("\n"),
        proposal.instructions.trim()
    );
    engine.notice(
        NoticeLevel::Info,
        format!("running spike {spike_id}: {}", spike.hypothesis.trim()),
    );
    let mut session = Session::new(engine.workspace_root().to_owned(), engine.now());
    let options = TurnOptions::new(ModelRole::Main);
    let n = spike.criteria.len();
    let validate = move |o: &SpikeOutcome| {
        if o.met.len() != n {
            return Err(format!("`met` must have exactly {n} booleans"));
        }
        if o.facts.trim().is_empty() {
            return Err("`facts` cannot be empty".into());
        }
        Ok(())
    };
    let run = worker
        .run_turn(&mut session, instructions, options.clone())
        .await;
    let outcome = match run {
        Ok(_) => match parse_valid(&final_text(&session), validate) {
            Ok(o) => Some(o),
            Err(error) => {
                let retry = worker
                    .run_turn(&mut session, retry_message(&error), options)
                    .await;
                match retry {
                    Ok(_) => parse_valid(&final_text(&session), validate).ok(),
                    Err(_) => None,
                }
            }
        },
        Err(err) => {
            engine.notice(
                NoticeLevel::Warning,
                format!("spike {spike_id} failed: {err}"),
            );
            None
        }
    };
    remove_worktree(engine, &worktree, &branch).await;
    let (facts, met) = match outcome {
        Some(o) => (o.facts, Some(o.met)),
        None => ("the run did not report an outcome".to_owned(), None),
    };
    finish_spike(engine, id, spike, facts, met).await.map(Some)
}

/// Scores `spike` with `facts` and `met` (every criterion failed when
/// `None`), stores it, appends `SpikeScored` and shows it.
async fn finish_spike(
    engine: &DiscoveryEngine,
    id: &DiscoveryId,
    mut spike: Spike,
    facts: String,
    met: Option<Vec<bool>>,
) -> Result<String, DiscoveryError> {
    let spike_id = spike.id;
    let met = met.unwrap_or_else(|| vec![false; spike.criteria.len()]);
    let score = spike
        .record(facts, met)
        .map_err(|e| DiscoveryError::Model(format!("spike outcome: {e}")))?;
    engine
        .put_json(id, &spike_artifact(spike_id), &spike)
        .await?;
    if let Some(sink) = &engine.deps().sink {
        let _ = sink
            .write(
                id.as_str(),
                &format!("spikes/{spike_id}.md"),
                &spike.to_markdown(),
            )
            .await;
    }
    engine
        .append(
            id,
            DiscoveryEvent::SpikeScored {
                spike: spike_id,
                score,
            },
        )
        .await?;
    engine.emit_markdown(format!("{id} · {spike_id}"), spike.to_markdown());
    Ok(format!(
        "{spike_id}: {score:?} ({} of {} criteria)",
        spike.held(),
        spike.criteria.len()
    ))
}

async fn git(engine: &DiscoveryEngine, args: &[&str]) -> bool {
    let Some(shell) = &engine.deps().shell else {
        return false;
    };
    shell
        .run(RunCommandRequest {
            command: "git".into(),
            args: args.iter().map(|a| (*a).to_owned()).collect(),
            timeout_s: 60,
            raw: false,
        })
        .await
        .is_ok_and(|out| out.status == Some(0))
}

async fn add_worktree(engine: &DiscoveryEngine, path: &str, branch: &str) -> bool {
    git(engine, &["worktree", "add", "-b", branch, path, "HEAD"]).await
}

async fn remove_worktree(engine: &DiscoveryEngine, path: &str, branch: &str) {
    if !git(engine, &["worktree", "remove", "--force", path]).await {
        engine.notice(
            NoticeLevel::Warning,
            format!("could not remove the spike worktree `{path}`; remove it by hand"),
        );
    }
    let _ = git(engine, &["branch", "-D", branch]).await;
}

/// Cleans up the worktrees of spikes that started and were never scored,
/// and scores them not satisfactory (in the log and in their record) so
/// the trail is consistent.
pub(crate) async fn clean_unfinished(
    engine: &DiscoveryEngine,
    id: &DiscoveryId,
    loaded: &Loaded,
) -> Result<Vec<SpikeId>, DiscoveryError> {
    let mut cleaned = Vec::new();
    let unfinished: Vec<(SpikeId, String, String)> = loaded
        .state
        .unfinished_spikes()
        .map(|(id, s)| (id, s.worktree.clone(), s.branch.clone()))
        .collect();
    for (spike_id, worktree, branch) in unfinished {
        remove_worktree(engine, &worktree, &branch).await;
        let stored: Option<Spike> = engine
            .deps()
            .log
            .artifact(id, &spike_artifact(spike_id))
            .await?
            .and_then(|text| serde_json::from_str(&text).ok());
        match stored {
            Some(spike) if !spike.is_scored() => {
                finish_spike(
                    engine,
                    id,
                    spike,
                    "interrupted before it was scored; its worktree was removed on resume"
                        .to_owned(),
                    None,
                )
                .await?;
            }
            stored => {
                // Scored in its record but not in the log (the process died
                // between the two): log the score the record holds.
                let score = stored
                    .and_then(|spike| spike.score())
                    .unwrap_or(SpikeScore::NotSatisfactory);
                engine
                    .append(
                        id,
                        DiscoveryEvent::SpikeScored {
                            spike: spike_id,
                            score,
                        },
                    )
                    .await?;
            }
        }
        cleaned.push(spike_id);
    }
    Ok(cleaned)
}
