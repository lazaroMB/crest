//! The model of the information mark: which entries are new (so one shows
//! as a toast), whether the panel is open, and the rows the panel lists.
//! It takes an explicit `now`, so the toast's three seconds are tested
//! without sleeping.

use std::time::{Duration, Instant};

use application_core::ports::events::NoticeLevel;

use crate::activity::Step;
use crate::status::KeptNotice;

/// How long a new entry shows as a toast before it parks behind the mark.
pub const TOAST_FOR: Duration = Duration::from_secs(3);

/// A new entry, shown for [`TOAST_FOR`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toast {
    /// Severity, which picks the color (a short step is `Info`).
    pub level: NoticeLevel,
    /// What it says.
    pub text: String,
    /// When it goes.
    pub until: Instant,
}

/// One line of the panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// The kept notice it is, when it can be dismissed; a step is read-only.
    pub notice: Option<u64>,
    /// Severity.
    pub level: NoticeLevel,
    /// What it says.
    pub text: String,
}

/// The text of a short step.
pub fn step_text(step: &Step) -> String {
    format!("{} · {} ms", step.label, step.elapsed.as_millis())
}

/// The rows of the panel: the kept notices, then the short steps, each
/// newest first. The logs bound them.
pub fn rows(kept: &[KeptNotice], steps: &[Step]) -> Vec<Row> {
    let notices = kept.iter().rev().map(|notice| Row {
        notice: Some(notice.id),
        level: notice.level,
        text: notice.message.clone(),
    });
    let short = steps
        .iter()
        .rev()
        .filter(|step| step.short)
        .map(|step| Row {
            notice: None,
            level: NoticeLevel::Info,
            text: step_text(step),
        });
    notices.chain(short).collect()
}

/// What the mark remembers between two looks at the logs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InfoState {
    /// Entries below these ids were seen already.
    next_notice: u64,
    next_step: u64,
    toast: Option<Toast>,
    open: bool,
}

impl InfoState {
    /// A state that takes what the logs hold now as seen: no toast for it.
    pub fn seen(kept: &[KeptNotice], steps: &[Step]) -> Self {
        let mut state = Self::default();
        state.sync(kept, steps, Instant::now());
        state.toast = None;
        state
    }

    /// Looks at the logs at `now`: of the entries added since the last look
    /// the newest becomes the toast. Returns whether there was one.
    pub fn sync(&mut self, kept: &[KeptNotice], steps: &[Step], now: Instant) -> bool {
        let notice = kept.iter().rfind(|n| n.id >= self.next_notice);
        let step = steps.iter().rfind(|s| s.id >= self.next_step && s.short);
        self.next_notice = kept.last().map_or(self.next_notice, |n| n.id + 1);
        self.next_step = steps.last().map_or(self.next_step, |s| s.id + 1);
        // Both in one look: the notice, which needs the person more.
        let (level, text) = match (notice, step) {
            (Some(n), _) => (n.level, n.message.clone()),
            (None, Some(s)) => (NoticeLevel::Info, step_text(s)),
            (None, None) => return false,
        };
        self.toast = Some(Toast {
            level,
            text,
            until: now + TOAST_FOR,
        });
        true
    }

    /// The toast still showing at `now`. The open panel lists everything,
    /// so no toast shows over it.
    pub fn toast(&self, now: Instant) -> Option<&Toast> {
        self.toast
            .as_ref()
            .filter(|toast| !self.open && toast.until > now)
    }

    /// Drops the toast (its entry was dismissed).
    pub fn clear_toast(&mut self) {
        self.toast = None;
    }

    /// Whether the panel is open.
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Opens or closes the panel.
    pub fn set_open(&mut self, open: bool) {
        self.open = open;
        self.toast = None;
    }
}
