//! A scripted [`DecisionPort`] for tests: answers with a closure and
//! records the requests.

use std::collections::BTreeMap;
use std::sync::{Mutex, PoisonError};

use async_trait::async_trait;
use domain::decision::{Answer, DecisionRequest, DecisionResponse, Question};

use crate::ports::decision::{DecisionError, DecisionPort};

type Script =
    Box<dyn Fn(&DecisionRequest) -> Result<DecisionResponse, DecisionError> + Send + Sync>;

/// Answers with a closure and keeps every request.
pub struct ScriptedDecider {
    script: Script,
    requests: Mutex<Vec<DecisionRequest>>,
}

impl std::fmt::Debug for ScriptedDecider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScriptedDecider").finish_non_exhaustive()
    }
}

impl ScriptedDecider {
    /// Answers with `script`.
    pub fn new(
        script: impl Fn(&DecisionRequest) -> Result<DecisionResponse, DecisionError>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self {
            script: Box::new(script),
            requests: Mutex::new(Vec::new()),
        }
    }

    /// Answers every question with `answer(id, question)`.
    pub fn answering(answer: impl Fn(&str, &Question) -> Answer + Send + Sync + 'static) -> Self {
        Self::new(move |request| {
            Ok(response(
                request
                    .questions
                    .iter()
                    .map(|(id, q)| (id.clone(), answer(id, q)))
                    .collect(),
            ))
        })
    }

    /// Yes with probability `p` to every yes/no question, the first option
    /// with confidence `p` to every choice and the top level to every
    /// score.
    pub fn yes(p: f32) -> Self {
        Self::answering(move |_, question| match question {
            Question::YesNo { .. } => Answer::YesNo { p_yes: p },
            Question::Choice { options, .. } => choice(&options[0].0, p),
            Question::Score { levels, .. } => score(levels.len() as f32 - 1.0),
        })
    }

    /// Fails every request.
    pub fn failing() -> Self {
        Self::new(|_| Err(DecisionError::Network("scripted failure".into())))
    }

    /// The requests answered so far.
    pub fn requests(&self) -> Vec<DecisionRequest> {
        self.requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// A choice of `option` with `confidence`.
#[must_use]
pub fn choice(option: &str, confidence: f32) -> Answer {
    Answer::Choice {
        choice: option.to_owned(),
        probabilities: BTreeMap::from([(option.to_owned(), confidence)]),
        confidence,
    }
}

/// A score of `level`.
#[must_use]
pub fn score(level: f32) -> Answer {
    Answer::Score {
        score: level,
        probabilities: BTreeMap::new(),
        confidence: 1.0,
    }
}

fn response(answers: BTreeMap<String, Answer>) -> DecisionResponse {
    DecisionResponse {
        model: "scripted".into(),
        answers,
        input_tokens: 100,
        output_tokens: 0,
    }
}

#[async_trait]
impl DecisionPort for ScriptedDecider {
    fn model(&self) -> &str {
        "scripted"
    }

    async fn decide(&self, request: &DecisionRequest) -> Result<DecisionResponse, DecisionError> {
        self.requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(request.clone());
        (self.script)(request)
    }
}
