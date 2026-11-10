//! Keys outside the prompt, the focus cycle and the events of the input.

use application_core::keymap::{Action, Key, KeyCode};
use application_core::ports::events::NoticeLevel;
use gpui::{App, Context, Entity, Focusable as _, KeystrokeEvent, Subscription, Window};

use super::{
    FOCUS_NEXT_ACTION, FOCUS_WIDGET_ACTION, FULLSCREEN_ACTION, INTERRUPT_ACTION, MODE_ACTION,
    MODEL_ACTION, OUTLINE_ACTION, PERMISSION_ACTION, PERMISSIONS_ACTION, PROJECT_ACTION,
    QUIT_ACTION, Session, SessionEvent, THEME_ACTION,
};
use crate::help::HELP_ACTION;
use crate::input::attachments::ImageMode;
use crate::input::key_from_keystroke;
use crate::input::{FOCUS_INPUT_ACTION, InputEvent, InputView, QUEUE_EDIT_ACTION};
use crate::timeline::DeferredKey;

/// Text a key typed outside the prompt puts in it: a printable character
/// without Ctrl, Alt or Cmd.
fn typed_text(key: &Key) -> Option<String> {
    if key.mods.ctrl || key.mods.alt || key.mods.cmd {
        return None;
    }
    match key.code {
        KeyCode::Char(c) => Some(c.to_string()),
        _ => None,
    }
}

impl Session {
    /// Keys typed while the focus is outside the prompt. In an overlay
    /// (picker, dialog, menu, modal) only `Ctrl+C` is taken. With the focus
    /// in the timeline, `Esc` and printable keys go first to the focused
    /// element (a block widget, the outline menu) and fall back to the
    /// prompt only when nothing takes them ([`DeferredKey`]). Elsewhere
    /// (the tree) the keys go through the input's keymap, any other
    /// printable key is typed into the prompt and the rest is left to the
    /// panel.
    pub(super) fn intercept_keys(cx: &mut Context<Self>) -> Subscription {
        let weak = cx.entity().downgrade();
        cx.intercept_keystrokes(move |event: &KeystrokeEvent, window, cx| {
            weak.update(cx, |this, cx| this.outside_key(event, window, cx))
                .ok();
        })
    }

    /// `Ctrl+C` with no other modifier (Shift makes it `Ctrl+Shift+C`).
    fn is_ctrl_c(key: &Key) -> bool {
        key.code == KeyCode::Char('c')
            && key.mods.ctrl
            && !key.mods.alt
            && !key.mods.cmd
            && !key.mods.shift
    }

    fn outside_key(&mut self, event: &KeystrokeEvent, window: &mut Window, cx: &mut Context<Self>) {
        // A parked session has no keyboard.
        if !self.shown {
            return;
        }
        // A key deferred before was taken by an element of the timeline.
        if self.timeline.read(cx).has_deferred_key() {
            self.timeline
                .update(cx, |timeline, _| timeline.clear_deferred_key());
        }
        // The dialog may be up while the prompt still has focus (Reject does
        // not move it). Take the keys here, before the prompt sees them.
        if self.approval.read(cx).is_open() {
            let handled = self.approval.update(cx, |dialog, cx| {
                dialog.handle_keystroke(&event.keystroke, window, cx)
            });
            let key = key_from_keystroke(&event.keystroke);
            if !handled && key.is_some_and(|key| Self::is_ctrl_c(&key)) {
                self.interrupt(cx);
            } else if !handled
                && let Some(action) = key.and_then(|key| self.app_wide_action(&key, cx))
            {
                self.action(&action, window, cx);
            }
            cx.stop_propagation();
            return;
        }
        // The tour covers the window. Escape closes it; nothing else reaches
        // the prompt, so a key cannot send a message while it is open.
        if self.tour.is_some() {
            if event.keystroke.key == "escape" {
                self.tour_act(crate::tour::Action::Exit, window, cx);
            }
            cx.stop_propagation();
            return;
        }
        if self.input.read(cx).owns_focus(window, cx) {
            return;
        }
        // The chat rename and search fields are text inputs outside the
        // prompt. Their keys stay there; a printable key must not fall into
        // the prompt.
        if crate::chats::field_has_focus(window, cx) {
            return;
        }
        let Some(key) = key_from_keystroke(&event.keystroke) else {
            return;
        };
        // The chats list takes its own keys (arrows, Enter, F2, `r`,
        // Delete); chords, `Esc` and function keys still go through the
        // keymap, and nothing it leaves is typed into the prompt.
        if crate::chats::list_has_focus(window, cx) {
            if self
                .input
                .update(cx, |input, cx| input.feed_key(key, window, cx))
            {
                cx.stop_propagation();
            }
            return;
        }
        if self.overlay_focused(window, cx) {
            let m = key.mods;
            if Self::is_ctrl_c(&key) {
                // Text selected in the picker's query is copied, and in the
                // (masked) key field left alone: copying never interrupts
                // the turn. Without a selection Ctrl+C interrupts, as
                // everywhere.
                let picked = self.picker.read(cx).selected_text(cx);
                let copied = picked.is_some() || self.login.read(cx).has_selection(cx);
                if let Some(text) = picked {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
                }
                if copied {
                    cx.stop_propagation();
                } else {
                    self.interrupt(cx);
                }
                return;
            }
            // Quitting and full screen work over any overlay.
            if let Some(action) = self.app_wide_action(&key, cx) {
                self.action(&action, window, cx);
                cx.stop_propagation();
                return;
            }
            // A waiting form keeps what is typed in it; a chord (`Ctrl+Tab`
            // to the prompt) still goes through the keymap, so the user can
            // leave the form and answer in the chat without the mouse. In
            // the picker too, minus its own `Ctrl+P`/`Ctrl+N` and `Alt+…`
            // toggles, and with the
            // keys after the leader: the shortcut that opened it closes it.
            let chord = m.ctrl || m.alt || m.cmd;
            let picker_keys = self.picker.read(cx).is_open()
                && !self.login.read(cx).is_open()
                && !self.modal.read(cx).is_open();
            let picker_own =
                (m.ctrl && !m.alt && !m.cmd && matches!(key.code, KeyCode::Char('p' | 'n')))
                    || (m.alt
                        && !m.ctrl
                        && !m.cmd
                        && crate::picker::PickerView::owns_alt_key(&key.code));
            let to_keymap = if crate::blocks::form::owns_focus(window, cx) {
                chord
            } else if picker_keys {
                (chord && !picker_own) || self.input.read(cx).has_pending_keys()
            } else {
                false
            };
            if to_keymap
                && self
                    .input
                    .update(cx, |input, cx| input.feed_key(key, window, cx))
            {
                cx.stop_propagation();
            }
            return;
        }
        // Ctrl+C with the chat focused and text selected in it copies, as
        // in Windows Terminal; without a selection it keeps its meaning.
        if Self::is_ctrl_c(&key)
            && self.timeline.focus_handle(cx).contains_focused(window, cx)
            && self
                .timeline
                .update(cx, |timeline, cx| timeline.copy_selection(window, cx))
        {
            cx.stop_propagation();
            return;
        }
        if self.defer_to_timeline(key, window, cx) {
            return;
        }
        let consumed = self
            .input
            .update(cx, |input, cx| input.feed_key(key, window, cx));
        if consumed {
            cx.stop_propagation();
            return;
        }
        if let Some(text) = typed_text(&key) {
            self.input
                .update(cx, |input, cx| input.insert_text(&text, window, cx));
            cx.stop_propagation();
        }
    }

    /// With the focus in the timeline, lets a plain `Esc` or printable key
    /// through to the focused element (it may close a menu, clear a
    /// cursor, zoom). The timeline runs [`Self::fallback_key`] if the key
    /// reaches it unhandled. A key after a pending prefix (the leader) is
    /// the keymap's. Returns whether the key was deferred.
    fn defer_to_timeline(&mut self, key: Key, window: &Window, cx: &mut Context<Self>) -> bool {
        let plain_esc = key.code == KeyCode::Esc
            && !(key.mods.ctrl || key.mods.alt || key.mods.cmd || key.mods.shift);
        if !(plain_esc || typed_text(&key).is_some())
            || self.input.read(cx).has_pending_keys()
            || !self.timeline.focus_handle(cx).contains_focused(window, cx)
        {
            return false;
        }
        let session = cx.entity().downgrade();
        let deferred = DeferredKey::new(key, move |window, cx| {
            session
                .update(cx, |this, cx| this.fallback_key(key, window, cx))
                .ok();
        });
        self.timeline
            .update(cx, |timeline, _| timeline.defer_key(deferred));
        true
    }

    /// A deferred key nothing in the timeline took: it goes through the
    /// keymap (`Esc` focuses the prompt) or is typed into the prompt.
    fn fallback_key(&mut self, key: Key, window: &mut Window, cx: &mut Context<Self>) {
        let consumed = self
            .input
            .update(cx, |input, cx| input.feed_key(key, window, cx));
        if !consumed && let Some(text) = typed_text(&key) {
            self.input
                .update(cx, |input, cx| input.insert_text(&text, window, cx));
        }
    }

    /// The action of `key` alone when it quits or switches full screen.
    fn app_wide_action(&self, key: &Key, cx: &App) -> Option<Action> {
        let input = self.input.read(cx);
        if input.has_pending_keys() {
            return None;
        }
        match input.keymap().lookup(std::slice::from_ref(key)).exact {
            Some(Action::Builtin(name)) if name == QUIT_ACTION || name == FULLSCREEN_ACTION => {
                Some(Action::Builtin(name.clone()))
            }
            _ => None,
        }
    }

    /// Whether a picker, the approval dialog, the information panel, a modal, the
    /// key dialog or a
    /// waiting form (or a widget's text input) has the focus. Keys typed there never reach the
    /// prompt (the API key must not land in it).
    fn overlay_focused(&self, window: &Window, cx: &App) -> bool {
        self.login.read(cx).is_open()
            || self.picker.read(cx).is_open()
            || self.approval.focus_handle(cx).contains_focused(window, cx)
            || self.status.read(cx).info_focused(window, cx)
            || self.modal.read(cx).is_open()
            || crate::blocks::form::owns_focus(window, cx)
            || crate::widgets::owns_focus(window, cx)
    }

    /// `<C-Tab>`: prompt → timeline → tree (when shown) → chats (when
    /// shown) → prompt.
    pub fn focus_next(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let timeline = self.timeline.focus_handle(cx);
        let tree_shown = self.tree.read(cx).is_visible();
        let in_timeline = timeline.contains_focused(window, cx);
        let in_tree = self.tree.focus_handle(cx).contains_focused(window, cx);
        if self.input.read(cx).owns_focus(window, cx) {
            window.focus(&timeline, cx);
            self.timeline
                .update(cx, |view, cx| view.select_last_if_none(cx));
        } else if in_timeline && tree_shown {
            self.tree.update(cx, |tree, cx| tree.focus(window, cx));
        } else if (in_timeline || in_tree) && self.sidebar != crate::chats::SidebarWidth::Strip {
            cx.emit(SessionEvent::Chats(crate::chats::ChatsCommand::Focus));
        } else {
            self.focus_input(window, cx);
        }
        cx.notify();
    }

    pub(super) fn on_input(
        &mut self,
        _input: &Entity<InputView>,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            // A form waiting for an answer takes the prompt as its answer.
            InputEvent::Submit(prompt) => match self.waiting_form(cx) {
                Some(form) => {
                    form.update(cx, |form, cx| form.reply(prompt.clone(), window, cx));
                }
                None => self.submit(prompt.clone(), cx),
            },
            InputEvent::SubmitImages {
                text,
                images,
                mode: ImageMode::Model,
            } => {
                if self.waiting_form(cx).is_some() {
                    let message = "a form is waiting for an answer; the images go to the next turn";
                    self.notice(NoticeLevel::Info, message.to_owned(), cx);
                }
                self.submit_with(text.clone(), None, images.clone(), cx);
            }
            InputEvent::SubmitImages {
                text,
                images,
                mode: ImageMode::ChatOnly,
            } => self.post_images(text.clone(), images.clone(), window, cx),
            InputEvent::Notice(message) => self.notice(NoticeLevel::Warning, message.clone(), cx),
            InputEvent::Command(command) => self.command(command, window, cx),
            InputEvent::Action(action) => self.action(action, window, cx),
            InputEvent::Recorded(line) => self.controller.record_prompt(line.clone()),
            InputEvent::ModelsWanted => self.load_catalog(window, cx),
            InputEvent::ThemesWanted => self.load_themes(window, cx),
            InputEvent::ThemePreview(name) => {
                if let Some(choice) = self.theme_choices.iter().find(|c| &c.name == name) {
                    crate::app::set_theme(&choice.theme, cx);
                }
            }
            InputEvent::ThemePreviewEnd => crate::app::set_theme(&self.theme, cx),
            InputEvent::Cleared => {
                self.notice(NoticeLevel::Info, "prompt cleared · ↑ restores", cx)
            }
        }
    }

    pub(super) fn action(&mut self, action: &Action, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .tree
            .update(cx, |tree, cx| tree.handle_action(action, window, cx))
        {
            // A hidden tree cannot keep the focus.
            if !self.tree.read(cx).is_visible() {
                self.focus_input(window, cx);
            }
            return;
        }
        if self
            .timeline
            .update(cx, |view, cx| view.handle_action(action, cx))
            || self
                .picker
                .update(cx, |picker, cx| picker.handle_action(action, window, cx))
        {
            return;
        }
        let name = match action {
            Action::Builtin(name) => name.as_str(),
            Action::Callback(id) => {
                let reply = self.controller.run_callback(*id, serde_json::Value::Null);
                self.await_reply(reply, 0, window, cx);
                return;
            }
        };
        match name {
            INTERRUPT_ACTION => {
                if !self.interrupt(cx) {
                    self.notice(NoticeLevel::Info, "nothing to interrupt", cx);
                }
            }
            // Copies from anywhere and never interrupts: the prompt's
            // selection first, else the chat's.
            "selection.copy" => {
                let copied = self.input.read(cx).copy_selection(cx)
                    || self
                        .timeline
                        .update(cx, |timeline, cx| timeline.copy_selection(window, cx));
                if !copied {
                    self.notice(NoticeLevel::Info, "nothing selected to copy", cx);
                }
            }
            PERMISSION_ACTION => self.cycle_permission_mode(cx),
            QUEUE_EDIT_ACTION => self.edit_queued(window, cx),
            FOCUS_INPUT_ACTION => self.focus_input(window, cx),
            FOCUS_NEXT_ACTION => self.focus_next(window, cx),
            MODEL_ACTION => self.open_model_popup(window, cx),
            HELP_ACTION => self.open_help("", window, cx),
            PERMISSIONS_ACTION => self.open_permissions(window, cx),
            QUIT_ACTION => self.quit(window, cx),
            FULLSCREEN_ACTION => window.toggle_fullscreen(),
            THEME_ACTION => self.open_theme_popup(window, cx),
            MODE_ACTION => self.open_mode_popup(window, cx),
            PROJECT_ACTION => self.switch_project(window, cx),
            OUTLINE_ACTION => self
                .timeline
                .update(cx, |timeline, cx| timeline.toggle_outline(window, cx)),
            FOCUS_WIDGET_ACTION => {
                let focused = self
                    .timeline
                    .update(cx, |timeline, cx| timeline.focus_widget(window, cx));
                if !focused {
                    self.notice(
                        NoticeLevel::Info,
                        "the selected block has no keys of its own",
                        cx,
                    );
                }
            }
            name if crate::chats::ChatsCommand::for_action(name).is_some() => {
                if let Some(command) = crate::chats::ChatsCommand::for_action(name) {
                    cx.emit(SessionEvent::Chats(command));
                }
            }
            _ => self.notice(
                NoticeLevel::Info,
                format!("the action {action:?} does nothing yet"),
                cx,
            ),
        }
    }
}
