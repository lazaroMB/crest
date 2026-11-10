//! The information mark: one small mark in the bottom-right corner of the
//! window for what should not sit in the prompt or the status bar. When an
//! error or warning notice is kept, or a step too short for the activity
//! line ends, that one entry shows as a toast for [`TOAST_FOR`]; then every
//! kept notice and short step waits behind the mark. A click on the mark
//! opens a panel with them: a notice has its own `Dismiss`, a step is
//! read-only. `Esc` or a click elsewhere closes the panel.
//!
//! The status bar hosts the mark and lays it out `absolute`, anchored to
//! the top of the bar and growing upward, so it adds no height, pushes
//! nothing and never covers the bar's own text (see `docs/usage.md`).

mod render;
mod state;

use gpui::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, InteractiveElement as _,
    IntoElement, KeyDownEvent, ParentElement as _, Render, Styled as _, Subscription, Task, Window,
};
use gpui_component::v_flex;

use crate::activity::ActivityView;
use crate::input::InputView;
use crate::session::Session;
use crate::status::StatusView;
pub use render::PANEL_MAX_HEIGHT;
pub use state::{InfoState, Row, TOAST_FOR, Toast, rows, step_text};

/// The information mark, its toast and its panel.
pub struct InfoView {
    state: InfoState,
    focus: FocusHandle,
    expiry: Option<Task<()>>,
    sources: Sources,
    _subscriptions: [Subscription; 2],
}

/// Where the entries come from, and who gets the keyboard back.
struct Sources {
    activity: Entity<ActivityView>,
    status: Entity<StatusView>,
    /// Takes the keyboard back when the panel closes.
    input: Entity<InputView>,
}

impl Focusable for InfoView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl InfoView {
    /// A mark over the kept notices of `status` and the steps of
    /// `activity`. What they hold now counts as seen: no toast for it.
    pub fn new(
        status: Entity<StatusView>,
        activity: Entity<ActivityView>,
        input: Entity<InputView>,
        cx: &mut Context<Self>,
    ) -> Self {
        let state = InfoState::seen(
            status.read(cx).state().kept.items(),
            activity.read(cx).state().steps.items(),
        );
        let subscriptions = [
            cx.observe(&status, |this, _, cx| this.look(cx)),
            cx.observe(&activity, |this, _, cx| this.look(cx)),
        ];
        Self {
            state,
            focus: cx.focus_handle(),
            expiry: None,
            sources: Sources {
                activity,
                status,
                input,
            },
            _subscriptions: subscriptions,
        }
    }

    /// The rows the panel lists now.
    pub fn rows(&self, cx: &App) -> Vec<Row> {
        rows(
            self.sources.status.read(cx).state().kept.items(),
            self.sources.activity.read(cx).state().steps.items(),
        )
    }

    /// Notices what the logs gained and toasts it for [`TOAST_FOR`]; the
    /// toast hides itself by its own time, the timer only repaints.
    fn look(&mut self, cx: &mut Context<Self>) {
        let now = cx.background_executor().now();
        let kept = self.sources.status.read(cx).state().kept.items();
        let steps = self.sources.activity.read(cx).state().steps.items();
        if self.state.sync(kept, steps, now) {
            self.expiry = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(TOAST_FOR).await;
                this.update(cx, |_, cx| cx.notify()).ok();
            }));
        }
        cx.notify();
    }

    /// Opens the panel, or closes it.
    pub fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.state.is_open() {
            self.close(window, cx);
        } else {
            self.state.set_open(true);
            window.focus(&self.focus, cx);
            cx.notify();
        }
    }

    /// Closes the panel; the prompt gets the keyboard back when the panel
    /// had it.
    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.state.is_open() {
            return;
        }
        self.state.set_open(false);
        if self.focus.contains_focused(window, cx) {
            let input = self.sources.input.clone();
            input.update(cx, |input, cx| input.focus(window, cx));
        }
        cx.notify();
    }

    /// Dismisses the kept notice `id`, and only it.
    pub fn dismiss(&mut self, id: u64, cx: &mut Context<Self>) {
        self.sources
            .status
            .update(cx, |status, cx| status.state_mut(cx).kept.dismiss(id));
        self.state.clear_toast();
        cx.notify();
    }

    /// Closes the panel once nothing is left in it.
    pub fn settle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.rows(cx).is_empty() {
            self.close(window, cx);
        }
    }
}

impl Render for InfoView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = cx.background_executor().now();
        let rows = self.rows(cx);
        let mut corner = v_flex()
            .id("info")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if this.state.is_open() && event.keystroke.key == "escape" {
                    this.close(window, cx);
                    cx.stop_propagation();
                }
            }))
            .absolute()
            .bottom_full()
            .right_0()
            .mb_2()
            .items_end()
            .gap_1();
        if self.state.is_open() {
            corner = corner
                .on_mouse_down_out(cx.listener(|this, _, window, cx| this.close(window, cx)))
                .child(render::panel(&rows, cx));
        } else if let Some(toast) = self.state.toast(now) {
            corner = corner.child(render::toast(toast, cx));
        }
        if !rows.is_empty() {
            corner = corner.child(render::mark(&rows, cx));
        }
        corner
    }
}

/// Gives `session`'s status bar its information mark.
pub fn attach(session: &Entity<Session>, cx: &mut App) {
    let s = session.read(cx);
    let (status, activity, input) = (s.status().clone(), s.activity().clone(), s.input().clone());
    let info = cx.new(|cx| InfoView::new(status.clone(), activity, input, cx));
    status.update(cx, |status, cx| status.set_info(info, cx));
}

#[cfg(test)]
mod tests;
