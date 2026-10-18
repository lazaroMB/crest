//! Modal prompt input: a multi-line editor (gpui-component `Textarea`)
//! driven by the Neovim-style [`KeySequencer`].
//!
//! - **insert**: the editor has focus. Keys the keymap does not map go to
//!   the editor untouched; `<CR>` submits ([`InputEvent::Submit`]),
//!   `<S-CR>` inserts a newline and `<Esc>` switches to normal.
//! - **normal**: the view's own focus handle has focus, so no text reaches
//!   the editor. Mapped keys (`j`, `k`, `gg`, `G`, `<CR>`, `t`, `y`...)
//!   are emitted as [`InputEvent::Action`] for the timeline; `i` goes back
//!   to insert and `:` opens the command line.
//! - **command**: a one-line buffer below the editor. `<CR>` emits
//!   [`InputEvent::Command`], `<Esc>` (or backspace on an empty line) goes
//!   back to normal.
//!
//! It looks like a terminal prompt: a `❯` colored by mode, the text in the
//! monospace font with no field around it, and a blinking block cursor
//! ([`cursor`]); in normal mode the block is an outline, and in command mode
//! it blinks at the end of the `:` line.
//!
//! Keys are read with a keystroke interceptor, which runs before GPUI's own
//! key bindings and the editor, while this view or its editor has focus.
//! A pending prefix (`g` of `gg`) is settled by a GPUI timer at the
//! sequencer's deadline (`timeoutlen`).
//!
//! Limitation: keys of a multi-key insert-mode mapping that ends up not
//! matching are replayed into the editor as text (`Enter` as a newline);
//! other non-character keys of such a failed prefix are dropped. Text that
//! an IME commits directly, without key events, bypasses the sequencer.

pub mod cursor;

use application::keymap::{
    Action, Feed, Key, KeyCode, KeySequencer, Keymap, Mode, Modifiers, default_keymap,
};
use gpui::canvas;
use gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, KeystrokeEvent, ParentElement as _, Render, SharedString,
    Styled as _, Subscription, Task, Window, div,
};
use gpui_component::input::{InputEvent as EditorEvent, Textarea, TextareaState};

use cursor::{BLINK_INTERVAL, Blink, CursorShape, HiddenCaret, paint_cursor};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};

/// Action of `Up` in an empty prompt while prompts are queued: takes the
/// last one back into the prompt.
pub const QUEUE_EDIT_ACTION: &str = "queue.edit";

/// Rows of the editor before it starts scrolling.
const MAX_ROWS: usize = 8;

/// What the input asks the rest of the UI to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputEvent {
    /// The prompt was sent with `<CR>` in insert mode (never empty).
    Submit(String),
    /// A `:` command was run, without the colon.
    Command(String),
    /// A mapped action the input does not handle itself (timeline
    /// navigation, pickers, interrupt...).
    Action(Action),
    /// The mode changed.
    ModeChanged(Mode),
}

/// Short name of a mode for the mode indicator.
pub fn mode_label(mode: Mode) -> &'static str {
    match mode {
        Mode::Insert => "INSERT",
        Mode::Normal => "NORMAL",
        Mode::Command => "COMMAND",
    }
}

/// Converts a GPUI keystroke to a keymap [`Key`]. Returns `None` for a bare
/// modifier press.
///
/// Printable keys use the character the platform produced (so `shift-g` is
/// `G` and `shift-;` is `:`) unless Ctrl, Alt or Cmd is held.
pub fn key_from_keystroke(keystroke: &gpui::Keystroke) -> Option<Key> {
    let m = &keystroke.modifiers;
    let named = match keystroke.key.as_str() {
        "shift" | "control" | "alt" | "platform" | "function" | "" => return None,
        "enter" => Some(KeyCode::Enter),
        "escape" => Some(KeyCode::Esc),
        "tab" => Some(KeyCode::Tab),
        "backspace" => Some(KeyCode::Backspace),
        "delete" => Some(KeyCode::Delete),
        "up" => Some(KeyCode::Up),
        "down" => Some(KeyCode::Down),
        "left" => Some(KeyCode::Left),
        "right" => Some(KeyCode::Right),
        "home" => Some(KeyCode::Home),
        "end" => Some(KeyCode::End),
        "pageup" => Some(KeyCode::PageUp),
        "pagedown" => Some(KeyCode::PageDown),
        "space" => Some(KeyCode::Char(' ')),
        key => key
            .strip_prefix('f')
            .and_then(|n| n.parse::<u8>().ok())
            .filter(|n| (1..=24).contains(n))
            .map(KeyCode::F),
    };
    let chord = m.control || m.alt || m.platform;
    let mut mods = Modifiers {
        ctrl: m.control,
        alt: m.alt,
        shift: m.shift,
        cmd: m.platform,
    };
    let code = match named {
        Some(code) => code,
        None => {
            let typed = keystroke.key_char.as_deref().filter(|_| !chord);
            let c = match typed.and_then(single_char) {
                Some(c) => {
                    // The character already carries the shift.
                    mods.shift = false;
                    c
                }
                None => single_char(&keystroke.key)?,
            };
            KeyCode::Char(c)
        }
    };
    Some(Key::new(code, mods))
}

fn single_char(s: &str) -> Option<char> {
    let mut chars = s.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => Some(c),
        _ => None,
    }
}

/// Text a replayed key inserts into the editor or the command line.
fn key_text(key: &Key) -> Option<char> {
    if key.mods.ctrl || key.mods.alt || key.mods.cmd {
        return None;
    }
    match key.code {
        KeyCode::Char(c) => Some(c),
        KeyCode::Enter => Some('\n'),
        _ => None,
    }
}

/// The prompt input with its modes.
pub struct InputView {
    sequencer: KeySequencer<Action>,
    editor: Entity<TextareaState>,
    focus_handle: FocusHandle,
    command: String,
    timeout: Option<Task<()>>,
    queued: bool,
    blink: Blink,
    _blink: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<InputEvent> for InputView {}

impl InputView {
    /// An input with the builtin keymap, starting in insert mode.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::with_keymap(default_keymap(), window, cx)
    }

    /// An input with a custom keymap, starting in insert mode.
    pub fn with_keymap(
        keymap: Keymap<Action>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let editor = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, MAX_ROWS)
                .placeholder("Ask crest…")
        });
        let weak = cx.entity().downgrade();
        let intercept = cx.intercept_keystrokes(move |event, window, cx| {
            weak.update(cx, |this, cx| this.intercept(event, window, cx))
                .ok();
        });
        let focus = cx.subscribe_in(&editor, window, |this, _, event, window, cx| {
            // Clicking the editor in normal mode means "type here".
            if matches!(event, EditorEvent::Focus) && this.mode() != Mode::Insert {
                this.set_mode(Mode::Insert, window, cx);
            }
        });
        Self {
            sequencer: KeySequencer::new(keymap, Mode::Insert),
            editor,
            focus_handle: cx.focus_handle(),
            command: String::new(),
            timeout: None,
            queued: false,
            blink: Blink::default(),
            _blink: Self::blink_loop(window, cx),
            _subscriptions: vec![intercept, focus],
        }
    }

    /// Toggles the cursor every [`BLINK_INTERVAL`] while the window is
    /// active and the input has focus.
    fn blink_loop(window: &mut Window, cx: &mut Context<Self>) -> Task<()> {
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(BLINK_INTERVAL).await;
                let alive = this.update_in(cx, |this, window, cx| {
                    if !window.is_window_active() || !this.owns_focus(window, cx) {
                        return;
                    }
                    let now = cx.background_executor().now();
                    if this.blink.tick(now) {
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
    }

    /// Whether the blinking cursor is on.
    pub fn cursor_visible(&self) -> bool {
        self.blink.visible()
    }

    /// Current mode.
    pub fn mode(&self) -> Mode {
        self.sequencer.mode()
    }

    /// Text of the prompt editor.
    pub fn text(&self, cx: &App) -> String {
        self.editor.read(cx).value().to_string()
    }

    /// Text of the command line (without the colon).
    pub fn command_line(&self) -> &str {
        &self.command
    }

    /// The keymap in use.
    pub fn keymap(&self) -> &Keymap<Action> {
        self.sequencer.keymap()
    }

    /// The sequencer, e.g. to change `timeoutlen` or add mappings.
    pub fn sequencer_mut(&mut self) -> &mut KeySequencer<Action> {
        &mut self.sequencer
    }

    /// Gives keyboard focus to the part that matches the current mode.
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        if self.mode() == Mode::Insert {
            let handle = self.editor.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
        } else {
            window.focus(&self.focus_handle, cx);
        }
    }

    /// Inserts `text` at the cursor of the prompt editor, switching to
    /// insert mode (e.g. a `@path` reference chosen in a picker).
    pub fn insert_text(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.set_mode(Mode::Insert, window, cx);
        let text = text.to_owned();
        self.editor
            .update(cx, |editor, cx| editor.insert(text, window, cx));
    }

    /// Replaces the prompt with `text`, cursor at the end, in insert mode
    /// (a queued prompt taken back with `Up`).
    pub fn set_text(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.set_mode(Mode::Insert, window, cx);
        let text = text.to_owned();
        self.editor.update(cx, |editor, cx| {
            editor.set_value("", window, cx);
            editor.insert(text, window, cx);
        });
    }

    /// Tells the input whether prompts are queued: then `Up` in an empty
    /// prompt emits [`QUEUE_EDIT_ACTION`] instead of moving the cursor.
    pub fn set_queued(&mut self, queued: bool) {
        self.queued = queued;
    }

    /// Switches mode, moving focus and notifying listeners.
    pub fn set_mode(&mut self, mode: Mode, window: &mut Window, cx: &mut Context<Self>) {
        if mode == Mode::Command {
            self.command.clear();
        }
        let changed = mode != self.mode();
        self.sequencer.set_mode(mode);
        self.timeout = None;
        self.focus(window, cx);
        if changed {
            cx.emit(InputEvent::ModeChanged(mode));
        }
        cx.notify();
    }

    fn owns_focus(&self, window: &Window, cx: &App) -> bool {
        self.focus_handle.is_focused(window)
            || self.editor.read(cx).focus_handle(cx).is_focused(window)
    }

    fn intercept(&mut self, event: &KeystrokeEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.owns_focus(window, cx) {
            return;
        }
        let Some(key) = key_from_keystroke(&event.keystroke) else {
            return;
        };
        let now = cx.background_executor().now();
        self.blink.keyed(now);
        let feeds = self.sequencer.feed(key, now);
        if self.process(feeds, true, window, cx) {
            cx.stop_propagation();
        }
        self.schedule_timeout(window, cx);
        cx.notify();
    }

    /// Runs the sequencer's output. With `live_key`, the last key of a
    /// trailing no-match is the keystroke being dispatched: it is left to
    /// propagate (to the editor or GPUI bindings) when nothing here wants
    /// it. Returns whether the live keystroke was consumed.
    fn process(
        &mut self,
        feeds: Vec<Feed<Action>>,
        live_key: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let count = feeds.len();
        let mut consumed = true;
        for (i, feed) in feeds.into_iter().enumerate() {
            match feed {
                Feed::Matched(action) => self.run(action, window, cx),
                Feed::Pending => {}
                Feed::NoMatch(mut keys) => {
                    let live = (live_key && i + 1 == count).then(|| keys.pop()).flatten();
                    for key in &keys {
                        self.replay(key, window, cx);
                    }
                    if let Some(key) = live {
                        consumed = self.unmapped(&key, window, cx);
                    }
                }
            }
        }
        consumed
    }

    /// Handles a live key no mapping wants; returns whether it was consumed.
    fn unmapped(&mut self, key: &Key, window: &mut Window, cx: &mut Context<Self>) -> bool {
        match self.mode() {
            Mode::Insert
                if self.queued
                    && *key == Key::new(KeyCode::Up, Modifiers::default())
                    && self.text(cx).is_empty() =>
            {
                cx.emit(InputEvent::Action(Action::builtin(QUEUE_EDIT_ACTION)));
                true
            }
            // The editor (insert) or the app bindings (normal) take it.
            Mode::Insert | Mode::Normal => false,
            Mode::Command => {
                self.command_key(key, window, cx);
                true
            }
        }
    }

    /// Re-delivers a key of a prefix that did not match.
    fn replay(&mut self, key: &Key, window: &mut Window, cx: &mut Context<Self>) {
        match self.mode() {
            Mode::Insert => {
                if let Some(c) = key_text(key) {
                    self.editor
                        .update(cx, |editor, cx| editor.insert(c.to_string(), window, cx));
                }
            }
            Mode::Command => self.command_key(key, window, cx),
            Mode::Normal => {}
        }
    }

    fn command_key(&mut self, key: &Key, window: &mut Window, cx: &mut Context<Self>) {
        match key.code {
            KeyCode::Enter => {
                let command = std::mem::take(&mut self.command).trim().to_owned();
                self.set_mode(Mode::Normal, window, cx);
                if !command.is_empty() {
                    cx.emit(InputEvent::Command(command));
                }
            }
            KeyCode::Backspace => {
                if self.command.pop().is_none() {
                    self.set_mode(Mode::Normal, window, cx);
                }
            }
            _ => {
                if let Some(c) = key_text(key).filter(|c| *c != '\n') {
                    self.command.push(c);
                }
            }
        }
    }

    fn run(&mut self, action: Action, window: &mut Window, cx: &mut Context<Self>) {
        let name = match &action {
            Action::Builtin(name) => name.as_str(),
            Action::Callback(_) => "",
        };
        match name {
            "mode.normal" => self.set_mode(Mode::Normal, window, cx),
            "mode.insert" => self.set_mode(Mode::Insert, window, cx),
            "mode.command" => self.set_mode(Mode::Command, window, cx),
            "submit" => self.submit(window, cx),
            "newline" => self
                .editor
                .update(cx, |editor, cx| editor.insert("\n", window, cx)),
            _ => cx.emit(InputEvent::Action(action)),
        }
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.text(cx);
        if text.trim().is_empty() {
            return;
        }
        self.editor
            .update(cx, |editor, cx| editor.set_value("", window, cx));
        cx.emit(InputEvent::Submit(text));
    }

    fn schedule_timeout(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(deadline) = self.sequencer.deadline() else {
            self.timeout = None;
            return;
        };
        let wait = deadline.saturating_duration_since(cx.background_executor().now());
        self.timeout = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(wait).await;
            this.update_in(cx, |this, window, cx| {
                let now = cx.background_executor().now();
                let feeds = this.sequencer.tick(now);
                this.process(feeds, false, window, cx);
                this.schedule_timeout(window, cx);
                cx.notify();
            })
            .ok();
        }));
    }

    fn pending_keys(&self) -> String {
        self.sequencer
            .pending()
            .iter()
            .map(ToString::to_string)
            .collect()
    }
}

impl Focusable for InputView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for InputView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let mode = self.mode();
        let mode_color = match mode {
            Mode::Insert => theme.success,
            Mode::Normal => theme.primary,
            Mode::Command => theme.warning,
        };
        let (block, inverted, muted) = (theme.foreground, theme.background, theme.muted_foreground);
        let blink_on = self.blink.visible();
        let shape = match mode {
            Mode::Insert => blink_on.then_some(CursorShape::Block),
            Mode::Normal => Some(CursorShape::Hollow),
            Mode::Command => None,
        };
        let editor = self.editor.clone();
        // Painted after the textarea, from the caret it laid out this frame.
        let cursor = canvas(
            |_, _, _| {},
            move |_, (), window, cx| {
                let focused = editor.read(cx).focus_handle(cx).is_focused(window);
                if let Some(shape) = shape.filter(|s| *s == CursorShape::Hollow || focused) {
                    paint_cursor(&editor, shape, block, inverted, window, cx);
                }
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();
        let prompt = h_flex()
            .items_start()
            .child(
                div()
                    .pt_2()
                    .text_color(mode_color)
                    .font_weight(gpui::FontWeight::BOLD)
                    .child("❯"),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .child(HiddenCaret::new(
                        Textarea::new(&self.editor).appearance(false),
                        self.editor.clone(),
                    ))
                    .child(cursor),
            );
        let command_line = (mode == Mode::Command).then(|| {
            let mut caret = div().child(" ");
            if blink_on {
                caret = caret.bg(block).text_color(inverted);
            }
            h_flex()
                .flex_1()
                .child(SharedString::from(format!(":{}", self.command)))
                .child(caret)
        });
        let status = h_flex()
            .gap_2()
            .text_xs()
            .text_color(muted)
            .children(command_line)
            .child(
                div()
                    .flex_1()
                    .child(SharedString::from(self.pending_keys())),
            )
            .child(
                div()
                    .id("input-mode")
                    .text_color(mode_color)
                    .child(SharedString::from(format!("-- {} --", mode_label(mode)))),
            );
        v_flex()
            .id("crest-input")
            .key_context("CrestInput")
            .track_focus(&self.focus_handle)
            .font_family(theme.mono_font_family.clone())
            .child(prompt)
            .child(status)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Keystroke, TestAppContext, VisualTestContext};
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::time::Duration;

    type Events = Rc<RefCell<Vec<InputEvent>>>;

    fn input(cx: &mut TestAppContext) -> (Entity<InputView>, &mut VisualTestContext, Events) {
        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(InputView::new);
        let events = Events::default();
        let recorded = events.clone();
        cx.update(|window, cx| {
            cx.subscribe(&view, move |_, event: &InputEvent, _| {
                recorded.borrow_mut().push(event.clone());
            })
            .detach();
            view.update(cx, |view, cx| view.focus(window, cx));
        });
        cx.run_until_parked();
        (view, cx, events)
    }

    /// Recorded events other than mode changes, draining them.
    fn take(events: &Events) -> Vec<InputEvent> {
        events
            .borrow_mut()
            .drain(..)
            .filter(|event| !matches!(event, InputEvent::ModeChanged(_)))
            .collect()
    }

    fn action(name: &str) -> InputEvent {
        InputEvent::Action(Action::builtin(name))
    }

    #[gpui::test]
    fn insert_mode_types_and_submits(cx: &mut TestAppContext) {
        let (view, cx, events) = input(cx);
        cx.simulate_input("hola");
        cx.simulate_keystrokes("shift-enter");
        cx.simulate_input("mundo");
        view.read_with(cx, |view, cx| assert_eq!(view.text(cx), "hola\nmundo"));
        cx.simulate_keystrokes("enter");
        assert_eq!(take(&events), [InputEvent::Submit("hola\nmundo".into())]);
        view.read_with(cx, |view, cx| {
            assert_eq!(view.text(cx), "");
            assert_eq!(view.mode(), Mode::Insert);
        });
    }

    #[gpui::test]
    fn empty_prompt_is_not_submitted(cx: &mut TestAppContext) {
        let (_view, cx, events) = input(cx);
        cx.simulate_keystrokes("space enter");
        assert!(take(&events).is_empty());
    }

    #[gpui::test]
    fn normal_mode_dispatches_timeline_actions(cx: &mut TestAppContext) {
        let (view, cx, events) = input(cx);
        cx.simulate_keystrokes("escape");
        assert_eq!(
            events.borrow_mut().drain(..).collect::<Vec<_>>(),
            [InputEvent::ModeChanged(Mode::Normal)]
        );
        cx.simulate_keystrokes("j k shift-g enter t y");
        assert_eq!(
            take(&events),
            [
                action("block.next"),
                action("block.prev"),
                action("block.last"),
                action("block.toggle"),
                action("block.toggle_view"),
                action("block.yank"),
            ]
        );
        view.read_with(cx, |view, cx| {
            assert_eq!(view.mode(), Mode::Normal);
            assert_eq!(view.text(cx), "", "normal-mode keys never reach the editor");
        });
    }

    #[gpui::test]
    fn gg_waits_for_the_second_key(cx: &mut TestAppContext) {
        let (_view, cx, events) = input(cx);
        cx.simulate_keystrokes("escape g");
        assert!(take(&events).is_empty(), "g alone is a pending prefix");
        cx.simulate_keystrokes("g");
        assert_eq!(take(&events), [action("block.first")]);
    }

    #[gpui::test]
    fn pending_prefix_times_out(cx: &mut TestAppContext) {
        let (view, cx, events) = input(cx);
        cx.simulate_keystrokes("escape g");
        view.read_with(cx, |view, _| assert_eq!(view.pending_keys(), "g"));
        cx.executor().advance_clock(Duration::from_millis(1100));
        cx.run_until_parked();
        view.read_with(cx, |view, _| assert_eq!(view.pending_keys(), ""));
        cx.simulate_keystrokes("g");
        assert!(take(&events).is_empty(), "the timed-out g does not pair up");
    }

    #[gpui::test]
    fn up_takes_back_a_queued_prompt_only_from_an_empty_prompt(cx: &mut TestAppContext) {
        let (view, cx, events) = input(cx);
        cx.simulate_keystrokes("up");
        assert!(take(&events).is_empty(), "nothing is queued");
        view.update(cx, |view, _| view.set_queued(true));
        cx.simulate_input("x");
        cx.simulate_keystrokes("up");
        assert!(take(&events).is_empty(), "the prompt has text");
        cx.simulate_keystrokes("backspace up");
        assert_eq!(take(&events), [action(QUEUE_EDIT_ACTION)]);
        cx.update(|window, cx| {
            view.update(cx, |view, cx| view.set_text("de vuelta", window, cx));
        });
        view.read_with(cx, |view, cx| assert_eq!(view.text(cx), "de vuelta"));
    }

    #[gpui::test]
    fn the_cursor_blinks_and_stays_on_while_typing(cx: &mut TestAppContext) {
        let (view, cx, _events) = input(cx);
        let visible = |cx: &mut VisualTestContext| view.read_with(cx, |v, _| v.cursor_visible());
        cx.update(|window, _| window.activate_window());
        cx.run_until_parked();
        assert!(visible(cx));
        cx.executor().advance_clock(BLINK_INTERVAL);
        cx.run_until_parked();
        assert!(!visible(cx), "blinked off");
        cx.simulate_input("a");
        assert!(visible(cx), "a key turns it on");
        cx.executor().advance_clock(BLINK_INTERVAL / 2);
        cx.run_until_parked();
        assert!(visible(cx), "still typing");
        cx.executor().advance_clock(BLINK_INTERVAL / 2);
        cx.run_until_parked();
        assert!(!visible(cx), "blinks again a whole interval after the key");
    }

    #[gpui::test]
    fn i_returns_to_insert(cx: &mut TestAppContext) {
        let (view, cx, _events) = input(cx);
        cx.simulate_keystrokes("escape i");
        cx.simulate_input("x");
        view.read_with(cx, |view, cx| {
            assert_eq!(view.mode(), Mode::Insert);
            assert_eq!(view.text(cx), "x");
        });
    }

    #[gpui::test]
    fn command_mode_runs_the_typed_command(cx: &mut TestAppContext) {
        let (view, cx, events) = input(cx);
        cx.simulate_keystrokes("escape :");
        view.read_with(cx, |view, _| assert_eq!(view.mode(), Mode::Command));
        cx.simulate_input("model opus");
        cx.simulate_keystrokes("backspace s");
        view.read_with(cx, |view, _| assert_eq!(view.command_line(), "model opus"));
        cx.simulate_keystrokes("enter");
        assert_eq!(take(&events), [InputEvent::Command("model opus".into())]);
        view.read_with(cx, |view, cx| {
            assert_eq!(view.mode(), Mode::Normal);
            assert_eq!(view.text(cx), "", "command text never reaches the editor");
        });
    }

    #[gpui::test]
    fn escape_cancels_command_mode(cx: &mut TestAppContext) {
        let (view, cx, events) = input(cx);
        cx.simulate_keystrokes("escape : q escape");
        assert!(take(&events).is_empty());
        view.read_with(cx, |view, _| assert_eq!(view.mode(), Mode::Normal));
    }

    #[gpui::test]
    fn mode_changes_are_emitted(cx: &mut TestAppContext) {
        let (_view, cx, events) = input(cx);
        cx.simulate_keystrokes("escape : escape i");
        let modes: Vec<_> = events
            .borrow()
            .iter()
            .filter_map(|event| match event {
                InputEvent::ModeChanged(mode) => Some(*mode),
                _ => None,
            })
            .collect();
        assert_eq!(
            modes,
            [Mode::Normal, Mode::Command, Mode::Normal, Mode::Insert]
        );
    }

    fn key(notation: &str) -> Option<Key> {
        let keystroke = Keystroke::parse(notation)
            .unwrap_or_else(|err| panic!("{err}"))
            .with_simulated_ime();
        key_from_keystroke(&keystroke)
    }

    fn parsed(notation: &str) -> Option<Key> {
        application::keymap::parse_keys(notation, " ")
            .ok()
            .and_then(|keys| keys.first().copied())
    }

    #[test]
    fn printable_keys_use_the_typed_character() {
        assert_eq!(key("g"), parsed("g"));
        assert_eq!(key("shift-g"), parsed("G"));
        assert_eq!(key(":"), parsed(":"));
        assert_eq!(key("space"), parsed("<Space>"));
    }

    #[test]
    fn named_and_chorded_keys_keep_modifiers() {
        assert_eq!(key("enter"), parsed("<CR>"));
        assert_eq!(key("shift-enter"), parsed("<S-CR>"));
        assert_eq!(key("escape"), parsed("<Esc>"));
        assert_eq!(key("ctrl-c"), parsed("<C-c>"));
        assert_eq!(key("f5"), parsed("<F5>"));
    }

    #[test]
    fn bare_modifiers_are_not_keys() {
        assert_eq!(key("shift"), None);
    }
}
