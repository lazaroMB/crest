//! Status bar: input mode, active model, permission mode, session cost and
//! workspace, plus transient notices (warnings, model fallback, retries).
//!
//! [`StatusState`] is the pure model, updated from [`DomainEvent`]s with an
//! explicit `now` so notice expiry is tested without sleeping.

use std::time::{Duration, Instant};

use application::keymap::Mode;
use application::ports::events::{DomainEvent, NoticeLevel};
use domain::conversation::Usage;
use domain::model::ModelId;
use domain::permission::PermissionMode;

use crate::input::mode_label;

/// How long a transient notice stays in the status bar.
pub const NOTICE_TTL: Duration = Duration::from_secs(6);

/// Formats a cost in USD: two decimals from one dollar up, otherwise up to
/// four (trailing zeros beyond the second dropped), so small sessions
/// still show a change.
pub fn format_cost(usd: f64) -> String {
    let usd = if usd.is_finite() { usd.max(0.0) } else { 0.0 };
    if usd >= 1.0 {
        return format!("${usd:.2}");
    }
    let mut text = format!("{usd:.4}");
    while text.ends_with('0') && text.len() > "0.00".len() {
        text.pop();
    }
    format!("${text}")
}

/// A message shown for a while in the status bar.
#[derive(Debug, Clone, PartialEq)]
pub struct Notice {
    /// Severity, which picks the color.
    pub level: NoticeLevel,
    /// Text to show.
    pub message: String,
    /// When it disappears.
    pub expires_at: Instant,
}

/// What the status bar shows.
#[derive(Debug, Clone, PartialEq)]
pub struct StatusState {
    workspace: String,
    model: Option<ModelId>,
    permission_mode: PermissionMode,
    input_mode: Mode,
    cost_usd: Option<f64>,
    session_usage: Option<Usage>,
    busy: bool,
    notice: Option<Notice>,
}

impl StatusState {
    /// Status for `workspace` before any turn: no model yet, default
    /// permission mode, insert mode, no cost.
    pub fn new(workspace: impl Into<String>) -> Self {
        Self {
            workspace: workspace.into(),
            model: None,
            permission_mode: PermissionMode::default(),
            input_mode: Mode::Insert,
            cost_usd: None,
            session_usage: None,
            busy: false,
            notice: None,
        }
    }

    /// Workspace label.
    pub fn workspace(&self) -> &str {
        &self.workspace
    }

    /// Model of the current or last turn.
    pub fn model(&self) -> Option<&ModelId> {
        self.model.as_ref()
    }

    /// Sets the model shown before the first turn (the configured one).
    pub fn set_model(&mut self, model: ModelId) {
        self.model = Some(model);
    }

    /// Permission mode.
    pub fn permission_mode(&self) -> PermissionMode {
        self.permission_mode
    }

    /// Sets the permission mode shown before any `ModeChanged`.
    pub fn set_permission_mode(&mut self, mode: PermissionMode) {
        self.permission_mode = mode;
    }

    /// Mode of the prompt input.
    pub fn input_mode(&self) -> Mode {
        self.input_mode
    }

    /// Updates the input mode (the input emits it, not the domain).
    pub fn set_input_mode(&mut self, mode: Mode) {
        self.input_mode = mode;
    }

    /// Cost of the session in USD, when pricing is known.
    pub fn cost_usd(&self) -> Option<f64> {
        self.cost_usd
    }

    /// Whether a turn is running.
    pub fn is_busy(&self) -> bool {
        self.busy
    }

    /// The notice still visible at `now`.
    pub fn notice(&self, now: Instant) -> Option<&Notice> {
        self.notice
            .as_ref()
            .filter(|notice| notice.expires_at > now)
    }

    /// Drops an expired notice; returns whether one was dropped.
    pub fn expire(&mut self, now: Instant) -> bool {
        let expired = self.notice.as_ref().is_some_and(|n| n.expires_at <= now);
        if expired {
            self.notice = None;
        }
        expired
    }

    /// Applies an event received at `now`. Returns whether anything shown
    /// changed.
    pub fn apply(&mut self, event: &DomainEvent, now: Instant) -> bool {
        match event {
            DomainEvent::TurnStarted { model, .. } => {
                self.model = Some(model.clone());
                self.busy = true;
            }
            DomainEvent::TurnFinished { .. } => self.busy = false,
            DomainEvent::TurnFailed { error, .. } => {
                self.busy = false;
                self.show(NoticeLevel::Error, format!("turn failed: {error}"), now);
            }
            DomainEvent::ModeChanged { mode } => self.permission_mode = *mode,
            DomainEvent::UsageUpdated {
                session, cost_usd, ..
            } => {
                self.session_usage = Some(*session);
                if cost_usd.is_some() {
                    self.cost_usd = *cost_usd;
                }
            }
            DomainEvent::Notice { level, message } => self.show(*level, message.clone(), now),
            DomainEvent::ModelFallback { from, to } => {
                self.model = Some(to.clone());
                self.show(
                    NoticeLevel::Warning,
                    format!("{from} failed, using {to}"),
                    now,
                );
            }
            DomainEvent::Retrying { attempt, delay_ms } => {
                let secs = *delay_ms as f64 / 1000.0;
                self.show(
                    NoticeLevel::Warning,
                    format!("retrying in {secs:.1}s (attempt {attempt})"),
                    now,
                );
            }
            _ => return false,
        }
        true
    }

    fn show(&mut self, level: NoticeLevel, message: String, now: Instant) {
        self.notice = Some(Notice {
            level,
            message,
            expires_at: now + NOTICE_TTL,
        });
    }

    /// Cost label: the session cost, `$0.00` before any usage and `$?`
    /// when usage arrived without pricing.
    pub fn cost_label(&self) -> String {
        match (self.cost_usd, self.session_usage) {
            (Some(cost), _) => format_cost(cost),
            (None, Some(_)) => "$?".to_owned(),
            (None, None) => format_cost(0.0),
        }
    }

    /// The fixed segments, left to right: input mode, model, permission
    /// mode, cost and workspace.
    pub fn segments(&self) -> [String; 5] {
        [
            mode_label(self.input_mode).to_owned(),
            self.model
                .as_ref()
                .map_or_else(|| "no model".to_owned(), ToString::to_string),
            self.permission_mode.as_str().to_owned(),
            self.cost_label(),
            self.workspace.clone(),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::conversation::{SessionId, StopReason, TurnId};
    use domain::model::ModelRole;

    fn started(model: &str) -> DomainEvent {
        DomainEvent::TurnStarted {
            session_id: SessionId::new(),
            turn_id: TurnId::new(),
            model: ModelId::new(model),
            role: ModelRole::Main,
        }
    }

    fn usage(cost_usd: Option<f64>) -> DomainEvent {
        DomainEvent::UsageUpdated {
            turn: Usage::default(),
            session: Usage::default(),
            cost_usd,
        }
    }

    #[test]
    fn cost_has_two_to_four_decimals() {
        assert_eq!(format_cost(0.0), "$0.00");
        assert_eq!(format_cost(0.25), "$0.25");
        assert_eq!(format_cost(0.0123), "$0.0123");
        assert_eq!(format_cost(0.00004), "$0.00");
        assert_eq!(format_cost(0.120), "$0.12");
        assert_eq!(format_cost(1.234), "$1.23");
        assert_eq!(format_cost(12.5), "$12.50");
        assert_eq!(format_cost(f64::NAN), "$0.00");
    }

    #[test]
    fn shows_model_mode_cost_and_workspace() {
        let now = Instant::now();
        let mut state = StatusState::new("proyecto-ide");
        assert_eq!(
            state.segments(),
            ["INSERT", "no model", "ask", "$0.00", "proyecto-ide"].map(String::from)
        );
        state.apply(&started("claude-sonnet-5-5"), now);
        state.apply(
            &DomainEvent::ModeChanged {
                mode: PermissionMode::Plan,
            },
            now,
        );
        state.apply(&usage(Some(0.0421)), now);
        state.set_input_mode(Mode::Normal);
        assert_eq!(
            state.segments(),
            [
                "NORMAL",
                "claude-sonnet-5-5",
                "plan",
                "$0.0421",
                "proyecto-ide"
            ]
            .map(String::from)
        );
    }

    #[test]
    fn usage_without_pricing_keeps_last_cost() {
        let now = Instant::now();
        let mut state = StatusState::new("w");
        state.apply(&usage(None), now);
        assert_eq!(state.cost_label(), "$?");
        state.apply(&usage(Some(0.5)), now);
        state.apply(&usage(None), now);
        assert_eq!(state.cost_label(), "$0.50");
    }

    #[test]
    fn turn_lifecycle_sets_busy() {
        let now = Instant::now();
        let mut state = StatusState::new("w");
        state.apply(&started("m"), now);
        assert!(state.is_busy());
        state.apply(
            &DomainEvent::TurnFinished {
                turn_id: TurnId::new(),
                stop_reason: StopReason::EndTurn,
            },
            now,
        );
        assert!(!state.is_busy());
    }

    #[test]
    fn notices_are_transient() {
        let now = Instant::now();
        let mut state = StatusState::new("w");
        state.apply(
            &DomainEvent::Notice {
                level: NoticeLevel::Warning,
                message: "unknown model".into(),
            },
            now,
        );
        let notice = state.notice(now).map(|n| (n.level, n.message.clone()));
        assert_eq!(notice, Some((NoticeLevel::Warning, "unknown model".into())));
        assert!(!state.expire(now + NOTICE_TTL / 2));
        assert!(state.notice(now + NOTICE_TTL).is_none());
        assert!(state.expire(now + NOTICE_TTL));
        assert!(state.notice(now).is_none());
    }

    #[test]
    fn fallback_switches_model_and_warns() {
        let now = Instant::now();
        let mut state = StatusState::new("w");
        state.apply(&started("opus"), now);
        state.apply(
            &DomainEvent::ModelFallback {
                from: ModelId::new("opus"),
                to: ModelId::new("sonnet"),
            },
            now,
        );
        assert_eq!(state.model().map(ModelId::as_str), Some("sonnet"));
        let message = state.notice(now).map(|n| n.message.as_str());
        assert_eq!(message, Some("opus failed, using sonnet"));
    }

    #[test]
    fn retrying_and_failed_turns_show_notices() {
        let now = Instant::now();
        let mut state = StatusState::new("w");
        state.apply(
            &DomainEvent::Retrying {
                attempt: 2,
                delay_ms: 1500,
            },
            now,
        );
        let message = state.notice(now).map(|n| n.message.clone());
        assert_eq!(message.as_deref(), Some("retrying in 1.5s (attempt 2)"));
        state.apply(
            &DomainEvent::TurnFailed {
                turn_id: TurnId::new(),
                error: "overloaded".into(),
            },
            now,
        );
        let notice = state.notice(now).map(|n| (n.level, n.message.clone()));
        assert_eq!(
            notice,
            Some((NoticeLevel::Error, "turn failed: overloaded".into()))
        );
    }

    #[test]
    fn unrelated_events_change_nothing() {
        let mut state = StatusState::new("w");
        let before = state.clone();
        let changed = state.apply(
            &DomainEvent::TextDelta {
                block_id: domain::block::BlockId::new(),
                text: "x".into(),
            },
            Instant::now(),
        );
        assert!(!changed);
        assert_eq!(state, before);
    }
}
