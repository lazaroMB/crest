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
//! - `Ctrl+C` ([`CANCEL_ACTION`]) copies the selected text; with none it
//!   clears the prompt into the history (`Up` brings it back), and in an
//!   empty prompt it asks to interrupt the turn.
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
pub mod completion;
pub mod cursor;
pub mod history;
mod keys;
pub mod model_popup;
mod palette;
mod paste;
pub mod path_popup;
pub mod paths;
mod popups;
mod render;
pub mod theme_popup;
pub mod tray;

pub use keys::key_from_keystroke;
pub use palette::palette_matches;

use std::cell::RefCell;
use std::rc::Rc;

use application_core::keymap::{Action, KeySequencer, Keymap, default_keymap};
use gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable, SharedString,
    Subscription, Task, Window,
};
use gpui_component::input::{InputEvent as EditorEvent, TextareaState};

use cursor::{BLINK_INTERVAL, Blink};

use attachments::ImageMode;
use domain::conversation::ImageAttachment;
use history::PromptHistory;
use model_popup::ModelCompletion;
pub use model_popup::ModelPopup;
use path_popup::PathCompletion;
pub use path_popup::PathPopup;
pub use theme_popup::ThemePopup;
use theme_popup::{ThemeCompletion, theme_query};
use tray::ImageTray;

use crate::help::CommandRow;
use domain::model::ModelId;

/// Action of `Up` in an empty prompt while prompts are queued: takes the
/// last one back into the prompt.
pub const QUEUE_EDIT_ACTION: &str = "queue.edit";
/// Action of `<Esc>`: back to the prompt (and hide the palette).
pub const FOCUS_INPUT_ACTION: &str = "focus.input";
/// Action of `<leader>i`: switches what `Enter` does with the pasted images.
pub const IMAGE_MODE_ACTION: &str = "image.mode";
/// Action of `<C-c>`: copies the selection, else clears the prompt (`Up`
/// brings it back), else asks to interrupt the turn (`turn.interrupt`).
pub const CANCEL_ACTION: &str = "input.cancel";

/// Rows of the editor before it starts scrolling.
const MAX_ROWS: usize = 8;
/// Pixels of wheel that move a popup by one row.
const WHEEL_ROW: f32 = 20.0;

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
    /// `Ctrl+C` cleared the prompt; `Up` brings the text back.
    Cleared,
    /// The `/model ` popup opened for the first time: list the models and
    /// hand them over with [`InputView::set_models`].
    ModelsWanted,
    /// The `/theme ` popup opened: list the themes and hand them over
    /// with [`InputView::set_themes`].
    ThemesWanted,
    /// The `/theme ` popup chose this theme: show it until it is saved or
    /// [`InputEvent::ThemePreviewEnd`].
    ThemePreview(String),
    /// The `/theme ` popup closed without saving: go back to the theme in
    /// use.
    ThemePreviewEnd,
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

/// The prompt's placeholder: what to type, with the keys of the pickers
/// (`Ask crest…  (/ for commands · Ctrl+G ff files · Ctrl+G fg text)`),
/// so they can be found without the help.
pub fn prompt_placeholder(keymap: &Keymap<Action>) -> String {
    let mut hints = vec!["/ for commands".to_owned()];
    for (action, what) in [
        (crate::picker::FILES_ACTION, "files"),
        (crate::picker::GREP_ACTION, "text"),
    ] {
        if let Some(keys) = crate::help::action_keys(keymap, action) {
            hints.push(format!("{keys} {what}"));
        }
    }
    format!("Ask crest…  ({})", hints.join(" · "))
}

/// The prompt input.
pub struct InputView {
    sequencer: KeySequencer<Action>,
    /// The keys of the image mode for the tray, looked up in the keymap
    /// once rather than on every frame; dropped by [`Self::sequencer_mut`],
    /// the only way the keymap changes.
    image_mode_keys: Option<Option<String>>,
    editor: Entity<TextareaState>,
    focus_handle: FocusHandle,
    timeout: Option<Task<()>>,
    queued: bool,
    /// The editor's text as last read, so that a frame that reads it a few
    /// times (palette, popups) copies it out of the editor only once per
    /// change.
    text: RefCell<SharedString>,
    commands: Vec<CommandRow>,
    /// The palette rows (indices into `commands`) for a text, worked out
    /// once per text and command list.
    palette_rows: RefCell<Option<(SharedString, Rc<[usize]>)>>,
    palette_selected: usize,
    /// First visible row of the `/model ` popup, which scrolls.
    palette_top: usize,
    /// Wheel pixels over a popup not yet a whole row.
    wheel_rest: f32,
    palette_hidden: bool,
    paths: PathCompletion,
    models: ModelCompletion,
    /// The model the next turn uses, marked in the `/model ` popup.
    current_model: Option<ModelId>,
    themes: ThemeCompletion,
    /// Name of the theme in use, marked in the `/theme ` popup.
    current_theme: String,
    /// Whether the `/theme ` popup is showing another theme.
    theme_previewing: bool,
    tray: ImageTray,
    /// The last step of the paste chain: images being prepared for the
    /// tray, or a prompt sent or an image mode switch pressed while some
    /// were, which waits for them.
    attaching: Option<Task<()>>,
    /// Steps of the paste chain not landed yet.
    pastes_pending: usize,
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
        let placeholder = prompt_placeholder(&keymap);
        let editor = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, MAX_ROWS)
                .placeholder(placeholder)
        });
        let weak = cx.entity().downgrade();
        let intercept = cx.intercept_keystrokes(move |event, window, cx| {
            weak.update(cx, |this, cx| this.intercept(event, window, cx))
                .ok();
        });
        let changes = cx.subscribe(&editor, |this, _, event, cx| {
            if matches!(event, EditorEvent::Change) {
                this.palette_selected = 0;
                this.palette_top = 0;
                this.palette_hidden = false;
                let text = this.text(cx);
                this.paths.request(&text, cx);
                if this.models.wants_listing(&text) {
                    cx.emit(InputEvent::ModelsWanted);
                }
                if this.themes.wants_listing(&text) {
                    cx.emit(InputEvent::ThemesWanted);
                }
                if theme_query(&text).is_none() {
                    this.end_theme_preview(cx);
                    // Theme files may change: list them again next time.
                    this.themes.forget();
                }
                cx.notify();
            }
        });
        let focus_handle = cx.focus_handle();
        // The popups belong to the prompt: they go when it loses the focus.
        let blur = cx.on_focus_out(&focus_handle, window, |this, _, _, cx| {
            this.hide_popups(cx);
        });
        Self {
            sequencer: KeySequencer::new(keymap),
            image_mode_keys: None,
            editor,
            focus_handle,
            timeout: None,
            queued: false,
            text: RefCell::default(),
            commands: Vec::new(),
            palette_rows: RefCell::default(),
            palette_selected: 0,
            palette_top: 0,
            wheel_rest: 0.0,
            palette_hidden: false,
            paths: PathCompletion::default(),
            models: ModelCompletion::default(),
            current_model: None,
            themes: ThemeCompletion::default(),
            current_theme: String::new(),
            theme_previewing: false,
            tray: ImageTray::default(),
            attaching: None,
            pastes_pending: 0,
            history: PromptHistory::default(),
            blink: Blink::default(),
            _blink: Self::blink_loop(window, cx),
            _subscriptions: vec![intercept, changes, blur],
        }
    }

    /// Hides the `/`, `@`, `/model` and `/theme` popups (until the text
    /// changes), ending a theme preview.
    pub fn hide_popups(&mut self, cx: &mut Context<Self>) {
        if self.palette_hidden {
            return;
        }
        self.palette_hidden = true;
        self.end_theme_preview(cx);
        cx.notify();
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
        self.shared_text(cx).to_string()
    }

    /// [`Self::text`] without a copy: the editor's text is copied out of
    /// it only when it differs from the last read (a comparison that
    /// allocates nothing).
    fn shared_text(&self, cx: &App) -> SharedString {
        let editor = self.editor.read(cx);
        let mut cached = self.text.borrow_mut();
        if editor.text() != cached.as_ref() {
            *cached = editor.value();
        }
        cached.clone()
    }

    /// The keymap in use.
    pub fn keymap(&self) -> &Keymap<Action> {
        self.sequencer.keymap()
    }

    /// Whether a key sequence (the leader) waits for its next key.
    pub fn has_pending_keys(&self) -> bool {
        !self.sequencer.pending().is_empty()
    }

    /// The sequencer, e.g. to change `timeoutlen` or add mappings.
    pub fn sequencer_mut(&mut self) -> &mut KeySequencer<Action> {
        self.image_mode_keys = None;
        &mut self.sequencer
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
}

impl Focusable for InputView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

#[cfg(test)]
mod tests;
