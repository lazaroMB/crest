//! The side chat of a flow waiting at a gate (`/aside`, `/flow`).
//!
//! [`Aside`] decides where a plain line of the chat goes while a flow may
//! be waiting ([`Aside::route`]):
//!
//! - **Off**: the line answers the gate, unless the decision model is sure
//!   it is a side question ([`classify`]): then the side chat opens.
//! - **On** (`/aside`): lines go to the normal chat with the gate as
//!   context, until one says to go back ([`wants_return`], or `/flow`):
//!   then a chat turn proposes the answer from the side conversation.
//! - **Proposing**: the next line sends the proposed answer (a "yes"), or
//!   is the answer itself.
//!
//! The state lives in memory: after a restart the flow is pending and
//! `/research resume` (or `/implement resume`) brings its gate back.

use application::decide::Decider;
use application::flows::aside::{
    GateInfo, LineIntent, classify, preamble, proposal_context, wants_return,
};
use application::flows::research::gate::is_approval;

/// Where the side chat is.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
enum State {
    /// Lines go to the waiting flow.
    #[default]
    Off,
    /// Lines go to the chat, with `gate` as context.
    On { gate: GateInfo },
    /// `answer` was proposed for `gate`; the next line decides.
    Proposing { gate: GateInfo, answer: String },
}

/// What to do with a plain line of the chat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// No flow waits: a normal chat turn.
    Chat,
    /// Answer the waiting gate with this text.
    Answer(String),
    /// A chat turn with this context before the prompt: the side chat.
    Aside(String),
    /// A chat turn with this context that proposes the answer to the gate;
    /// [`Aside::proposed`] takes its reply.
    Propose(String),
}

/// The side chat of the waiting flow, if open.
#[derive(Debug, Default)]
pub struct Aside {
    state: State,
}

impl Aside {
    /// Whether the side chat is open (or proposing an answer).
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.state != State::Off
    }

    /// Opens the side chat of `gate` (`/aside`).
    pub fn open(&mut self, gate: GateInfo) {
        self.state = State::On { gate };
    }

    /// Closes it: the flow takes the lines again.
    pub fn close(&mut self) {
        self.state = State::Off;
    }

    /// Where `line` goes, with `gate` the gate waiting now (`None` when no
    /// flow waits). With `auto`, the decision model may open the side chat
    /// by itself; the second value is then the notice that says so.
    pub async fn route(
        &mut self,
        line: &str,
        gate: Option<GateInfo>,
        decider: &Decider,
        auto: bool,
    ) -> (Route, Option<String>) {
        let Some(gate) = gate else {
            self.close();
            return (Route::Chat, None);
        };
        match std::mem::take(&mut self.state) {
            State::Proposing { answer, .. } => {
                let answer = if is_approval(line) {
                    answer
                } else {
                    line.to_owned()
                };
                (Route::Answer(answer), None)
            }
            State::On { .. } => {
                let route = if wants_return(line, &gate, decider).await {
                    Route::Propose(proposal_context(&gate))
                } else {
                    Route::Aside(preamble(&gate))
                };
                self.state = State::On { gate };
                (route, None)
            }
            State::Off => {
                if auto && classify(line, &gate, decider).await == LineIntent::SideQuestion {
                    let notice = opened_notice(&gate);
                    let route = Route::Aside(preamble(&gate));
                    self.open(gate);
                    (route, Some(notice))
                } else {
                    (Route::Answer(line.to_owned()), None)
                }
            }
        }
    }

    /// `/flow` without an answer while the side chat is open: the context
    /// of the turn that proposes the answer; `None` when it is not open.
    #[must_use]
    pub fn propose(&self) -> Option<String> {
        match &self.state {
            State::On { gate } | State::Proposing { gate, .. } => Some(proposal_context(gate)),
            State::Off => None,
        }
    }

    /// Takes the reply of the [`Route::Propose`] turn: the next line sends
    /// it. Returns the notice to show; without a reply the side chat stays
    /// open.
    pub fn proposed(&mut self, answer: Option<String>) -> String {
        let gate = match std::mem::take(&mut self.state) {
            State::On { gate } | State::Proposing { gate, .. } => gate,
            State::Off => return "the side chat is closed".to_owned(),
        };
        match answer {
            Some(answer) => {
                let notice = format!(
                    "proposed answer for {} (above): Enter or `ok` sends it to the flow; \
                     anything else is sent instead",
                    gate.stage
                );
                self.state = State::Proposing { gate, answer };
                notice
            }
            None => {
                self.state = State::On { gate };
                "no answer came out of the side chat; `/flow <answer>` sends one".to_owned()
            }
        }
    }
}

/// The notice shown when the side chat opens.
#[must_use]
pub fn opened_notice(gate: &GateInfo) -> String {
    format!(
        "side chat: {gate}, your lines go to the chat now; `/flow` (or \"listo, continúa\") \
         goes back with an answer, `/flow <answer>` answers directly"
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use application::decide::DecisionOptions;
    use application::flows::aside::FlowKind;
    use application::testing::decision::{ScriptedDecider, choice};

    use super::*;

    fn gate() -> GateInfo {
        GateInfo {
            flow: FlowKind::Research,
            stage: "Definition and constraints".into(),
            prompt: "Which latency is acceptable?".into(),
            topic: None,
        }
    }

    fn side() -> Decider {
        Decider::new(
            Arc::new(ScriptedDecider::answering(|_, _| choice("side", 0.95))),
            DecisionOptions::default(),
        )
    }

    #[tokio::test]
    async fn without_a_flow_lines_are_chat() {
        let mut aside = Aside::default();
        aside.open(gate());
        let (route, _) = aside.route("hola", None, &side(), true).await;
        assert_eq!(route, Route::Chat);
        assert!(!aside.is_open());
    }

    #[tokio::test]
    async fn without_the_decision_model_lines_answer_the_gate() {
        let mut aside = Aside::default();
        let (route, notice) = aside
            .route(
                "¿qué latencia hay hoy?",
                Some(gate()),
                &Decider::none(),
                true,
            )
            .await;
        assert_eq!(route, Route::Answer("¿qué latencia hay hoy?".into()));
        assert_eq!(notice, None);
    }

    #[tokio::test]
    async fn a_side_question_opens_the_side_chat_until_the_user_goes_back() {
        let mut aside = Aside::default();
        let (route, notice) = aside
            .route("¿qué latencia hay hoy?", Some(gate()), &side(), true)
            .await;
        assert!(matches!(&route, Route::Aside(c) if c.contains("Which latency")));
        assert!(notice.is_some_and(|n| n.contains("/flow")));
        assert!(aside.is_open());

        // Headless never opens it by itself.
        let mut manual = Aside::default();
        let (route, _) = manual
            .route("¿qué latencia hay hoy?", Some(gate()), &side(), false)
            .await;
        assert!(matches!(route, Route::Answer(_)));

        let (route, _) = aside
            .route("¿y el p99?", Some(gate()), &Decider::none(), true)
            .await;
        assert!(matches!(route, Route::Aside(_)));
        let (route, _) = aside
            .route("listo, continúa", Some(gate()), &Decider::none(), true)
            .await;
        assert!(matches!(&route, Route::Propose(c) if c.contains("Which latency")));

        let notice = aside.proposed(Some("200 ms at p95".into()));
        assert!(notice.contains("Enter"), "{notice}");
        let (route, _) = aside.route("", Some(gate()), &side(), true).await;
        assert_eq!(route, Route::Answer("200 ms at p95".into()));
        assert!(!aside.is_open());
    }

    #[tokio::test]
    async fn another_line_replaces_the_proposed_answer() {
        let mut aside = Aside::default();
        aside.open(gate());
        assert!(aside.propose().is_some());
        aside.proposed(Some("200 ms".into()));
        let (route, _) = aside
            .route("mejor 150 ms", Some(gate()), &side(), true)
            .await;
        assert_eq!(route, Route::Answer("mejor 150 ms".into()));
    }

    #[tokio::test]
    async fn an_empty_proposal_keeps_the_side_chat_open() {
        let mut aside = Aside::default();
        aside.open(gate());
        let notice = aside.proposed(None);
        assert!(notice.contains("/flow <answer>"), "{notice}");
        assert!(aside.is_open());
        let (route, _) = aside.route("¿y el p99?", Some(gate()), &side(), true).await;
        assert!(matches!(route, Route::Aside(_)));
    }
}
