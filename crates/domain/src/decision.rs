//! Questions for a decision model, and its answers: probabilities rather
//! than text. A [`DecisionRequest`] gives a state (the text or data to
//! judge) and typed questions under ids; the answers come back under the
//! same ids. Used to judge, classify and route without a full model call.

use std::collections::BTreeMap;

use serde_json::Value;

/// Most questions in one request.
pub const MAX_QUESTIONS: usize = 64;
/// Longest question id.
pub const MAX_ID_LEN: usize = 100;
/// Options of a [`Question::Choice`].
pub const CHOICE_OPTIONS: std::ops::RangeInclusive<usize> = 2..=255;
/// Levels of a [`Question::Score`].
pub const SCORE_LEVELS: std::ops::RangeInclusive<usize> = 2..=10;

/// One typed question.
#[derive(Debug, Clone, PartialEq)]
pub enum Question {
    /// Yes or no; the answer is the probability of yes.
    YesNo {
        /// The question.
        instructions: String,
        /// What yes means, if it needs saying.
        yes: Option<String>,
        /// What no means, if it needs saying.
        no: Option<String>,
    },
    /// One of named options.
    Choice {
        /// The question.
        instructions: String,
        /// Option ids and what each means, in order.
        options: Vec<(String, String)>,
    },
    /// A level on an ordered scale, lowest first.
    Score {
        /// The question.
        instructions: String,
        /// What each level means, lowest first.
        levels: Vec<String>,
    },
}

impl Question {
    /// A yes/no question.
    #[must_use]
    pub fn yes_no(instructions: impl Into<String>) -> Self {
        Self::YesNo {
            instructions: instructions.into(),
            yes: None,
            no: None,
        }
    }

    /// A choice among `options` (id, meaning).
    #[must_use]
    pub fn choice<I, K, V>(instructions: impl Into<String>, options: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        Self::Choice {
            instructions: instructions.into(),
            options: options
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
        }
    }

    /// A score on `levels`, lowest first.
    #[must_use]
    pub fn score<I, L>(instructions: impl Into<String>, levels: I) -> Self
    where
        I: IntoIterator<Item = L>,
        L: Into<String>,
    {
        Self::Score {
            instructions: instructions.into(),
            levels: levels.into_iter().map(Into::into).collect(),
        }
    }
}

/// A state and the questions about it.
#[derive(Debug, Clone, PartialEq)]
pub struct DecisionRequest {
    /// What is judged: text, or structured data.
    pub state: Value,
    /// The questions, by id.
    pub questions: BTreeMap<String, Question>,
}

/// Why a [`DecisionRequest`] cannot be sent.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DecisionRequestError {
    /// No questions, or more than [`MAX_QUESTIONS`].
    #[error("a decision needs 1 to {MAX_QUESTIONS} questions, got {0}")]
    QuestionCount(usize),
    /// An id that is empty, too long or has other characters than letters,
    /// digits, `_`, `.` and `-`.
    #[error("bad question id {0:?}")]
    BadId(String),
    /// A choice or score with the wrong number of options or levels.
    #[error("question {id:?} has {count} options or levels, outside {range}")]
    BadOptions {
        /// The question id.
        id: String,
        /// How many it has.
        count: usize,
        /// The allowed range.
        range: String,
    },
}

impl DecisionRequest {
    /// A request about `state`, without questions yet.
    #[must_use]
    pub fn new(state: impl Into<Value>) -> Self {
        Self {
            state: state.into(),
            questions: BTreeMap::new(),
        }
    }

    /// Adds `question` under `id`.
    #[must_use]
    pub fn ask(mut self, id: impl Into<String>, question: Question) -> Self {
        self.questions.insert(id.into(), question);
        self
    }

    /// Checks the limits every decision model shares.
    ///
    /// # Errors
    ///
    /// The first [`DecisionRequestError`] found.
    pub fn validate(&self) -> Result<(), DecisionRequestError> {
        let count = self.questions.len();
        if count == 0 || count > MAX_QUESTIONS {
            return Err(DecisionRequestError::QuestionCount(count));
        }
        for (id, question) in &self.questions {
            let id_ok = !id.is_empty()
                && id.len() <= MAX_ID_LEN
                && id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'));
            if !id_ok {
                return Err(DecisionRequestError::BadId(id.clone()));
            }
            let (len, range) = match question {
                Question::YesNo { .. } => continue,
                Question::Choice { options, .. } => (options.len(), CHOICE_OPTIONS),
                Question::Score { levels, .. } => (levels.len(), SCORE_LEVELS),
            };
            if !range.contains(&len) {
                return Err(DecisionRequestError::BadOptions {
                    id: id.clone(),
                    count: len,
                    range: format!("{}..={}", range.start(), range.end()),
                });
            }
        }
        Ok(())
    }
}

/// The answer to one question.
#[derive(Debug, Clone, PartialEq)]
pub enum Answer {
    /// Probability of yes, 0 to 1.
    YesNo {
        /// Probability of yes.
        p_yes: f32,
    },
    /// The likeliest option and every option's probability.
    Choice {
        /// The likeliest option id.
        choice: String,
        /// Probability per option id.
        probabilities: BTreeMap<String, f32>,
        /// Confidence in the choice, 0 to 1.
        confidence: f32,
    },
    /// The probability-weighted level (0 is the lowest; may fall between
    /// levels) and every level's probability.
    Score {
        /// The weighted level.
        score: f32,
        /// Probability per level index.
        probabilities: BTreeMap<usize, f32>,
        /// Confidence, 0 to 1.
        confidence: f32,
    },
}

/// What a decision model answered.
#[derive(Debug, Clone, PartialEq)]
pub struct DecisionResponse {
    /// The model that answered.
    pub model: String,
    /// The answers, by question id.
    pub answers: BTreeMap<String, Answer>,
    /// Input tokens read.
    pub input_tokens: u64,
    /// Output tokens (usually none are billed).
    pub output_tokens: u64,
}

impl DecisionResponse {
    /// The probability of yes for `id`, if it is a yes/no answer.
    #[must_use]
    pub fn yes(&self, id: &str) -> Option<f32> {
        match self.answers.get(id)? {
            Answer::YesNo { p_yes } => Some(*p_yes),
            _ => None,
        }
    }

    /// The chosen option of `id` and its confidence, if it is a choice.
    #[must_use]
    pub fn choice(&self, id: &str) -> Option<(&str, f32)> {
        match self.answers.get(id)? {
            Answer::Choice {
                choice, confidence, ..
            } => Some((choice.as_str(), *confidence)),
            _ => None,
        }
    }

    /// The weighted level of `id`, if it is a score.
    #[must_use]
    pub fn score(&self, id: &str) -> Option<f32> {
        match self.answers.get(id)? {
            Answer::Score { score, .. } => Some(*score),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_respect_the_limits() {
        let ok = DecisionRequest::new("text")
            .ask("urgent", Question::yes_no("Is it urgent?"))
            .ask(
                "team",
                Question::choice("Which team?", [("a", "A"), ("b", "B")]),
            )
            .ask("level", Question::score("How bad?", ["low", "high"]));
        assert_eq!(ok.validate(), Ok(()));
        assert_eq!(
            DecisionRequest::new("x").validate(),
            Err(DecisionRequestError::QuestionCount(0))
        );
        let bad_id = DecisionRequest::new("x").ask("a b", Question::yes_no("?"));
        assert_eq!(
            bad_id.validate(),
            Err(DecisionRequestError::BadId("a b".into()))
        );
        let one_option = DecisionRequest::new("x").ask("c", Question::choice("?", [("a", "A")]));
        assert!(matches!(
            one_option.validate(),
            Err(DecisionRequestError::BadOptions { count: 1, .. })
        ));
        let many = (0..=MAX_QUESTIONS).fold(DecisionRequest::new("x"), |r, i| {
            r.ask(format!("q{i}"), Question::yes_no("?"))
        });
        assert_eq!(
            many.validate(),
            Err(DecisionRequestError::QuestionCount(MAX_QUESTIONS + 1))
        );
    }

    #[test]
    fn answers_are_read_by_kind() {
        let response = DecisionResponse {
            model: "m".into(),
            answers: BTreeMap::from([
                ("y".into(), Answer::YesNo { p_yes: 0.8 }),
                (
                    "c".into(),
                    Answer::Choice {
                        choice: "a".into(),
                        probabilities: BTreeMap::from([("a".into(), 0.9), ("b".into(), 0.1)]),
                        confidence: 0.9,
                    },
                ),
                (
                    "s".into(),
                    Answer::Score {
                        score: 1.5,
                        probabilities: BTreeMap::new(),
                        confidence: 0.5,
                    },
                ),
            ]),
            input_tokens: 10,
            output_tokens: 0,
        };
        assert_eq!(response.yes("y"), Some(0.8));
        assert_eq!(response.choice("c"), Some(("a", 0.9)));
        assert_eq!(response.score("s"), Some(1.5));
        assert_eq!(response.yes("c"), None);
        assert_eq!(response.choice("missing"), None);
    }
}
