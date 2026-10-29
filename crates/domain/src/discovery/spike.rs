//! A spike: a short experiment whose hypothesis and pass criteria are
//! frozen before it runs, so the result cannot be negotiated afterwards.

use serde::{Deserialize, Serialize};

use super::event::{SpikeId, SpikeScore};

/// Why a spike could not be frozen or scored.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SpikeError {
    /// No criteria, or `k` outside `1..=n`.
    #[error("a spike needs `k` of `n` criteria with 1 <= k <= n, got k = {k}, n = {n}")]
    BadThreshold {
        /// Criteria that must hold.
        k: u32,
        /// Criteria declared.
        n: u32,
    },
    /// An empty hypothesis or criterion.
    #[error("a spike's hypothesis and criteria cannot be empty")]
    Empty,
    /// The outcome lists a different number of criteria than were frozen.
    #[error("the spike froze {frozen} criteria but {given} outcomes were given")]
    OutcomeMismatch {
        /// Criteria frozen.
        frozen: usize,
        /// Outcomes given.
        given: usize,
    },
    /// The spike was already scored.
    #[error("the spike is already scored")]
    AlreadyScored,
}

/// A spike record, as stored next to the discovery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Spike {
    /// Its id.
    pub id: SpikeId,
    /// What it tests.
    pub hypothesis: String,
    /// The criteria, frozen before the run.
    pub criteria: Vec<String>,
    /// How many criteria must hold.
    pub k: u32,
    /// What was observed, filled after the run.
    #[serde(default)]
    pub facts: String,
    /// Whether each criterion held, in the order of `criteria`; empty
    /// until scored.
    #[serde(default)]
    pub met: Vec<bool>,
}

impl Spike {
    /// Freezes a spike: `k` of `criteria` must hold.
    ///
    /// # Errors
    ///
    /// [`SpikeError::Empty`] or [`SpikeError::BadThreshold`].
    pub fn frozen(
        id: SpikeId,
        hypothesis: impl Into<String>,
        criteria: Vec<String>,
        k: u32,
    ) -> Result<Self, SpikeError> {
        let hypothesis = hypothesis.into();
        if hypothesis.trim().is_empty() || criteria.iter().any(|c| c.trim().is_empty()) {
            return Err(SpikeError::Empty);
        }
        let n = u32::try_from(criteria.len()).unwrap_or(u32::MAX);
        if n == 0 || k == 0 || k > n {
            return Err(SpikeError::BadThreshold { k, n });
        }
        Ok(Self {
            id,
            hypothesis,
            criteria,
            k,
            facts: String::new(),
            met: Vec::new(),
        })
    }

    /// Whether the spike has been scored.
    #[must_use]
    pub fn is_scored(&self) -> bool {
        self.met.len() == self.criteria.len()
    }

    /// Records the outcome: `facts` and whether each criterion held.
    ///
    /// # Errors
    ///
    /// [`SpikeError::AlreadyScored`] or [`SpikeError::OutcomeMismatch`].
    pub fn record(
        &mut self,
        facts: impl Into<String>,
        met: Vec<bool>,
    ) -> Result<SpikeScore, SpikeError> {
        if self.is_scored() {
            return Err(SpikeError::AlreadyScored);
        }
        if met.len() != self.criteria.len() {
            return Err(SpikeError::OutcomeMismatch {
                frozen: self.criteria.len(),
                given: met.len(),
            });
        }
        self.facts = facts.into();
        self.met = met;
        Ok(self.score().unwrap_or(SpikeScore::NotSatisfactory))
    }

    /// Criteria that held so far.
    #[must_use]
    pub fn held(&self) -> u32 {
        u32::try_from(self.met.iter().filter(|m| **m).count()).unwrap_or(u32::MAX)
    }

    /// The score, once recorded: satisfactory when at least `k` held.
    #[must_use]
    pub fn score(&self) -> Option<SpikeScore> {
        self.is_scored().then(|| {
            if self.held() >= self.k {
                SpikeScore::Satisfactory
            } else {
                SpikeScore::NotSatisfactory
            }
        })
    }

    /// The spike as a Markdown file: hypothesis, the frozen criteria with
    /// their outcome once scored, the facts and the result.
    #[must_use]
    pub fn to_markdown(&self) -> String {
        let mut out = format!(
            "# {}: {}\n\n## Hypothesis\n\n{}\n\n",
            self.id,
            first_line(&self.hypothesis),
            self.hypothesis
        );
        out.push_str(&format!(
            "## Validated if ({} of {})\n\n",
            self.k,
            self.criteria.len()
        ));
        for (i, criterion) in self.criteria.iter().enumerate() {
            let mark = match self.met.get(i) {
                Some(true) => " **Met.**",
                Some(false) => " **Not met.**",
                None => "",
            };
            out.push_str(&format!("{}. {criterion}{mark}\n", i + 1));
        }
        out.push_str("\n## Facts\n\n");
        if self.facts.trim().is_empty() {
            out.push_str("_Not run yet._\n");
        } else {
            out.push_str(self.facts.trim());
            out.push('\n');
        }
        out.push_str("\n## Result\n\n");
        match self.score() {
            Some(SpikeScore::Satisfactory) => out.push_str(&format!(
                "**{} of {} criteria met; threshold {}. Satisfactory.**\n",
                self.held(),
                self.criteria.len(),
                self.k
            )),
            Some(SpikeScore::NotSatisfactory) => out.push_str(&format!(
                "**{} of {} criteria met; threshold {}. Not satisfactory.** A new hypothesis is needed, not a weaker criterion.\n",
                self.held(),
                self.criteria.len(),
                self.k
            )),
            None => out.push_str("_Not scored yet._\n"),
        }
        out
    }
}

fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or("").trim()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spike() -> Spike {
        Spike::frozen(
            SpikeId(1),
            "a small model can drive the engine",
            vec!["replays".into(), "80% pass".into(), "one loop".into()],
            2,
        )
        .unwrap()
    }

    #[test]
    fn freezing_checks_the_threshold_and_the_text() {
        assert_eq!(
            Spike::frozen(SpikeId(1), "h", vec!["a".into()], 2).unwrap_err(),
            SpikeError::BadThreshold { k: 2, n: 1 }
        );
        assert_eq!(
            Spike::frozen(SpikeId(1), "h", vec![], 0).unwrap_err(),
            SpikeError::BadThreshold { k: 0, n: 0 }
        );
        assert_eq!(
            Spike::frozen(SpikeId(1), " ", vec!["a".into()], 1).unwrap_err(),
            SpikeError::Empty
        );
    }

    #[test]
    fn scoring_counts_k_of_n_and_happens_once() {
        let mut s = spike();
        assert_eq!(s.score(), None);
        assert_eq!(
            s.record("ran", vec![true, false]).unwrap_err(),
            SpikeError::OutcomeMismatch {
                frozen: 3,
                given: 2
            }
        );
        assert_eq!(
            s.record("ran", vec![true, true, false]).unwrap(),
            SpikeScore::Satisfactory
        );
        assert_eq!(
            s.record("again", vec![false, false, false]).unwrap_err(),
            SpikeError::AlreadyScored
        );
        let mut failing = spike();
        assert_eq!(
            failing.record("ran", vec![true, false, false]).unwrap(),
            SpikeScore::NotSatisfactory
        );
    }

    #[test]
    fn markdown_shows_the_frozen_criteria_and_the_result() {
        let mut s = spike();
        let before = s.to_markdown();
        assert!(before.contains("## Validated if (2 of 3)"));
        assert!(before.contains("_Not scored yet._"));
        s.record("it ran", vec![true, true, false]).unwrap();
        let after = s.to_markdown();
        assert!(after.contains("1. replays **Met.**"));
        assert!(after.contains("3. one loop **Not met.**"));
        assert!(after.contains("2 of 3 criteria met; threshold 2. Satisfactory."));
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<Spike>(&json).unwrap(), s);
    }
}
