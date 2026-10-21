//! What the frame keeps in the graph-flow `Context` besides the
//! [`ResearchState`](domain::research::ResearchState): the flow status,
//! the pending gate and the user's answer to it.
//!
//! Every key the frame owns starts with [`RESERVED_PREFIX`]; nodes store
//! their own data under any other key (see
//! [`NodeContext::set_data`](super::node::NodeContext::set_data)).

use domain::decision::{DecisionRequest, Question};

use crate::decide::Decider;
use domain::research::Stage;
use serde::{Deserialize, Serialize};

/// Prefix of the context keys owned by the frame.
pub const RESERVED_PREFIX: &str = "research.";

/// Key of the [`ResearchState`](domain::research::ResearchState).
pub(crate) const KEY_STATE: &str = "research.state";
/// Key of the original request of the user (a `String`).
pub(crate) const KEY_PROBLEM: &str = "research.problem";
/// Key of the [`FlowStatus`].
pub(crate) const KEY_STATUS: &str = "research.status";
/// Key of the [`PendingGate`], while a node waits for the user.
pub(crate) const KEY_GATE: &str = "research.gate";
/// Key of the [`GateReply`] to the pending gate.
pub(crate) const KEY_REPLY: &str = "research.reply";
/// Key of the last stage whose `StageHeader` was emitted.
pub(crate) const KEY_ENTERED: &str = "research.entered";
/// Key of the `Back` count per requesting stage.
pub(crate) const KEY_BACKS: &str = "research.backs";
/// Key of the usage per stage ([`UsageByStage`](super::usage::UsageByStage)).
pub(crate) const KEY_USAGE: &str = "research.usage";
/// Key of the depth the user asked for at the start (a `String`, e.g.
/// `"quick"`), absent when the flow picks it.
pub const KEY_DEPTH: &str = "research.depth";

/// Prefix of the graph-flow task ids of the nodes.
const TASK_PREFIX: &str = "research/";

/// The graph-flow task id of the node of `stage`, e.g. `research/intake`.
#[must_use]
pub fn task_id(stage: Stage) -> String {
    let name = serde_json::to_value(stage)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| format!("{stage:?}"));
    format!("{TASK_PREFIX}{name}")
}

/// The stage of a graph-flow task id built by [`task_id`].
#[must_use]
pub fn stage_of_task(id: &str) -> Option<Stage> {
    let name = id.strip_prefix(TASK_PREFIX)?;
    serde_json::from_value(serde_json::Value::String(name.to_owned())).ok()
}

/// Where the flow is, as stored in the session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum FlowStatus {
    /// Between nodes: the next `step` runs `next`.
    Running {
        /// Node that runs next.
        next: Stage,
    },
    /// A node stopped at a gate and waits for the user.
    WaitingForInput {
        /// Node that waits.
        stage: Stage,
        /// What it asks.
        prompt: String,
    },
    /// The last node finished.
    Completed,
    /// The user aborted the flow with `/abort`.
    Aborted,
    /// A node failed (after its retries). The session stays at the last
    /// checkpoint; retrying runs `stage` again.
    Failed {
        /// Node that failed.
        stage: Stage,
        /// Why.
        error: String,
    },
}

impl FlowStatus {
    /// Whether `step` would run a node.
    #[must_use]
    pub fn is_running(&self) -> bool {
        matches!(self, Self::Running { .. })
    }

    /// Whether the flow still has work: running, at a gate or failed.
    #[must_use]
    pub fn is_open(&self) -> bool {
        !matches!(self, Self::Completed | Self::Aborted)
    }
}

/// Why a node waits for the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateKind {
    /// A gated node finished and asks to confirm before going on. An
    /// approving answer (see [`is_approval`]) moves on; anything else
    /// runs the node again with the answer as feedback.
    Confirm,
    /// The node asked a question ([`NodeOutcome::NeedsInput`]); it runs
    /// again with the answer.
    ///
    /// [`NodeOutcome::NeedsInput`]: super::node::NodeOutcome::NeedsInput
    Question,
}

/// A gate waiting for the user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingGate {
    /// Node that waits.
    pub stage: Stage,
    /// Kind of gate.
    pub kind: GateKind,
    /// What the user is asked.
    pub prompt: String,
}

/// The user's answer to a [`PendingGate`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateReply {
    /// Node the answer is for.
    pub stage: Stage,
    /// What the user wrote.
    pub text: String,
}

/// Confidence from which the decision model's reading of an answer is
/// taken.
const APPROVAL_CONFIDENCE: f32 = 0.7;

/// Whether `text` approves a confirmation gate: a fixed "yes" of
/// [`is_approval`], or free text the decision model confidently reads as
/// one (rather than instructions).
pub async fn approves(text: &str, decider: &Decider) -> bool {
    if is_approval(text) {
        return true;
    }
    let request = DecisionRequest::new(text).ask(
        "answer",
        Question::choice(
            "The research flow asked the user to confirm a step and this is the answer. What \
             does it do?",
            [
                ("approve", "approves or says to go on"),
                (
                    "instructions",
                    "asks for changes or gives instructions or details",
                ),
            ],
        ),
    );
    decider
        .decide(&request)
        .await
        .and_then(|r| {
            r.choice("answer")
                .map(|(c, p)| c == "approve" && p >= APPROVAL_CONFIDENCE)
        })
        .unwrap_or(false)
}

/// Whether `text` approves a confirmation gate: empty, or a short "yes"
/// such as `yes`, `ok` or `continue` (case and accents ignored, trailing
/// punctuation allowed). Spanish yeses such as `sí`, `continuar` or `dale`
/// are accepted too.
#[must_use]
pub fn is_approval(text: &str) -> bool {
    const YES: &[&str] = &[
        "",
        "si",
        "s",
        "ok",
        "okay",
        "vale",
        "dale",
        "listo",
        "sigue",
        "seguir",
        "continua",
        "continuar",
        "adelante",
        "de acuerdo",
        "aprobado",
        "y",
        "yes",
        "yep",
        "sure",
        "go",
        "go ahead",
        "next",
        "continue",
        "proceed",
        "approve",
        "approved",
        "agreed",
        "done",
        "lgtm",
    ];
    let normalized = normalize(text);
    let trimmed = normalized.trim_end_matches(['.', '!', ',']).trim();
    YES.contains(&trimmed)
}

/// Lowercase, trimmed and without accents, for lenient matching.
pub(crate) fn normalize(text: &str) -> String {
    text.trim()
        .chars()
        .flat_map(char::to_lowercase)
        .map(|c| match c {
            'á' | 'à' | 'ä' => 'a',
            'é' | 'è' | 'ë' => 'e',
            'í' | 'ì' | 'ï' => 'i',
            'ó' | 'ò' | 'ö' => 'o',
            'ú' | 'ù' | 'ü' => 'u',
            other => other,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::decide::DecisionOptions;
    use crate::testing::decision::{ScriptedDecider, choice};

    #[tokio::test]
    async fn free_text_approves_only_when_the_decision_model_is_sure() {
        let answer = |option: &'static str, p: f32| {
            Decider::new(
                Arc::new(ScriptedDecider::answering(move |_, _| choice(option, p))),
                DecisionOptions::default(),
            )
        };
        assert!(approves("sí", &Decider::none()).await);
        assert!(!approves("me parece bien, adelante con eso", &Decider::none()).await);
        assert!(approves("me parece bien, adelante con eso", &answer("approve", 0.9)).await);
        assert!(!approves("me parece bien, adelante con eso", &answer("approve", 0.5)).await);
        assert!(!approves("añade un test", &answer("instructions", 0.9)).await);
    }

    #[test]
    fn task_ids_round_trip_for_every_stage() {
        for stage in Stage::ALL {
            let id = task_id(stage);
            assert!(id.starts_with("research/"), "{id}");
            assert_eq!(stage_of_task(&id), Some(stage));
        }
        assert_eq!(task_id(Stage::CriticalReview), "research/critical_review");
        assert_eq!(stage_of_task("other"), None);
        assert_eq!(stage_of_task("research/nope"), None);
    }

    #[test]
    fn approvals_are_short_yeses() {
        for yes in [
            "",
            "  ",
            "sí",
            "Si.",
            "OK",
            "dale!",
            "Continuar",
            "de acuerdo",
            "yes",
            "Y",
            "Continue.",
            "go ahead",
        ] {
            assert!(is_approval(yes), "{yes:?}");
        }
        for no in [
            "no",
            "yes, but add hypothesis H3",
            "sí, pero agrega la hipótesis H3",
            "the benchmark is missing",
        ] {
            assert!(!is_approval(no), "{no:?}");
        }
    }

    #[test]
    fn status_round_trips_through_json() {
        let status = FlowStatus::WaitingForInput {
            stage: Stage::Hypotheses,
            prompt: "Shall we go on?".into(),
        };
        let json = serde_json::to_value(&status).unwrap();
        assert_eq!(json["status"], "waiting_for_input");
        assert_eq!(serde_json::from_value::<FlowStatus>(json).unwrap(), status);
    }
}
