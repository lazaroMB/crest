//! The lines sent from the prompt in this session, which `Up` and `Down`
//! bring back ([`PromptHistory`]), as in a shell. The text being written
//! when `Up` first leaves it is the draft: `Down` past the newest line
//! brings it back.

/// The sent lines, oldest first, and where `Up`/`Down` are in them.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PromptHistory {
    entries: Vec<String>,
    /// The entry in the prompt, while browsing.
    index: Option<usize>,
    draft: String,
}

impl PromptHistory {
    /// The lines, oldest first.
    pub fn entries(&self) -> &[String] {
        &self.entries
    }

    /// Replaces the lines (a resumed session's), and stops browsing.
    pub fn set_entries(&mut self, entries: Vec<String>) {
        self.entries = entries;
        self.reset();
    }

    /// Records a sent line and stops browsing; returns whether it was
    /// added (not blank, nor the same as the last one).
    pub fn push(&mut self, line: &str) -> bool {
        self.reset();
        if line.trim().is_empty() || self.entries.last().is_some_and(|last| last == line) {
            return false;
        }
        self.entries.push(line.to_owned());
        true
    }

    /// Whether `Up` has left the draft.
    pub fn browsing(&self) -> bool {
        self.index.is_some()
    }

    /// The line before the one in the prompt; the first time, `current` is
    /// kept as the draft. `None` at the oldest line.
    pub fn older(&mut self, current: &str) -> Option<&str> {
        let ix = match self.index {
            None if self.entries.is_empty() => return None,
            None => {
                self.draft = current.to_owned();
                self.entries.len() - 1
            }
            Some(0) => return None,
            Some(ix) => ix - 1,
        };
        self.index = Some(ix);
        Some(&self.entries[ix])
    }

    /// The line after the one in the prompt, or the draft past the newest.
    /// `None` when not browsing.
    pub fn newer(&mut self) -> Option<String> {
        let ix = self.index? + 1;
        if ix < self.entries.len() {
            self.index = Some(ix);
            return Some(self.entries[ix].clone());
        }
        self.index = None;
        Some(std::mem::take(&mut self.draft))
    }

    fn reset(&mut self) {
        self.index = None;
        self.draft.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn history(lines: &[&str]) -> PromptHistory {
        let mut history = PromptHistory::default();
        for line in lines {
            history.push(line);
        }
        history
    }

    #[test]
    fn blank_lines_and_repeats_are_not_recorded() {
        let history = history(&["uno", "  ", "uno", "dos", "uno"]);
        assert_eq!(history.entries(), ["uno", "dos", "uno"]);
    }

    #[test]
    fn up_goes_back_and_down_returns_to_the_draft() {
        let mut history = history(&["uno", "dos"]);
        assert_eq!(history.newer(), None, "not browsing");
        assert_eq!(history.older("borrador"), Some("dos"));
        assert_eq!(history.older("dos"), Some("uno"));
        assert_eq!(history.older("uno"), None, "the oldest");
        assert!(history.browsing());
        assert_eq!(history.newer().as_deref(), Some("dos"));
        assert_eq!(history.newer().as_deref(), Some("borrador"));
        assert!(!history.browsing());
        assert_eq!(history.newer(), None);
    }

    #[test]
    fn an_empty_history_has_nothing_to_bring_back() {
        let mut history = PromptHistory::default();
        assert_eq!(history.older("x"), None);
        assert!(!history.browsing());
    }

    #[test]
    fn sending_stops_browsing() {
        let mut history = history(&["uno", "dos"]);
        history.older("");
        history.older("");
        assert!(history.push("tres"));
        assert!(!history.browsing());
        assert_eq!(
            history.older("").map(str::to_owned),
            Some("tres".to_owned())
        );
    }
}
