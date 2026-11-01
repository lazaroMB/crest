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
pub mod model_popup;
pub mod path_popup;
pub mod paths;
pub mod theme_popup;
pub mod tray;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use application::keymap::{
    Action, Feed, Key, KeyCode, KeySequencer, Keymap, Modifiers, default_keymap,
};
use gpui::canvas;
use gpui::{
    App, AppContext as _, ClipboardItem, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, KeystrokeEvent, ParentElement as _, Render,
    ScrollWheelEvent, SharedString, Styled as _, Subscription, Task, Window, div, px,
};
use gpui_component::input::{InputEvent as EditorEvent, Textarea, TextareaState};

use cursor::{BLINK_INTERVAL, Blink, CursorShape, HiddenCaret, paint_cursor};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};

use application::ports::search::FileSearchPort;
use attachments::{ImageMode, PendingImage};
use completion::{PALETTE_ROWS, Popup, PopupKey, palette_top, scroll_window, window_note};
use domain::conversation::ImageAttachment;
use history::PromptHistory;
pub use model_popup::ModelPopup;
use model_popup::{ModelCompletion, model_key};
pub use path_popup::PathPopup;
use path_popup::{PathCompletion, path_key};
pub use theme_popup::ThemePopup;
use theme_popup::{ThemeCompletion, theme_key, theme_query};
use tray::{ImageTray, paste_handler};

use crate::help::CommandRow;
use crate::session::{INTERRUPT_ACTION, ModelChoice, ThemeChoice};
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

/// The completion popup the input shows.
enum Completion {
    /// `@` paths.
    Path(PathPopup),
    /// `/model ` models.
    Model(ModelPopup),
    /// `/theme ` themes.
    Theme(ThemePopup),
}

impl Completion {
    /// Entries to choose from.
    fn len(&self) -> usize {
        match self {
            Self::Path(popup) => popup.len(),
            Self::Model(popup) => popup.len(),
            Self::Theme(popup) => popup.len(),
        }
    }
}

/// A step of the paste chain of [`InputView`].
enum PasteStep {
    /// Pasted images, being prepared in the background.
    Images(Task<Vec<Result<PendingImage, String>>>),
    /// A prompt sent while images were being prepared.
    Submit(String),
}

/// A [`PasteStep`] ready to land.
enum Landed {
    Images(Vec<Result<PendingImage, String>>),
    Submit(String),
}

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

/// The commands the palette offers for `text`, best first: names starting
/// with what follows the `/`, then names containing it. Empty unless
/// `text` is a `/` and a partial name (no arguments yet).
pub fn palette_matches<'a>(commands: &'a [CommandRow], text: &str) -> Vec<&'a CommandRow> {
    palette_indices(commands, text)
        .into_iter()
        .map(|ix| &commands[ix])
        .collect()
}

/// [`palette_matches`] as indices into `commands`.
fn palette_indices(commands: &[CommandRow], text: &str) -> Vec<usize> {
    let Some(typed) = text.strip_prefix('/') else {
        return Vec::new();
    };
    if typed.starts_with('/') {
        return Vec::new();
    }
    let typed = typed.to_lowercase();
    if typed.contains(char::is_whitespace) {
        // After a space only the rows named with their argument match
        // (`/mode ` lists `mode plan`...).
        return commands
            .iter()
            .enumerate()
            .filter(|(_, row)| {
                let name = row.name.to_lowercase();
                name.contains(' ') && name.starts_with(&typed)
            })
            .map(|(ix, _)| ix)
            .collect();
    }
    let mut starts: Vec<usize> = Vec::new();
    let mut contains: Vec<usize> = Vec::new();
    for (ix, row) in commands.iter().enumerate() {
        let name = row.name.to_lowercase();
        // A row named with its argument (`mode plan`) waits for the space.
        if name.contains(' ') {
            continue;
        }
        if name.starts_with(&typed) {
            starts.push(ix);
        } else if name.contains(&typed) {
            contains.push(ix);
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
    /// tray, or a prompt sent while some were, which waits for them.
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

    /// The commands the `/` palette offers.
    pub fn set_commands(&mut self, commands: Vec<CommandRow>, cx: &mut Context<Self>) {
        self.commands = commands;
        self.palette_rows.take();
        cx.notify();
    }

    /// The commands the palette shows for the current text.
    pub fn palette(&self, cx: &App) -> Vec<&CommandRow> {
        if self.palette_hidden {
            return Vec::new();
        }
        let text = self.shared_text(cx);
        let mut cached = self.palette_rows.borrow_mut();
        let rows = match cached.as_ref() {
            Some((key, rows)) if *key == text => Rc::clone(rows),
            _ => {
                let rows: Rc<[usize]> = palette_indices(&self.commands, &text).into();
                *cached = Some((text, Rc::clone(&rows)));
                rows
            }
        };
        rows.iter()
            .filter_map(|&ix| self.commands.get(ix))
            .collect()
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
        self.paths.popup(&self.shared_text(cx))
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

    /// Hands the `/theme ` popup the themes, or why they could not be
    /// listed.
    pub fn set_themes(&mut self, themes: Result<Vec<ThemeChoice>, String>, cx: &mut Context<Self>) {
        self.themes.set(themes);
        cx.notify();
    }

    /// The theme in use, marked in the `/theme ` popup; the themes are
    /// listed again the next time it opens.
    pub fn set_current_theme(&mut self, name: &str) {
        self.current_theme = name.to_owned();
        self.themes.forget();
    }

    /// What the `/theme ` popup shows for the current text, if anything.
    pub fn theme_popup(&self, cx: &App) -> Option<ThemePopup> {
        if self.palette_hidden {
            return None;
        }
        self.themes.popup(&self.shared_text(cx))
    }

    fn end_theme_preview(&mut self, cx: &mut Context<Self>) {
        if std::mem::take(&mut self.theme_previewing) {
            cx.emit(InputEvent::ThemePreviewEnd);
        }
    }

    /// What the `/model ` popup shows for the current text, if anything.
    pub fn model_popup(&self, cx: &App) -> Option<ModelPopup> {
        if self.palette_hidden {
            return None;
        }
        self.models.popup(&self.shared_text(cx))
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
    /// is reported with [`InputEvent::Notice`]. They are checked and
    /// converted (a bmp re-encoded as png, the base64 of every one) on the
    /// background executor and land in the tray a moment later, in paste
    /// order.
    pub fn attach_images(&mut self, images: Vec<gpui::Image>, cx: &mut Context<Self>) {
        let prepared = cx.background_spawn(async move { tray::prepare(&images) });
        self.after_pastes(PasteStep::Images(prepared), cx);
    }

    /// Whether pasted images are still being prepared (or a prompt sent
    /// meanwhile still waits for them).
    pub fn pastes_pending(&self) -> bool {
        self.pastes_pending > 0
    }

    /// Chains `step` after the pastes not landed yet, so pastes land in
    /// paste order and a prompt sent while some were being prepared goes
    /// with them.
    fn after_pastes(&mut self, step: PasteStep, cx: &mut Context<Self>) {
        let previous = self.attaching.take();
        self.pastes_pending += 1;
        self.attaching = Some(cx.spawn(async move |this, cx| {
            let landed = match step {
                PasteStep::Images(prepared) => Landed::Images(prepared.await),
                PasteStep::Submit(text) => Landed::Submit(text),
            };
            // An earlier step lands first.
            if let Some(previous) = previous {
                previous.await;
            }
            this.update(cx, |this, cx| {
                this.pastes_pending = this.pastes_pending.saturating_sub(1);
                match landed {
                    Landed::Images(prepared) => {
                        let problems = this.tray.add(prepared);
                        this.report(problems, cx);
                    }
                    Landed::Submit(text) => {
                        if let Some(event) = this.prompt_event(text) {
                            cx.emit(event);
                        }
                        cx.notify();
                    }
                }
            })
            .ok();
        }));
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
        if let Some(shown) = self.completion(cx) {
            let pressed = match &shown {
                Completion::Path(popup) => {
                    path_key(key, popup, &self.text(cx), self.palette_selected)
                }
                Completion::Model(popup) => model_key(key, popup, self.palette_selected),
                Completion::Theme(popup) => theme_key(key, popup, self.palette_selected),
            };
            let theme = matches!(shown, Completion::Theme(_));
            match pressed {
                PopupKey::Select(ix) => {
                    self.palette_top = palette_top(ix, self.palette_top, shown.len());
                    if let Completion::Theme(Popup::Entries(entries)) = &shown {
                        self.theme_previewing = true;
                        cx.emit(InputEvent::ThemePreview(entries[ix].name.clone()));
                    }
                    self.palette_selected = ix;
                }
                PopupKey::Hide => {
                    self.palette_hidden = true;
                    if theme {
                        self.end_theme_preview(cx);
                    }
                }
                PopupKey::SetText(text) => self.set_text(&text, window, cx),
                PopupKey::Run(line) => {
                    if theme {
                        // Saving replaces the preview.
                        self.theme_previewing = false;
                    }
                    self.record(&format!("/{line}"), cx);
                    self.editor
                        .update(cx, |editor, cx| editor.set_value("", window, cx));
                    cx.emit(InputEvent::Command(line));
                }
                PopupKey::Ignored => return false,
            }
            return true;
        }
        let choices: Vec<Choice> = self.palette(cx).into_iter().map(choice).collect();
        if choices.is_empty() {
            return false;
        }
        let len = choices.len();
        let selected = self.palette_selected.min(len - 1);
        match (key.code, &choices[selected]) {
            (KeyCode::Down | KeyCode::Up, _) => {
                let ix = if key.code == KeyCode::Down {
                    (selected + 1) % len
                } else {
                    (selected + len - 1) % len
                };
                self.palette_top = palette_top(ix, self.palette_top, len);
                self.palette_selected = ix;
            }
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

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let line = classify_line(&self.text(cx));
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
    fn prompt_event(&mut self, text: String) -> Option<InputEvent> {
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

    fn pending_keys(&self) -> String {
        self.sequencer
            .pending()
            .iter()
            .map(ToString::to_string)
            .collect()
    }

    /// The completion popup the input shows, if any: the `@` one first,
    /// then `/model `, then `/theme ` (the same one `render` picks).
    fn completion(&self, cx: &App) -> Option<Completion> {
        if let Some(popup) = self.path_popup(cx) {
            return Some(Completion::Path(popup));
        }
        if let Some(popup) = self.model_popup(cx) {
            return Some(Completion::Model(popup));
        }
        self.theme_popup(cx).map(Completion::Theme)
    }

    /// Entries of the popup the input shows, the same one `render` picks.
    fn popup_len(&self, cx: &App) -> usize {
        match self.completion(cx) {
            Some(shown) => shown.len(),
            None => self.palette(cx).len(),
        }
    }

    /// The wheel over a popup scrolls its rows, carrying the chosen one
    /// along when it would leave the window (the `/theme ` popup previews
    /// it).
    fn popup_wheel(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let len = self.popup_len(cx);
        if len <= PALETTE_ROWS {
            return;
        }
        cx.stop_propagation();
        // Up is a positive delta; small touchpad deltas add up to a row.
        self.wheel_rest -= f32::from(event.delta.pixel_delta(px(WHEEL_ROW)).y);
        let rows = (self.wheel_rest / WHEEL_ROW).trunc();
        if rows == 0.0 {
            return;
        }
        self.wheel_rest -= rows * WHEEL_ROW;
        let before = self.palette_selected.min(len - 1);
        let (top, selected) = scroll_window(self.palette_top, before, rows as isize, len);
        self.palette_top = top;
        self.palette_selected = selected;
        if selected != before
            && let Some(Completion::Theme(Popup::Entries(entries))) = self.completion(cx)
            && let Some(choice) = entries.get(selected)
        {
            self.theme_previewing = true;
            cx.emit(InputEvent::ThemePreview(choice.name.clone()));
        }
        cx.notify();
    }

    fn render_palette(&self, cx: &App) -> Option<gpui::AnyElement> {
        let rows = self.palette(cx);
        if rows.is_empty() {
            return None;
        }
        let theme = cx.theme();
        let total = rows.len();
        let top = self.palette_top;
        let note = window_note(top, total).unwrap_or_default();
        let items = completion::render_rows(
            "palette",
            rows.iter(),
            total,
            self.palette_selected,
            top,
            theme,
            |row| {
                h_flex()
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
                    )
            },
        );
        Some(completion::frame(
            "command-palette",
            items,
            format!("↑↓ choose · Tab complete · Enter run · Esc hide{note}"),
            theme,
        ))
    }

    /// The keys that can follow the pending prefix (the leader), for the
    /// which-key popup. Empty when nothing is pending.
    pub fn which_key(&self) -> Vec<crate::help::NextKey> {
        let pending = self.sequencer.pending();
        if pending.is_empty() {
            return Vec::new();
        }
        crate::help::next_keys(self.keymap(), pending)
    }

    /// The which-key popup: while a prefix waits for its next key, what
    /// each next key does. It goes away with the prefix (a key, `Esc`, or
    /// `timeoutlen`).
    fn render_which_key(&self, cx: &App) -> Option<gpui::AnyElement> {
        let next = self.which_key();
        if next.is_empty() {
            return None;
        }
        let theme = cx.theme();
        let pending = crate::help::keys_label(self.sequencer.pending());
        let cells = next.into_iter().map(|next| {
            h_flex()
                .w(px(300.))
                .gap_2()
                .px_2()
                .child(
                    div()
                        .w(px(64.))
                        .flex_none()
                        .text_color(theme.primary)
                        .child(SharedString::from(next.key)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(theme.muted_foreground)
                        .child(SharedString::from(next.what)),
                )
        });
        Some(
            v_flex()
                .id("which-key")
                .debug_selector(|| "which-key".to_owned())
                .mb_1()
                .py_1()
                .border_1()
                .border_color(theme.border)
                .rounded_md()
                .bg(theme.popover)
                .text_sm()
                .child(h_flex().flex_wrap().children(cells))
                .child(
                    div()
                        .px_2()
                        .pt_1()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(SharedString::from(format!(
                            "{pending} … · a key runs it · Esc or a pause cancels"
                        ))),
                )
                .into_any_element(),
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
        let mode_keys = self
            .image_mode_keys
            .get_or_insert_with(|| {
                crate::help::action_keys(self.sequencer.keymap(), IMAGE_MODE_ACTION)
            })
            .clone();
        let tray = self.tray.render(mode_keys.as_deref(), cx);
        let which_key = self.render_which_key(cx);
        let completion = self.completion(cx).map(|shown| {
            // The list may have arrived or shrunk since the last key.
            let len = shown.len();
            if len > 0 {
                let selected = self.palette_selected.min(len - 1);
                self.palette_top = palette_top(selected, self.palette_top, len);
            }
            let (selected, top) = (self.palette_selected, self.palette_top);
            match shown {
                Completion::Path(popup) => path_popup::render(popup, selected, top, cx),
                Completion::Model(popup) => {
                    model_popup::render(popup, selected, top, self.current_model.as_ref(), cx)
                }
                Completion::Theme(popup) => {
                    theme_popup::render(popup, selected, top, &self.current_theme, cx)
                }
            }
        });
        let palette = if completion.is_none() {
            let total = self.palette(cx).len();
            if total > 0 {
                let selected = self.palette_selected.min(total - 1);
                self.palette_top = palette_top(selected, self.palette_top, total);
            }
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
            // A click anywhere else closes the popups.
            .on_mouse_down_out(cx.listener(|this, _: &gpui::MouseDownEvent, _, cx| {
                this.hide_popups(cx);
            }))
            .font_family(theme.mono_font_family.clone())
            .child(
                // The wheel over a popup moves its window of rows.
                v_flex()
                    .id("input-popups")
                    .debug_selector(|| "input-popups".to_owned())
                    .on_scroll_wheel(cx.listener(Self::popup_wheel))
                    .children(completion)
                    .children(palette)
                    .children(which_key),
            )
            .children(tray)
            .child(prompt)
            .children(status)
    }
}

#[cfg(test)]
mod tests;
