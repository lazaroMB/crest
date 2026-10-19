//! Status bar: the crest mark, active model, permission mode, session
//! cost, git branch and changes, and workspace, plus transient notices
//! (warnings, model fallback, retries).
//!
//! [`StatusState`] is the pure model, updated from [`DomainEvent`]s with an
//! explicit `now` so notice expiry is tested without sleeping;
//! [`StatusView`] draws it and clears a notice when its time is up.

use std::time::{Duration, Instant};

use application::ports::events::{DomainEvent, NoticeLevel};
use domain::conversation::Usage;
use domain::git::GitStatus;
use domain::model::ModelId;
use domain::permission::PermissionMode;
use gpui::{
    App, Context, Hsla, IntoElement, ParentElement as _, Render, SharedString, Styled as _, Task,
    Window, div,
};
use gpui_component::{ActiveTheme as _, h_flex};

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
    git: Option<GitStatus>,
    cost_usd: Option<f64>,
    session_usage: Option<Usage>,
    busy: bool,
    notice: Option<Notice>,
}

impl StatusState {
    /// Status for `workspace` before any turn: no model yet, default
    /// permission mode, no git state yet, no cost.
    pub fn new(workspace: impl Into<String>) -> Self {
        Self {
            workspace: workspace.into(),
            model: None,
            permission_mode: PermissionMode::default(),
            git: None,
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

    /// Git state of the workspace, when it is a repository.
    pub fn git(&self) -> Option<&GitStatus> {
        self.git.as_ref()
    }

    /// Updates the git state; returns whether it changed.
    pub fn set_git(&mut self, git: Option<GitStatus>) -> bool {
        let changed = self.git != git;
        self.git = git;
        changed
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

    /// The fixed segments, left to right: model, permission mode, cost,
    /// git (empty outside a repository) and workspace.
    pub fn segments(&self) -> [String; 5] {
        [
            self.model
                .as_ref()
                .map_or_else(|| "no model".to_owned(), ToString::to_string),
            self.permission_mode.as_str().to_owned(),
            self.cost_label(),
            self.git
                .as_ref()
                .map_or_else(String::new, ToString::to_string),
            self.workspace.clone(),
        ]
    }
}

/// The status bar view.
pub struct StatusView {
    state: StatusState,
    expiry: Option<Task<()>>,
}

impl StatusView {
    /// A status bar for `workspace`.
    pub fn new(workspace: impl Into<String>) -> Self {
        Self {
            state: StatusState::new(workspace),
            expiry: None,
        }
    }

    /// The model behind the view.
    pub fn state(&self) -> &StatusState {
        &self.state
    }

    /// Mutable access, e.g. to set the configured model before a turn.
    pub fn state_mut(&mut self, cx: &mut Context<Self>) -> &mut StatusState {
        cx.notify();
        &mut self.state
    }

    /// Shows the git state of the workspace.
    pub fn set_git(&mut self, git: Option<GitStatus>, cx: &mut Context<Self>) {
        if self.state.set_git(git) {
            cx.notify();
        }
    }

    /// Applies a domain event, scheduling the removal of a new notice.
    pub fn apply(&mut self, event: &DomainEvent, cx: &mut Context<Self>) {
        let now = cx.background_executor().now();
        let had_notice = self.state.notice(now).cloned();
        if !self.state.apply(event, now) {
            return;
        }
        if self.state.notice(now).cloned() != had_notice {
            self.expiry = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(NOTICE_TTL).await;
                this.update(cx, |this, cx| {
                    let now = cx.background_executor().now();
                    if this.state.expire(now) {
                        cx.notify();
                    }
                })
                .ok();
            }));
        }
        cx.notify();
    }
}

fn notice_color(level: NoticeLevel, cx: &App) -> Hsla {
    let theme = cx.theme();
    match level {
        NoticeLevel::Info => theme.info,
        NoticeLevel::Warning => theme.warning,
        NoticeLevel::Error => theme.danger,
    }
}

impl Render for StatusView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let now = cx.background_executor().now();
        let [model, permission, cost, git, workspace] = self.state.segments();
        let muted = theme.muted_foreground;
        let sep = || div().text_color(muted).child("│");
        let mut left = h_flex()
            .gap_2()
            .child(crate::brand::mark(14.0))
            .child(sep())
            .child(SharedString::from(model));
        if self.state.is_busy() {
            left = left.child(div().text_color(theme.info).child("●"));
        }
        left = left
            .child(sep())
            .child(SharedString::from(permission))
            .child(sep())
            .child(SharedString::from(cost));
        let mut right = h_flex().gap_2();
        if let Some(notice) = self.state.notice(now) {
            right = right.child(
                div()
                    .text_color(notice_color(notice.level, cx))
                    .child(SharedString::from(notice.message.clone())),
            );
        }
        if let Some(status) = self.state.git() {
            let color = if status.conflicted > 0 {
                theme.danger
            } else if status.is_clean() {
                theme.success
            } else {
                theme.warning
            };
            right = right
                .child(div().text_color(color).child(SharedString::from(git)))
                .child(sep());
        }
        right = right.child(div().text_color(muted).child(SharedString::from(workspace)));
        h_flex()
            .w_full()
            .justify_between()
            .text_sm()
            .child(left)
            .child(right)
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
            ["no model", "ask", "$0.00", "", "proyecto-ide"].map(String::from)
        );
        state.apply(&started("claude-sonnet-5-5"), now);
        state.apply(
            &DomainEvent::ModeChanged {
                mode: PermissionMode::Plan,
            },
            now,
        );
        state.apply(&usage(Some(0.0421)), now);
        assert!(state.set_git(Some(GitStatus::parse("# branch.head main\n? a\n"))));
        assert!(!state.set_git(state.git().cloned()), "unchanged");
        assert_eq!(
            state.segments(),
            [
                "claude-sonnet-5-5",
                "plan",
                "$0.0421",
                "main ?1",
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

    #[gpui::test]
    fn view_draws_and_clears_the_notice(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, _| StatusView::new("proyecto-ide"));
        view.update(cx, |view, cx| {
            view.apply(&started("claude-sonnet-5-5"), cx);
            view.apply(
                &DomainEvent::Notice {
                    level: NoticeLevel::Info,
                    message: "session saved".into(),
                },
                cx,
            );
        });
        cx.draw(
            gpui::point(gpui::px(0.), gpui::px(0.)),
            gpui::size(gpui::px(1000.), gpui::px(24.)),
            |_, _| div().size_full().child(view.clone()),
        );
        view.read_with(cx, |view, _| assert!(view.state.notice.is_some()));
        cx.executor().advance_clock(NOTICE_TTL);
        cx.run_until_parked();
        view.read_with(cx, |view, _| {
            assert!(view.state.notice.is_none());
            assert_eq!(
                view.state().model().map(ModelId::as_str),
                Some("claude-sonnet-5-5")
            );
        });
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
