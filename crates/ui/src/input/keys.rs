//! Keys: decoding keystrokes, the keymap's sequences and what each action
//! does to the prompt.

use application_core::keymap::{Action, Feed, Key, KeyCode, Modifiers};
use gpui::{App, ClipboardItem, Context, KeystrokeEvent, Window};

use super::mode::Press;
use super::paste::PasteStep;
use super::{
    CANCEL_ACTION, ChatMode, FOCUS_INPUT_ACTION, IMAGE_MODE_ACTION, InputEvent, InputView, Line,
    PROMPT_MODE_ACTION, QUEUE_EDIT_ACTION, classify_line,
};
use crate::session::INTERRUPT_ACTION;

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
        "insert" => Some(KeyCode::Insert),
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

impl InputView {
    pub(super) fn intercept(
        &mut self,
        event: &KeystrokeEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // An open approval dialog owns the keyboard, including the reason
        // line. Those keys must not land in the prompt behind it.
        if crate::approval::dialog_is_open(cx) {
            return;
        }
        if !self.owns_focus(window, cx) {
            return;
        }
        let Some(key) = key_from_keystroke(&event.keystroke) else {
            return;
        };
        let now = cx.background_executor().now();
        self.blink.keyed(now);
        let press = self.mode_control.press(&key);
        if let Press::Pick(entry) = press {
            self.pick_entry(entry, window, cx);
        }
        if press != Press::Pass || self.palette_key(&key, window, cx) {
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if self.feed(key, true, window, cx) {
            cx.stop_propagation();
        }
    }

    /// Feeds a key typed while another panel has focus, so the shortcuts
    /// work everywhere. Only chords, `Esc`, function keys and the keys
    /// after a pending prefix count: plain keys (`Enter`, the arrows) are
    /// the panel's. Returns whether a mapping took it (or it is part of a
    /// pending prefix); an unmatched key is left to the caller.
    pub fn feed_key(&mut self, key: Key, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let chord = key.mods.ctrl || key.mods.alt || key.mods.cmd;
        let global = chord || matches!(key.code, KeyCode::Esc | KeyCode::F(_));
        if !global && self.sequencer.pending().is_empty() {
            return false;
        }
        self.feed(key, false, window, cx)
    }

    fn feed(&mut self, key: Key, own: bool, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let now = cx.background_executor().now();
        let feeds = self.sequencer.feed(key, now);
        let consumed = self.process(feeds, Some(own), window, cx);
        self.schedule_timeout(window, cx);
        cx.notify();
        consumed
    }

    /// Runs the sequencer's output. `live` is `Some(own)` when the last key
    /// of a trailing no-match is the keystroke being dispatched (`own` when
    /// it reached the editor): it is left to propagate when nothing here
    /// wants it. Returns whether the live keystroke was consumed.
    fn process(
        &mut self,
        feeds: Vec<Feed<Action>>,
        live: Option<bool>,
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
                    let last = live.filter(|_| i + 1 == count);
                    let key = last.and_then(|_| keys.pop());
                    for key in &keys {
                        self.replay(key, window, cx);
                    }
                    if let (Some(key), Some(own)) = (key, last) {
                        consumed = own && self.unmapped(&key, window, cx);
                    }
                }
            }
        }
        consumed
    }

    /// Handles a live key no mapping wants; returns whether it was consumed.
    fn unmapped(&mut self, key: &Key, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.queued && *key == Key::plain(KeyCode::Up) && self.text(cx).is_empty() {
            cx.emit(InputEvent::Action(Action::builtin(QUEUE_EDIT_ACTION)));
            return true;
        }
        if self.browse(key, window, cx) {
            return true;
        }
        if *key == Key::plain(KeyCode::Backspace)
            && !self.tray.is_empty()
            && self.text(cx).is_empty()
        {
            self.remove_image(self.tray.images().len() - 1, cx);
            return true;
        }
        if *key == Key::plain(KeyCode::Backspace)
            && self.chat_mode() != ChatMode::Chat
            && self.text(cx).is_empty()
        {
            self.set_chat_mode(ChatMode::Chat, window, cx);
            return true;
        }
        // The editor takes it.
        false
    }

    /// `Up` on the first line and `Down` on the last bring back the lines
    /// of the history; returns whether one was brought back.
    fn browse(&mut self, key: &Key, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let text = self.text(cx);
        let cursor = self.editor.read(cx).cursor().min(text.len());
        let (before, after) = text.split_at(cursor);
        let line = if *key == Key::plain(KeyCode::Up) && !before.contains('\n') {
            self.history.older(&text).map(str::to_owned)
        } else if *key == Key::plain(KeyCode::Down) && !after.contains('\n') {
            self.history.newer()
        } else {
            None
        };
        let Some(line) = line else {
            return false;
        };
        self.set_text(&line, window, cx);
        true
    }

    /// Adds a sent line to the history.
    pub(super) fn record(&mut self, line: &str, cx: &mut Context<Self>) {
        if self.history.push(line) {
            cx.emit(InputEvent::Recorded(line.to_owned()));
        }
    }

    /// Re-delivers a key of a prefix that did not match.
    fn replay(&mut self, key: &Key, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(c) = key_text(key) {
            self.editor
                .update(cx, |editor, cx| editor.insert(c.to_string(), window, cx));
        }
    }

    fn run(&mut self, action: Action, window: &mut Window, cx: &mut Context<Self>) {
        let name = match &action {
            Action::Builtin(name) => name.as_str(),
            Action::Callback(_) => "",
        };
        match name {
            "submit" => self.submit(window, cx),
            "newline" => self
                .editor
                .update(cx, |editor, cx| editor.insert("\n", window, cx)),
            FOCUS_INPUT_ACTION if self.owns_focus(window, cx) => self.palette_hidden = true,
            IMAGE_MODE_ACTION => self.toggle_image_mode(cx),
            PROMPT_MODE_ACTION => self.cycle_mode(window, cx),
            CANCEL_ACTION => self.cancel(window, cx),
            _ => cx.emit(InputEvent::Action(action)),
        }
    }

    /// Copies the text selected in the prompt. Returns whether there was any.
    pub fn copy_selection(&self, cx: &App) -> bool {
        let text = self.text(cx);
        let selected = self.editor.read(cx).selected_range();
        match text.get(selected).filter(|s| !s.is_empty()) {
            Some(selection) => {
                cx.write_to_clipboard(ClipboardItem::new_string(selection.to_owned()));
                true
            }
            None => false,
        }
    }

    /// `Ctrl+C` in the prompt, as in a terminal: copies the selection, else
    /// clears the prompt into the history (not saved with the session) so
    /// `Up` brings it back, else asks to interrupt the turn. The images in
    /// the tray stay.
    fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.copy_selection(cx) {
            return;
        }
        let text = self.text(cx);
        if text.is_empty() {
            cx.emit(InputEvent::Action(Action::builtin(INTERRUPT_ACTION)));
            return;
        }
        self.history.push(&text);
        self.editor
            .update(cx, |editor, cx| editor.set_value("", window, cx));
        cx.emit(InputEvent::Cleared);
    }

    pub(super) fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let line = classify_line(&self.mode_control.mode.command_for(&self.text(cx)));
        // A command leaves the images in the tray for the next prompt.
        let text = match line {
            Some(Line::Command(command)) => {
                self.clear_sent(window, cx);
                cx.emit(InputEvent::Command(command));
                return;
            }
            Some(Line::Prompt(text)) => text,
            None => String::new(),
        };
        if self.pastes_pending() {
            // Images pasted just before `Enter` are still being prepared:
            // the prompt leaves the editor now and is sent with them when
            // they land.
            self.clear_sent(window, cx);
            self.after_pastes(PasteStep::Submit(text), cx);
            return;
        }
        let Some(event) = self.prompt_event(text) else {
            return;
        };
        self.clear_sent(window, cx);
        cx.emit(event);
    }

    /// What sending the prompt `text` (maybe empty) emits with the images
    /// in the tray, which it takes; `None` when there is nothing to send.
    pub(super) fn prompt_event(&mut self, text: String) -> Option<InputEvent> {
        if !self.tray.is_empty() {
            let (images, mode) = self.tray.take();
            return Some(InputEvent::SubmitImages { text, images, mode });
        }
        (!text.is_empty()).then_some(InputEvent::Submit(text))
    }

    /// Records the sent prompt in the history and empties the editor.
    fn clear_sent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let sent = self.text(cx);
        self.record(&sent, cx);
        self.editor
            .update(cx, |editor, cx| editor.set_value("", window, cx));
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
                this.process(feeds, None, window, cx);
                this.schedule_timeout(window, cx);
                cx.notify();
            })
            .ok();
        }));
    }

    pub(super) fn pending_keys(&self) -> String {
        self.sequencer
            .pending()
            .iter()
            .map(ToString::to_string)
            .collect()
    }
}
