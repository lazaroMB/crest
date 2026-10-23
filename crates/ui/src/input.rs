//! The prompt input: a multi-line editor (gpui-component `Textarea`) that
//! is always ready to type, plus the Neovim-style [`KeySequencer`] for the
//! shortcuts (chords such as `<C-c>` or `<A-Up>`, and the leader chord,
//! `<C-g>` by default, followed by a key: `<C-g>e`).
//!
//! - Keys no mapping wants go to the editor untouched; `<CR>` submits and
//!   `<S-CR>` inserts a newline.
//! - A line starting with `/` is a command ([`InputEvent::Command`], without
//!   the slash): `/model opus`, `/research …`, `/!ls -la`. `//text` sends
//!   `/text` as a prompt.
//! - While the line is `/` plus a partial command name, a palette above the
//!   prompt lists the matching commands: `Up`/`Down` choose, `Tab`
//!   completes and `Enter` runs the chosen one (or completes it up to the
//!   argument it needs); `Esc` hides it.
//! - While the last word starts with `@`, the same popup lists the entries
//!   of the workspace directory it names ([`paths`]): `Tab` completes a
//!   directory (`@src/`, whose entries are listed next) or a file, and
//!   `Enter` completes the chosen entry, or submits once the word already
//!   is it.
//! - Pasting an image (`Ctrl+V` with a screenshot or a copied picture)
//!   puts it in a tray above the prompt ([`attachments`]), with the text
//!   kept. A chip in the tray says what `Enter` does with the images:
//!   **→ Model** sends them with the prompt, **Chat only** adds them to the
//!   chat without calling the model; a click or `<leader>i` switches it. `×`,
//!   or `Backspace` in an empty prompt, removes an image.
//! - `Up` on the first line of the prompt brings back the line sent before
//!   ([`history`]), and `Down` on the last line the one after, then the
//!   text that was being written; elsewhere they move the cursor.
//!
//! It looks like a terminal prompt: a `❯`, the text in the monospace font
//! with no field around it, and a blinking block cursor ([`cursor`]).
//!
//! Keys are read with a keystroke interceptor, which runs before GPUI's own
//! key bindings and the editor, while the editor has focus. The session
//! also feeds keys typed while another panel has focus through
//! [`InputView::feed_key`], so the shortcuts work everywhere. A pending
//! prefix (the leader) is settled by a GPUI timer at the sequencer's
//! deadline (`timeoutlen`).
//!
//! Limitation: keys of a multi-key mapping that ends up not matching are
//! replayed into the editor as text (`Enter` as a newline); other
//! non-character keys of such a failed prefix are dropped. Text that an IME
//! commits directly, without key events, bypasses the sequencer.

pub mod attachments;
pub mod cursor;
pub mod history;
pub mod model_popup;
pub mod path_popup;
pub mod paths;
pub mod tray;

use std::sync::Arc;

use application::keymap::{
    Action, Feed, Key, KeyCode, KeySequencer, Keymap, Modifiers, default_keymap,
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

use application::ports::search::FileSearchPort;
use attachments::{ImageMode, PendingImage};
use domain::conversation::ImageAttachment;
use history::PromptHistory;
pub use model_popup::ModelPopup;
use model_popup::{ModelCompletion, ModelKey, model_key};
pub use path_popup::PathPopup;
use path_popup::{PathCompletion, PathKey, path_key};
use tray::{ImageTray, paste_handler};

use crate::help::CommandRow;
use crate::session::ModelChoice;
use domain::model::ModelId;

/// Action of `Up` in an empty prompt while prompts are queued: takes the
/// last one back into the prompt.
pub const QUEUE_EDIT_ACTION: &str = "queue.edit";
/// Action of `<Esc>`: back to the prompt (and hide the palette).
pub const FOCUS_INPUT_ACTION: &str = "focus.input";
/// Action of `<leader>i`: switches what `Enter` does with the pasted images.
pub const IMAGE_MODE_ACTION: &str = "image.mode";

/// Rows of the editor before it starts scrolling.
const MAX_ROWS: usize = 8;
/// Rows of the command palette.
const PALETTE_ROWS: usize = 8;

/// What the input asks the rest of the UI to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputEvent {
    /// The prompt was sent with `<CR>` (never empty).
    Submit(String),
    /// The prompt (maybe empty) was sent with pasted images; `mode` says
    /// whether they go to the model or only to the chat.
    SubmitImages {
        /// The prompt.
        text: String,
        /// The images, in the order they were pasted.
        images: Vec<ImageAttachment>,
        /// What to do with them.
        mode: ImageMode,
    },
    /// Something the user should know (an image that cannot be attached).
    Notice(String),
    /// A `/` command was run, without the slash.
    Command(String),
    /// A mapped action the input does not handle itself (timeline
    /// navigation, pickers, interrupt...).
    Action(Action),
    /// A sent line was added to the history, as typed (`/model opus`).
    Recorded(String),
    /// The `/model ` popup opened for the first time: list the models and
    /// hand them over with [`InputView::set_models`].
    ModelsWanted,
}

/// What a submitted line is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line {
    /// A prompt for the agent.
    Prompt(String),
    /// A command, without the slash.
    Command(String),
}

/// Splits a submitted line into a prompt or a `/command`; `//text` is the
/// prompt `/text`. `None` for an empty line or a bare `/`.
pub fn classify_line(text: &str) -> Option<Line> {
    if text.trim().is_empty() {
        return None;
    }
    let trimmed = text.trim_start();
    if let Some(literal) = trimmed.strip_prefix("//") {
        return Some(Line::Prompt(format!("/{literal}")));
    }
    match trimmed.strip_prefix('/') {
        Some(command) if command.trim().is_empty() => None,
        Some(command) => Some(Line::Command(command.trim().to_owned())),
        None => Some(Line::Prompt(text.to_owned())),
    }
}

/// The commands the palette offers for `text`, best first: names starting
/// with what follows the `/`, then names containing it. Empty unless
/// `text` is a `/` and a partial name (no arguments yet).
pub fn palette_matches<'a>(commands: &'a [CommandRow], text: &str) -> Vec<&'a CommandRow> {
    let Some(typed) = text.strip_prefix('/') else {
        return Vec::new();
    };
    if typed.starts_with('/') || typed.contains(char::is_whitespace) {
        return Vec::new();
    }
    let typed = typed.to_lowercase();
    let mut starts: Vec<&CommandRow> = Vec::new();
    let mut contains: Vec<&CommandRow> = Vec::new();
    for row in commands {
        let name = row.name.to_lowercase();
        if name.starts_with(&typed) {
            starts.push(row);
        } else if name.contains(&typed) {
            contains.push(row);
        }
    }
    starts.append(&mut contains);
    starts
}

/// What choosing a row of the palette does.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Choice {
    /// Runs the command (`plugin list`).
    Run(String),
    /// Puts `/name … ` in the prompt for the arguments it needs.
    Complete(String),
}

/// What `Enter` on `row` does: the fixed words of its usage run as they
/// are (`/plugin list`, `/clear`, `/help [section]`), and a usage that
/// needs an argument (`/implement <spec>`) is completed up to it.
fn choice(row: &CommandRow) -> Choice {
    let mut fixed = Vec::new();
    for word in row.usage.trim_start_matches('/').split_whitespace() {
        if word.starts_with("@<") {
            // A path: complete up to the `@`, which lists the workspace.
            return Choice::Complete(format!("/{} @", fixed.join(" ")));
        }
        if word.starts_with('<') {
            return Choice::Complete(format!("/{} ", fixed.join(" ")));
        }
        if word.starts_with('[') {
            break;
        }
        fixed.push(word);
    }
    if fixed.is_empty() {
        fixed.push(row.name.as_str());
    }
    Choice::Run(fixed.join(" "))
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

/// The prompt input.
pub struct InputView {
    sequencer: KeySequencer<Action>,
    editor: Entity<TextareaState>,
    focus_handle: FocusHandle,
    timeout: Option<Task<()>>,
    queued: bool,
    commands: Vec<CommandRow>,
    palette_selected: usize,
    palette_hidden: bool,
    paths: PathCompletion,
    models: ModelCompletion,
    /// The model the next turn uses, marked in the `/model ` popup.
    current_model: Option<ModelId>,
    tray: ImageTray,
    history: PromptHistory,
    blink: Blink,
    _blink: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<InputEvent> for InputView {}

impl InputView {
    /// An input with the builtin keymap.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::with_keymap(default_keymap(), window, cx)
    }

    /// An input with a custom keymap.
    pub fn with_keymap(
        keymap: Keymap<Action>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let editor = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, MAX_ROWS)
                .placeholder("Ask crest…  (/ for commands)")
        });
        let weak = cx.entity().downgrade();
        let intercept = cx.intercept_keystrokes(move |event, window, cx| {
            weak.update(cx, |this, cx| this.intercept(event, window, cx))
                .ok();
        });
        let changes = cx.subscribe(&editor, |this, _, event, cx| {
            if matches!(event, EditorEvent::Change) {
                this.palette_selected = 0;
                this.palette_hidden = false;
                let text = this.text(cx);
                this.paths.request(&text, cx);
                if this.models.wants_listing(&text) {
                    cx.emit(InputEvent::ModelsWanted);
                }
                cx.notify();
            }
        });
        Self {
            sequencer: KeySequencer::new(keymap),
            editor,
            focus_handle: cx.focus_handle(),
            timeout: None,
            queued: false,
            commands: Vec::new(),
            palette_selected: 0,
            palette_hidden: false,
            paths: PathCompletion::default(),
            models: ModelCompletion::default(),
            current_model: None,
            tray: ImageTray::default(),
            history: PromptHistory::default(),
            blink: Blink::default(),
            _blink: Self::blink_loop(window, cx),
            _subscriptions: vec![intercept, changes],
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

    /// Text of the prompt editor.
    pub fn text(&self, cx: &App) -> String {
        self.editor.read(cx).value().to_string()
    }

    /// The keymap in use.
    pub fn keymap(&self) -> &Keymap<Action> {
        self.sequencer.keymap()
    }

    /// The sequencer, e.g. to change `timeoutlen` or add mappings.
    pub fn sequencer_mut(&mut self) -> &mut KeySequencer<Action> {
        &mut self.sequencer
    }

    /// The commands the `/` palette offers.
    pub fn set_commands(&mut self, commands: Vec<CommandRow>, cx: &mut Context<Self>) {
        self.commands = commands;
        cx.notify();
    }

    /// The commands the palette shows for the current text.
    pub fn palette(&self, cx: &App) -> Vec<&CommandRow> {
        if self.palette_hidden {
            return Vec::new();
        }
        palette_matches(&self.commands, &self.text(cx))
    }

    /// Lists the directories `@` words name; without it they get no
    /// suggestions.
    pub fn set_file_search(&mut self, files: Arc<dyn FileSearchPort>) {
        self.paths.set_file_search(files);
    }

    /// What the `@` popup shows for the current text, if anything.
    pub fn path_popup(&self, cx: &App) -> Option<PathPopup> {
        if self.palette_hidden {
            return None;
        }
        self.paths.popup(&self.text(cx))
    }

    /// Hands the `/model ` popup the models of the providers, or why they
    /// could not be listed.
    pub fn set_models(&mut self, models: Result<Vec<ModelChoice>, String>, cx: &mut Context<Self>) {
        self.models.set(models);
        cx.notify();
    }

    /// The model the next turn uses, marked in the `/model ` popup.
    pub fn set_current_model(&mut self, model: Option<ModelId>) {
        self.current_model = model;
    }

    /// What the `/model ` popup shows for the current text, if anything.
    pub fn model_popup(&self, cx: &App) -> Option<ModelPopup> {
        if self.palette_hidden {
            return None;
        }
        self.models.popup(&self.text(cx))
    }

    /// The images waiting in the tray, in paste order.
    pub fn images(&self) -> &[PendingImage] {
        self.tray.images()
    }

    /// What `Enter` does with the images in the tray.
    pub fn image_mode(&self) -> ImageMode {
        self.tray.mode()
    }

    /// Switches what `Enter` does with the images (no-op without images).
    pub fn toggle_image_mode(&mut self, cx: &mut Context<Self>) {
        if self.tray.toggle() {
            cx.notify();
        }
    }

    /// Adds pasted images to the tray; an image that cannot be attached
    /// is reported with [`InputEvent::Notice`].
    pub fn attach_images(&mut self, images: Vec<gpui::Image>, cx: &mut Context<Self>) {
        let problems = self.tray.attach(images);
        self.report(problems, cx);
    }

    /// Puts images back in the tray (a queued prompt taken back to edit).
    pub fn restore_images(&mut self, images: Vec<ImageAttachment>, cx: &mut Context<Self>) {
        let problems = self.tray.restore(images);
        self.report(problems, cx);
    }

    fn report(&mut self, problems: Vec<String>, cx: &mut Context<Self>) {
        for problem in problems {
            cx.emit(InputEvent::Notice(problem));
        }
        cx.notify();
    }

    /// Removes the image at `ix` from the tray.
    pub fn remove_image(&mut self, ix: usize, cx: &mut Context<Self>) {
        if self.tray.remove(ix) {
            cx.notify();
        }
    }

    /// Gives keyboard focus to the editor.
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        let handle = self.editor.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
    }

    /// Inserts `text` at the cursor of the prompt editor and focuses it
    /// (e.g. a `@path` reference chosen in a picker, or a key typed while
    /// another panel had focus).
    pub fn insert_text(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.focus(window, cx);
        let text = text.to_owned();
        self.editor
            .update(cx, |editor, cx| editor.insert(text, window, cx));
    }

    /// Replaces the prompt with `text`, cursor at the end (a queued prompt
    /// taken back with `Up`).
    pub fn set_text(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.focus(window, cx);
        let text = text.to_owned();
        self.editor.update(cx, |editor, cx| {
            editor.set_value("", window, cx);
            editor.insert(text, window, cx);
        });
    }

    /// The lines sent so far, oldest first.
    pub fn history(&self) -> &[String] {
        self.history.entries()
    }

    /// Replaces the lines `Up` brings back (a resumed or new session's).
    pub fn set_history(&mut self, entries: Vec<String>) {
        self.history.set_entries(entries);
    }

    /// Tells the input whether prompts are queued: then `Up` in an empty
    /// prompt emits [`QUEUE_EDIT_ACTION`] instead of moving the cursor.
    pub fn set_queued(&mut self, queued: bool) {
        self.queued = queued;
    }

    /// Whether the editor has focus.
    pub fn owns_focus(&self, window: &Window, cx: &App) -> bool {
        self.focus_handle.contains_focused(window, cx)
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
        if self.palette_key(&key, window, cx) {
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

    /// The palette's keys; returns whether the key was one.
    fn palette_key(&mut self, key: &Key, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.sequencer.pending().is_empty() || key.mods != Modifiers::default() {
            return false;
        }
        if let Some(popup) = self.path_popup(cx) {
            match path_key(key, &popup, &self.text(cx), self.palette_selected) {
                PathKey::Select(ix) => self.palette_selected = ix,
                PathKey::Hide => self.palette_hidden = true,
                PathKey::SetText(text) => self.set_text(&text, window, cx),
                PathKey::Ignored => return false,
            }
            return true;
        }
        if let Some(popup) = self.model_popup(cx) {
            match model_key(key, &popup, self.palette_selected) {
                ModelKey::Select(ix) => self.palette_selected = ix,
                ModelKey::Hide => self.palette_hidden = true,
                ModelKey::SetText(text) => self.set_text(&text, window, cx),
                ModelKey::Run(line) => {
                    self.record(&format!("/{line}"), cx);
                    self.editor
                        .update(cx, |editor, cx| editor.set_value("", window, cx));
                    cx.emit(InputEvent::Command(line));
                }
                ModelKey::Ignored => return false,
            }
            return true;
        }
        let choices: Vec<Choice> = self.palette(cx).into_iter().map(choice).collect();
        if choices.is_empty() {
            return false;
        }
        let len = choices.len().min(PALETTE_ROWS);
        let selected = self.palette_selected.min(len - 1);
        match (key.code, &choices[selected]) {
            (KeyCode::Down, _) => self.palette_selected = (selected + 1) % len,
            (KeyCode::Up, _) => self.palette_selected = (selected + len - 1) % len,
            (KeyCode::Esc, _) => self.palette_hidden = true,
            (KeyCode::Tab, Choice::Run(line)) => self.set_text(&format!("/{line} "), window, cx),
            (KeyCode::Tab | KeyCode::Enter, Choice::Complete(line)) => {
                self.set_text(line, window, cx);
            }
            (KeyCode::Enter, Choice::Run(line)) => {
                let line = line.clone();
                self.record(&format!("/{line}"), cx);
                self.editor
                    .update(cx, |editor, cx| editor.set_value("", window, cx));
                cx.emit(InputEvent::Command(line));
            }
            _ => return false,
        }
        true
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
    fn record(&mut self, line: &str, cx: &mut Context<Self>) {
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
            _ => cx.emit(InputEvent::Action(action)),
        }
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let line = classify_line(&self.text(cx));
        // A command leaves the images in the tray for the next prompt.
        let event = match line {
            Some(Line::Command(command)) => InputEvent::Command(command),
            line if !self.tray.is_empty() => {
                let text = match line {
                    Some(Line::Prompt(text)) => text,
                    _ => String::new(),
                };
                let (images, mode) = self.tray.take();
                InputEvent::SubmitImages { text, images, mode }
            }
            Some(Line::Prompt(text)) => InputEvent::Submit(text),
            None => return,
        };
        let sent = self.text(cx);
        self.record(&sent, cx);
        self.editor
            .update(cx, |editor, cx| editor.set_value("", window, cx));
        cx.emit(event);
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

    fn pending_keys(&self) -> String {
        self.sequencer
            .pending()
            .iter()
            .map(ToString::to_string)
            .collect()
    }

    fn render_palette(&self, cx: &App) -> Option<impl IntoElement> {
        let rows = self.palette(cx);
        if rows.is_empty() {
            return None;
        }
        let theme = cx.theme();
        let selected = self.palette_selected.min(rows.len().min(PALETTE_ROWS) - 1);
        let items = rows.iter().take(PALETTE_ROWS).enumerate().map(|(ix, row)| {
            let mut item = h_flex()
                .id(("palette", ix))
                .gap_3()
                .px_2()
                .child(
                    div()
                        .text_color(theme.primary)
                        .child(SharedString::from(row.usage.clone())),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(theme.muted_foreground)
                        .child(SharedString::from(row.description.clone())),
                );
            if ix == selected {
                item = item.bg(theme.list_active).rounded_sm();
            }
            item
        });
        Some(
            v_flex()
                .id("command-palette")
                .mb_1()
                .py_1()
                .border_1()
                .border_color(theme.border)
                .rounded_md()
                .bg(theme.popover)
                .text_sm()
                .children(items)
                .child(
                    div()
                        .px_2()
                        .pt_1()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child("↑↓ choose · Tab complete · Enter run · Esc hide"),
                ),
        )
    }
}

impl Focusable for InputView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for InputView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tray = self.tray.render(cx);
        let paths = self
            .path_popup(cx)
            .map(|popup| path_popup::render(popup, self.palette_selected, cx));
        let models = if paths.is_none() {
            self.model_popup(cx).map(|popup| {
                model_popup::render(
                    popup,
                    self.palette_selected,
                    self.current_model.as_ref(),
                    cx,
                )
            })
        } else {
            None
        };
        let palette = if paths.is_none() && models.is_none() {
            self.render_palette(cx)
        } else {
            None
        };
        let focused = self.owns_focus(window, cx);
        let theme = cx.theme();
        let (block, inverted, muted) = (theme.foreground, theme.background, theme.muted_foreground);
        let prompt_color = if focused { theme.primary } else { muted };
        let shape = self.blink.visible().then_some(CursorShape::Block);
        let editor = self.editor.clone();
        // Painted after the textarea, from the caret it laid out this frame.
        let cursor = canvas(
            |_, _, _| {},
            move |_, (), window, cx| {
                let focused = editor.read(cx).focus_handle(cx).is_focused(window);
                if let Some(shape) = shape.filter(|_| focused) {
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
                    .text_color(prompt_color)
                    .font_weight(gpui::FontWeight::BOLD)
                    .child("❯"),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .child(HiddenCaret::new(
                        Textarea::new(&self.editor)
                            .appearance(false)
                            .on_paste(paste_handler(cx.entity().downgrade())),
                        self.editor.clone(),
                    ))
                    .child(cursor),
            );
        let pending = self.pending_keys();
        let status = (!pending.is_empty()).then(|| {
            h_flex()
                .gap_2()
                .text_xs()
                .text_color(muted)
                .child(SharedString::from(pending))
        });
        v_flex()
            .id("crest-input")
            .key_context("CrestInput")
            .track_focus(&self.focus_handle)
            .font_family(theme.mono_font_family.clone())
            .children(paths)
            .children(models)
            .children(palette)
            .children(tray)
            .child(prompt)
            .children(status)
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

    /// The events so far but the history's [`InputEvent::Recorded`].
    fn take(events: &Events) -> Vec<InputEvent> {
        take_all(events)
            .into_iter()
            .filter(|event| !matches!(event, InputEvent::Recorded(_)))
            .collect()
    }

    fn take_all(events: &Events) -> Vec<InputEvent> {
        events.borrow_mut().drain(..).collect()
    }

    fn action(name: &str) -> InputEvent {
        InputEvent::Action(Action::builtin(name))
    }

    fn row(name: &str, usage: &str) -> CommandRow {
        CommandRow {
            name: name.into(),
            usage: usage.into(),
            description: format!("does {name}"),
            source: "crest".into(),
        }
    }

    #[gpui::test]
    fn types_and_submits(cx: &mut TestAppContext) {
        let (view, cx, events) = input(cx);
        cx.simulate_input("hola");
        cx.simulate_keystrokes("shift-enter");
        cx.simulate_input("mundo");
        view.read_with(cx, |view, cx| assert_eq!(view.text(cx), "hola\nmundo"));
        cx.simulate_keystrokes("enter");
        assert_eq!(take(&events), [InputEvent::Submit("hola\nmundo".into())]);
        view.read_with(cx, |view, cx| assert_eq!(view.text(cx), ""));
    }

    #[gpui::test]
    fn plain_keys_are_always_typed(cx: &mut TestAppContext) {
        let (view, cx, events) = input(cx);
        cx.simulate_input("jk:?Gi");
        assert!(take(&events).is_empty());
        view.read_with(cx, |view, cx| assert_eq!(view.text(cx), "jk:?Gi"));
    }

    #[gpui::test]
    fn empty_prompt_is_not_submitted(cx: &mut TestAppContext) {
        let (_view, cx, events) = input(cx);
        cx.simulate_keystrokes("space enter");
        assert!(take(&events).is_empty());
    }

    #[gpui::test]
    fn a_slash_line_is_a_command(cx: &mut TestAppContext) {
        let (view, cx, events) = input(cx);
        cx.simulate_input("/model opus");
        cx.simulate_keystrokes("enter");
        assert_eq!(take(&events), [InputEvent::Command("model opus".into())]);
        cx.simulate_input("//etc/hosts");
        cx.simulate_keystrokes("enter");
        assert_eq!(take(&events), [InputEvent::Submit("/etc/hosts".into())]);
        view.read_with(cx, |view, cx| assert_eq!(view.text(cx), ""));
    }

    #[gpui::test]
    fn the_palette_completes_and_runs_commands(cx: &mut TestAppContext) {
        let (view, cx, events) = input(cx);
        view.update(cx, |view, cx| {
            view.set_commands(
                vec![
                    row("clear", "/clear"),
                    row("implement", "/implement <spec>"),
                    row("help", "/help [section]"),
                ],
                cx,
            );
        });
        cx.simulate_input("/cl");
        view.read_with(cx, |view, cx| assert_eq!(view.palette(cx).len(), 1));
        cx.simulate_keystrokes("enter");
        assert_eq!(take(&events), [InputEvent::Command("clear".into())]);
        // A command that needs an argument is completed, not run.
        cx.simulate_input("/imp");
        cx.simulate_keystrokes("enter");
        assert!(take(&events).is_empty());
        view.read_with(cx, |view, cx| assert_eq!(view.text(cx), "/implement "));
        view.update_in(cx, |view, window, cx| view.set_text("", window, cx));
        // Up/Down choose; Tab completes.
        cx.simulate_input("/");
        cx.simulate_keystrokes("down tab");
        view.read_with(cx, |view, cx| assert_eq!(view.text(cx), "/implement "));
        // Esc hides it until the text changes.
        view.update_in(cx, |view, window, cx| view.set_text("/h", window, cx));
        cx.simulate_keystrokes("escape");
        view.read_with(cx, |view, cx| assert!(view.palette(cx).is_empty()));
    }

    fn entries(view: &Entity<InputView>, cx: &mut VisualTestContext) -> Vec<String> {
        cx.run_until_parked();
        view.read_with(cx, |view, cx| match view.path_popup(cx) {
            Some(PathPopup::Entries(entries)) => entries
                .into_iter()
                .map(|e| {
                    if e.dir {
                        format!("{}/", e.name)
                    } else {
                        e.name
                    }
                })
                .collect(),
            other => panic!("no entries: {other:?}"),
        })
    }

    fn message(view: &Entity<InputView>, cx: &mut VisualTestContext) -> String {
        cx.run_until_parked();
        view.read_with(cx, |view, cx| match view.path_popup(cx) {
            Some(PathPopup::Message(message)) => message,
            other => panic!("no message: {other:?}"),
        })
    }

    #[gpui::test]
    fn at_words_complete_workspace_paths_level_by_level(cx: &mut TestAppContext) {
        use application::testing::fs::{InMemoryFs, test_root};

        let (view, cx, events) = input(cx);
        let fs = InMemoryFs::new(test_root())
            .with_file("README.md", "")
            .with_file("src/main.rs", "")
            .with_file("src/components/button.rs", "")
            .with_file(".env", "SECRET=1");
        view.update(cx, |view, cx| {
            view.set_commands(vec![row("open", "/open @<path>")], cx);
            view.set_file_search(Arc::new(fs));
        });
        // The palette completes `/open` up to the `@`, which lists the root.
        cx.simulate_input("/op");
        cx.simulate_keystrokes("enter");
        view.read_with(cx, |view, cx| assert_eq!(view.text(cx), "/open @"));
        assert_eq!(entries(&view, cx), ["src/", "README.md"]);
        cx.simulate_input("s");
        assert_eq!(entries(&view, cx), ["src/"]);
        cx.simulate_keystrokes("tab");
        view.read_with(cx, |view, cx| assert_eq!(view.text(cx), "/open @src/"));
        assert_eq!(entries(&view, cx), ["components/", "main.rs"]);
        cx.simulate_keystrokes("enter");
        view.read_with(cx, |view, cx| {
            assert_eq!(view.text(cx), "/open @src/components/");
        });
        assert_eq!(entries(&view, cx), ["button.rs"]);
        // Enter completes the file, then submits the line.
        cx.simulate_keystrokes("enter");
        view.read_with(cx, |view, cx| {
            assert_eq!(view.text(cx), "/open @src/components/button.rs ");
            assert_eq!(view.path_popup(cx), None);
        });
        assert!(take(&events).is_empty());
        cx.simulate_keystrokes("enter");
        assert_eq!(
            take(&events),
            [InputEvent::Command("open @src/components/button.rs".into())]
        );
        // Unmatched prefixes and paths out of the root only get a message.
        cx.simulate_input("look at @zz");
        assert_eq!(message(&view, cx), "nothing in the workspace matches `zz`");
        view.update_in(cx, |view, window, cx| view.set_text("@../", window, cx));
        assert!(message(&view, cx).contains("inside the workspace"));
        view.update_in(cx, |view, window, cx| view.set_text("@/etc/", window, cx));
        assert!(message(&view, cx).contains("inside the workspace"));
        // Up/Down move the choice; Esc hides the popup.
        view.update_in(cx, |view, window, cx| view.set_text("@", window, cx));
        cx.run_until_parked();
        cx.simulate_keystrokes("down tab");
        view.read_with(cx, |view, cx| assert_eq!(view.text(cx), "@README.md "));
        view.update_in(cx, |view, window, cx| view.set_text("@", window, cx));
        cx.run_until_parked();
        cx.simulate_keystrokes("escape");
        view.read_with(cx, |view, cx| assert_eq!(view.path_popup(cx), None));
    }

    fn clipboard_png(width: u32, cx: &mut VisualTestContext) {
        let png = attachments::tests::encoded(width, 2, image::ImageFormat::Png);
        let item =
            gpui::ClipboardItem::new_image(&gpui::Image::from_bytes(gpui::ImageFormat::Png, png));
        cx.write_to_clipboard(item);
    }

    #[gpui::test]
    fn pasted_images_wait_in_the_tray_until_sent(cx: &mut TestAppContext) {
        let (view, cx, events) = input(cx);
        cx.simulate_input("describe");
        clipboard_png(3, cx);
        cx.simulate_keystrokes("secondary-v");
        clipboard_png(5, cx);
        cx.simulate_keystrokes("secondary-v");
        view.read_with(cx, |view, cx| {
            // The typed text stays; the images are in the tray.
            assert_eq!(view.text(cx), "describe");
            let widths: Vec<u32> = view.images().iter().map(|i| i.attachment.width).collect();
            assert_eq!(widths, [3, 5]);
            assert_eq!(view.image_mode(), ImageMode::Model);
        });
        // Text still pastes as text.
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(" it".into()));
        cx.simulate_keystrokes("secondary-v");
        view.read_with(cx, |view, cx| assert_eq!(view.text(cx), "describe it"));
        // A command leaves the images for the next prompt.
        view.update_in(cx, |view, window, cx| {
            view.set_text("/mode ask", window, cx)
        });
        cx.simulate_keystrokes("enter");
        assert_eq!(take(&events), [InputEvent::Command("mode ask".into())]);
        view.read_with(cx, |view, _| assert_eq!(view.images().len(), 2));
        // Backspace in an empty prompt removes the last one; `<leader>i` switches
        // the mode, which the submission carries.
        cx.simulate_keystrokes("backspace");
        view.read_with(cx, |view, _| assert_eq!(view.images().len(), 1));
        cx.simulate_keystrokes("ctrl-g i");
        view.read_with(cx, |view, _| {
            assert_eq!(view.image_mode(), ImageMode::ChatOnly)
        });
        cx.simulate_keystrokes("ctrl-g i");
        cx.simulate_keystrokes("enter");
        let sent = take(&events);
        let [InputEvent::SubmitImages { text, images, mode }] = sent.as_slice() else {
            panic!("{sent:?}");
        };
        assert_eq!((text.as_str(), *mode), ("", ImageMode::Model));
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].media_type, "image/png");
        view.read_with(cx, |view, _| assert!(view.images().is_empty()));
        // An image that cannot be attached is reported.
        clipboard_png(domain::conversation::MAX_IMAGE_SIDE + 1, cx);
        cx.simulate_keystrokes("secondary-v");
        assert!(matches!(take(&events).as_slice(), [InputEvent::Notice(m)] if m.contains("px")));
        view.read_with(cx, |view, _| assert!(view.images().is_empty()));
    }

    #[gpui::test]
    fn the_tray_buttons_remove_images_and_switch_the_mode(cx: &mut TestAppContext) {
        let (view, cx, _events) = input(cx);
        clipboard_png(4, cx);
        cx.simulate_keystrokes("secondary-v");
        cx.draw(
            gpui::point(gpui::px(0.), gpui::px(0.)),
            gpui::size(gpui::px(800.), gpui::px(400.)),
            |_, _| div().size_full().child(view.clone()),
        );
        let chat = cx.debug_bounds("tray-chat-only").expect("mode chip drawn");
        cx.simulate_click(chat.center(), gpui::Modifiers::none());
        view.read_with(cx, |view, _| {
            assert_eq!(view.image_mode(), ImageMode::ChatOnly)
        });
        let remove = cx.debug_bounds("tray-remove-0").expect("× drawn");
        cx.simulate_click(remove.center(), gpui::Modifiers::none());
        view.read_with(cx, |view, _| {
            assert!(view.images().is_empty());
            assert_eq!(view.image_mode(), ImageMode::Model);
        });
    }

    #[gpui::test]
    fn leader_and_chords_fire_actions(cx: &mut TestAppContext) {
        let (view, cx, events) = input(cx);
        cx.simulate_keystrokes("ctrl-g e ctrl-g k ctrl-g j ctrl-g g g ctrl-g shift-g ctrl-c");
        assert_eq!(
            take(&events),
            [
                action("panel.tree"),
                action("block.prev"),
                action("block.next"),
                action("block.first"),
                action("block.last"),
                action("turn.interrupt")
            ]
        );
        view.read_with(cx, |view, cx| assert_eq!(view.text(cx), ""));
    }

    #[gpui::test]
    fn pending_leader_times_out(cx: &mut TestAppContext) {
        let (view, cx, events) = input(cx);
        cx.simulate_keystrokes("ctrl-g");
        view.read_with(cx, |view, _| assert_eq!(view.pending_keys(), "<C-g>"));
        cx.executor().advance_clock(Duration::from_millis(1100));
        cx.run_until_parked();
        view.read_with(cx, |view, _| assert_eq!(view.pending_keys(), ""));
        cx.simulate_keystrokes("e");
        assert!(
            take(&events).is_empty(),
            "the timed-out leader does not pair up"
        );
    }

    #[gpui::test]
    fn up_and_down_browse_the_sent_lines_and_return_to_the_draft(cx: &mut TestAppContext) {
        let (view, cx, events) = input(cx);
        cx.simulate_input("uno");
        cx.simulate_keystrokes("enter");
        cx.simulate_input("/model opus");
        cx.simulate_keystrokes("enter");
        let recorded: Vec<_> = take_all(&events)
            .into_iter()
            .filter(|event| matches!(event, InputEvent::Recorded(_)))
            .collect();
        assert_eq!(
            recorded,
            [
                InputEvent::Recorded("uno".into()),
                InputEvent::Recorded("/model opus".into())
            ]
        );
        let text = |cx: &mut VisualTestContext| view.read_with(cx, |view, cx| view.text(cx));
        cx.simulate_input("bor");
        cx.simulate_keystrokes("up");
        assert_eq!(text(cx), "/model opus");
        cx.simulate_keystrokes("up");
        assert_eq!(text(cx), "uno");
        cx.simulate_keystrokes("up");
        assert_eq!(text(cx), "uno", "the oldest line stays");
        cx.simulate_keystrokes("down");
        assert_eq!(text(cx), "/model opus");
        cx.simulate_keystrokes("down");
        assert_eq!(text(cx), "bor", "the draft comes back");
        assert!(take(&events).is_empty());
    }

    #[gpui::test]
    fn up_moves_the_cursor_until_the_first_line(cx: &mut TestAppContext) {
        let (view, cx, _) = input(cx);
        cx.update(|window, cx| {
            view.update(cx, |view, _| view.set_history(vec!["antes".into()]));
            view.update(cx, |view, cx| view.set_text("a\nb", window, cx));
        });
        let text = |cx: &mut VisualTestContext| view.read_with(cx, |view, cx| view.text(cx));
        cx.simulate_keystrokes("up");
        assert_eq!(text(cx), "a\nb", "the cursor went to the first line");
        cx.simulate_keystrokes("up");
        assert_eq!(text(cx), "antes");
        cx.simulate_keystrokes("down");
        assert_eq!(text(cx), "a\nb");
    }

    #[gpui::test]
    fn a_queued_prompt_comes_back_before_the_history(cx: &mut TestAppContext) {
        let (view, cx, events) = input(cx);
        view.update(cx, |view, _| {
            view.set_history(vec!["antes".into()]);
            view.set_queued(true);
        });
        cx.simulate_keystrokes("up");
        assert_eq!(take(&events), [action(QUEUE_EDIT_ACTION)]);
        view.read_with(cx, |view, cx| assert_eq!(view.text(cx), ""));
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

    #[test]
    fn lines_are_prompts_or_commands() {
        assert_eq!(classify_line("  "), None);
        assert_eq!(classify_line("/"), None);
        assert_eq!(classify_line("hola"), Some(Line::Prompt("hola".into())));
        assert_eq!(
            classify_line(" /mode plan "),
            Some(Line::Command("mode plan".into()))
        );
        assert_eq!(classify_line("//x"), Some(Line::Prompt("/x".into())));
        assert_eq!(
            classify_line("/!ls -la"),
            Some(Line::Command("!ls -la".into()))
        );
    }

    #[test]
    fn palette_choices_run_fixed_words_and_complete_arguments() {
        assert_eq!(choice(&row("clear", "/clear")), Choice::Run("clear".into()));
        assert_eq!(
            choice(&row("help", "/help [section|search]")),
            Choice::Run("help".into())
        );
        assert_eq!(
            choice(&row("plugin", "/plugin list")),
            Choice::Run("plugin list".into())
        );
        assert_eq!(
            choice(&row("plugin", "/plugin trust <name>")),
            Choice::Complete("/plugin trust ".into())
        );
        assert_eq!(
            choice(&row("!", "/! <command>")),
            Choice::Complete("/! ".into())
        );
    }

    #[test]
    fn the_palette_prefers_prefixes() {
        let rows = [
            row("research", "/research"),
            row("clear", "/clear"),
            row("mcp", "/mcp"),
        ];
        let names = |text| -> Vec<&str> {
            palette_matches(&rows, text)
                .iter()
                .map(|r| r.name.as_str())
                .collect()
        };
        assert_eq!(names("/"), ["research", "clear", "mcp"]);
        assert_eq!(names("/c"), ["clear", "research", "mcp"]);
        assert_eq!(names("/re"), ["research"]);
        assert!(names("/research x").is_empty());
        assert!(names("hola").is_empty());
        assert!(names("//").is_empty());
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
