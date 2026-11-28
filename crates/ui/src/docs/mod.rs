//! The doc panel: a chat's document beside the conversation (or in its
//! place), with its page, its source editor and its diagram editor.
//!
//! ```text
//! ┌ v0.6.0 notes ▾ · the chat's title      saved  source  export ▾  split only hide ┐
//! │ <dir>/<file>.md                                                                 │
//! │ [toolbar]                                                                       │
//! │   the page (a live-preview markdown editor) or the source editor                │
//! └─────────────────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! The panel owns what it shows (the text, the assets, the chat's docs) and
//! asks the host to do everything else through [`DocPanelEvent`]: saving
//! (debounced 600 ms after the last key in the source), renaming, deleting,
//! a new doc, switching docs, exporting, the agent's passage, and the file
//! of a picked picture. The host answers with [`DocPanel::set_saved`] and
//! [`DocPanel::show`].
//!
//! Selection: on mouse-up over the page or the source the panel reads the
//! editor's selection back and floats a bar with Copy and "Ask agent"
//! ([`DocPanelEvent::AskAgent`]).
//!
//! Pictures: the toolbar's Image button opens the platform file picker
//! (`App::prompt_for_paths`), reads the file, stores it under a clean
//! unique name in the doc's assets, inserts `![stem](assets/<name>)` and
//! emits [`DocPanelEvent::WriteAsset`] for the host to write it.
//!
//! The page is a `zorite_editor::EditorState` live-preview markdown editor
//! ([`DocPanel::page_editor`], see [`page`]): the marks are hidden except on
//! the caret line. Typing in it, or in the source (a `Textarea` over the
//! same text), changes the doc's text and is saved after the debounce as
//! [`DocPanelEvent::Save`]; the toolbar acts on the selection of whichever
//! is showing. A diagram opens in the diagram editor from its "edit" button
//! or a double click and is written back in place of its block. Methods that
//! need a window (`show`, `open_diagram`, ...) are called without one by
//! the host, so they record what to do and the next frame does it.

pub mod diagram_editor;
pub mod export;
pub mod page;
pub mod source;
pub mod toolbar;

use std::ops::Range;
use std::time::{Duration, Instant};

use application_core::keymap::Action;
use domain::doc::{Doc, DocId, ExportFormat, Format, FormatExtra, block_at, diagram_source};
use domain::doc::{diagram_spans, format_selection, insert_diagram_after, replace_diagram};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, AppContext as _, ClipboardItem, Context, Entity, EventEmitter, FocusHandle,
    Focusable, InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton, MouseUpEvent,
    ParentElement as _, PathPromptOptions, Pixels, Point, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Task, Window, anchored, deferred,
    div, point, px,
};
use gpui_component::input::{Input, InputEvent, InputState, TextareaState};
use gpui_component::{ActiveTheme as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit_assets::IconName;
use zorite_editor::{EditorEvent, EditorState};

use crate::kit::{quiet_button, tool_button};
use crate::theme::token;
use diagram_editor::{DiagramEditor, DiagramEvent, DiagramSpec};
use toolbar::Tool;

/// Action that cycles the panel: hidden, split, alone.
/// How long the highlight of an added span lasts, in redraw steps of `ADDED_STEP`.
const ADDED_STEPS: u32 = 40;
const ADDED_STEP: Duration = Duration::from_millis(40);
/// The highlight fades over this long.
const ADDED_FADE: Duration = Duration::from_millis(1600);

pub const TOGGLE_ACTION: &str = "panel.doc";
/// Action that starts a new doc.
pub const NEW_ACTION: &str = "doc.new";
/// Action that switches between the page and the source.
pub const SOURCE_ACTION: &str = "doc.source";
/// Action that opens the export menu.
pub const EXPORT_ACTION: &str = "doc.export";
/// Action that opens the title's rename field.
pub const RENAME_ACTION: &str = "doc.rename";
/// Action that opens the delete confirmation.
pub const DELETE_ACTION: &str = "doc.delete";
/// Action that opens the menu of the chat's docs.
pub const SWITCH_ACTION: &str = "doc.switch";

/// Whether `name` is one of the actions [`DocPanel::handle_action`] runs
/// besides [`TOGGLE_ACTION`]: the head's, the toolbar's and the diagram editor's.
#[must_use]
pub fn is_panel_action(name: &str) -> bool {
    use diagram_editor::{
        CANCEL_ACTION, SAVE_ACTION, ZOOM_FIT_ACTION, ZOOM_IN_ACTION, ZOOM_OUT_ACTION,
    };
    matches!(
        name,
        NEW_ACTION
            | SOURCE_ACTION
            | EXPORT_ACTION
            | RENAME_ACTION
            | DELETE_ACTION
            | SWITCH_ACTION
            | SAVE_ACTION
            | CANCEL_ACTION
            | ZOOM_IN_ACTION
            | ZOOM_OUT_ACTION
            | ZOOM_FIT_ACTION
    ) || Tool::from_action_name(name).is_some()
}

/// Quiet time after the last key in the source before it is saved.
pub const SAVE_AFTER_MS: u64 = 600;
/// Largest picture the Image button reads, in bytes.
const MAX_IMAGE_BYTES: u64 = 20 * 1024 * 1024;

/// How the panel shares the window with the chat.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DocView {
    /// Not shown.
    #[default]
    Hidden,
    /// Beside the conversation.
    Split,
    /// In place of the conversation; the prompt stays.
    Alone,
}

impl DocView {
    /// The next view of the cycle hidden, split, alone.
    #[must_use]
    pub fn cycle(self) -> Self {
        match self {
            Self::Hidden => Self::Split,
            Self::Split => Self::Alone,
            Self::Alone => Self::Hidden,
        }
    }
}

/// Whether the doc's text reached its file.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum SavedState {
    /// The file has the text.
    #[default]
    Saved,
    /// A save is under way.
    Saving,
    /// The save failed, with the reason.
    Failed(String),
    /// The text changed and is waiting for the debounce.
    Dirty,
}

/// What the panel asks of the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocPanelEvent {
    /// Write this text as the doc's file.
    Save {
        /// The doc.
        id: DocId,
        /// Its whole markdown.
        text: String,
    },
    /// Rename the doc.
    Rename {
        /// The doc.
        id: DocId,
        /// The new title.
        title: String,
    },
    /// Delete the doc (the person confirmed).
    Delete(DocId),
    /// Start a new doc in this chat.
    New,
    /// Show another doc of the chat.
    Switch(DocId),
    /// Export the doc in a format.
    Export(ExportFormat),
    /// Send a selected passage of the page to the agent.
    AskAgent {
        /// The doc.
        id: DocId,
        /// The selected text.
        passage: String,
    },
    /// The view changed (the head's buttons, the `panel.doc` action).
    ViewChanged(DocView),
    /// The person asked for the chat back ("show chat").
    ShowChat,
    /// Esc left the source editor: the prompt takes the keyboard back.
    FocusPrompt,
    /// Store a picture in the doc's `assets/`.
    WriteAsset {
        /// File name inside `assets/`.
        name: String,
        /// The picture's bytes.
        bytes: Vec<u8>,
    },
}

/// What the body shows.
enum Mode {
    Page,
    Source,
    Diagram {
        editor: Entity<DiagramEditor>,
        /// Whether to go back to the source (else the page) on leaving.
        from_source: bool,
        _events: Subscription,
    },
}

/// The menu open under the head.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Menu {
    Export,
    Switch,
    ConfirmDelete,
}

/// The address prompt of the Link button.
struct LinkPrompt {
    input: Entity<InputState>,
    /// The selection the link wraps, taken when the prompt opened.
    range: (usize, usize),
    _events: Subscription,
}

/// The inline rename field of the title.
struct RenameField {
    input: Entity<InputState>,
    _events: Subscription,
}

/// The bar floating over a selection of the page.
#[derive(Debug, Clone, PartialEq)]
struct SelectionBar {
    at: Point<Pixels>,
    text: String,
}

/// The doc panel entity.
pub struct DocPanel {
    focus: FocusHandle,
    doc: Option<Doc>,
    text: String,
    /// Bumped on every change of the text, so cached diagram drawings follow.
    rev: u64,
    assets: Vec<(String, Vec<u8>)>,
    docs: Vec<Doc>,
    chat_title: Option<String>,
    saved: SavedState,
    author: Option<String>,
    view: DocView,
    mode: Mode,
    source: Entity<TextareaState>,
    /// The WYSIWYG editor of the page.
    page_editor: Entity<EditorState>,
    pending_source: Option<String>,
    pending_diagram: Option<(DiagramSpec, bool)>,
    /// The template the next new diagram opens on (index into
    /// `DIAGRAM_TEMPLATES`), consumed when the editor is created.
    pending_template: usize,
    pending_focus_source: bool,
    save_timer: Option<Task<()>>,
    menu: Option<Menu>,
    rename: Option<RenameField>,
    link: Option<LinkPrompt>,
    page: page::PageState,
    selection: Option<SelectionBar>,
    agent_line: Option<String>,
    notice: Option<String>,
    /// The span just added by "add to doc", lit while it fades, and when it began.
    added: Option<(Range<usize>, Instant)>,
    /// Redraws the page while the highlight fades.
    added_task: Option<Task<()>>,
    /// The page is yet to scroll to the added span (after the next paint).
    reveal_added: bool,
    _source_changes: Subscription,
    _page_events: Subscription,
}

impl EventEmitter<DocPanelEvent> for DocPanel {}

impl Focusable for DocPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl DocPanel {
    /// An empty panel, hidden.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let source = cx.new(|cx| {
            TextareaState::new(window, cx)
                .soft_wrap(true)
                .placeholder("Write markdown… (B, I, H and the toolbar format the selection)")
        });
        let changes = cx.subscribe(&source, |this, _, event, cx| {
            if matches!(event, InputEvent::Change) {
                this.source_changed(cx);
            }
        });
        let (page, page_editor) = page::PageState::new(window, cx);
        let page_events = cx.subscribe_in(
            &page_editor,
            window,
            |this, _, event: &EditorEvent, window, cx| match event {
                EditorEvent::Changed => {
                    this.page_changed(cx);
                    this.reveal_caret(window, cx);
                }
                EditorEvent::SelectionChanged => this.reveal_caret(window, cx),
                _ => {}
            },
        );
        Self {
            focus: cx.focus_handle(),
            doc: None,
            text: String::new(),
            rev: 0,
            assets: Vec::new(),
            docs: Vec::new(),
            chat_title: None,
            saved: SavedState::Saved,
            author: None,
            view: DocView::Hidden,
            mode: Mode::Page,
            source,
            page_editor,
            pending_source: None,
            pending_diagram: None,
            pending_template: 0,
            pending_focus_source: false,
            save_timer: None,
            menu: None,
            rename: None,
            link: None,
            page,
            selection: None,
            agent_line: None,
            notice: None,
            added: None,
            added_task: None,
            reveal_added: false,
            _source_changes: changes,
            _page_events: page_events,
        }
    }

    /// Shows `doc` with its `text` and the bytes of its `assets`. The
    /// editor state is reset; the source view stays open when the same doc
    /// is shown again (the agent changed it).
    pub fn show(
        &mut self,
        doc: Doc,
        text: String,
        assets: Vec<(String, Vec<u8>)>,
        cx: &mut Context<Self>,
    ) {
        let same = self.doc.as_ref().is_some_and(|d| d.id == doc.id);
        if !same {
            self.added = None;
            self.added_task = None;
            self.reveal_added = false;
        }
        if !(same && matches!(self.mode, Mode::Source)) {
            self.mode = Mode::Page;
        }
        self.page.shared.borrow_mut().set_assets(&assets);
        self.assets = assets;
        self.doc = Some(doc);
        self.rev += 1;
        self.text.clone_from(&text);
        self.pending_source = Some(text);
        // The same text leaves the editor (and its undo) alone: an agent
        // reload of what the page already shows.
        self.push_to_page_reloaded(same, cx);
        self.pending_diagram = None;
        self.saved = SavedState::Saved;
        self.save_timer = None;
        self.menu = None;
        self.rename = None;
        self.link = None;
        self.selection = None;
        self.notice = None;
        cx.notify();
    }

    /// Highlights `range` of the text for a moment and scrolls the page to
    /// it, as feedback to an "add to doc".
    pub fn flash_added(&mut self, range: Range<usize>, cx: &mut Context<Self>) {
        self.added = Some((range, Instant::now()));
        self.reveal_added = true;
        self.added_task = Some(cx.spawn(async move |this, cx| {
            for _ in 0..ADDED_STEPS {
                cx.background_executor().timer(ADDED_STEP).await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    return;
                }
            }
            this.update(cx, |this, cx| {
                this.added = None;
                this.added_task = None;
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// The span lit by the last "add to doc", while it is lit.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn added_range(&self) -> Option<Range<usize>> {
        self.added.as_ref().map(|(range, _)| range.clone())
    }

    /// Takes the docs of the chat, for the title's switcher.
    pub fn set_docs_of_chat(&mut self, docs: Vec<Doc>, cx: &mut Context<Self>) {
        self.docs = docs;
        cx.notify();
    }

    /// The chat's title, shown next to the doc's.
    pub fn set_chat_title(&mut self, title: Option<String>, cx: &mut Context<Self>) {
        self.chat_title = title;
        cx.notify();
    }

    /// Updates the saved state; `author` is who wrote the text last
    /// (`Some("agent")` says "edited by agent").
    pub fn set_saved(&mut self, state: SavedState, author: Option<&str>, cx: &mut Context<Self>) {
        self.saved = state;
        self.author = author.map(str::to_owned);
        cx.notify();
    }

    /// The saved state.
    #[must_use]
    pub fn saved(&self) -> &SavedState {
        &self.saved
    }

    /// The words the head shows for the saved state.
    #[must_use]
    pub fn saved_label(&self) -> String {
        match &self.saved {
            SavedState::Failed(reason) => format!("save failed: {reason}"),
            SavedState::Saving => "saving…".to_owned(),
            SavedState::Dirty => "unsaved".to_owned(),
            SavedState::Saved if self.author.as_deref() == Some("agent") => {
                "edited by agent".to_owned()
            }
            SavedState::Saved => "saved".to_owned(),
        }
    }

    /// Sets the view and tells the host.
    pub fn set_view(&mut self, view: DocView, cx: &mut Context<Self>) {
        if self.view != view {
            self.view = view;
            cx.emit(DocPanelEvent::ViewChanged(view));
        }
        cx.notify();
    }

    /// The view.
    #[must_use]
    pub fn view(&self) -> DocView {
        self.view
    }

    /// Hidden, split, alone, hidden...
    pub fn cycle_view(&mut self, cx: &mut Context<Self>) {
        self.set_view(self.view.cycle(), cx);
    }

    /// Switches between the page and the source.
    pub fn toggle_source(&mut self, cx: &mut Context<Self>) {
        if self.doc.is_none() {
            return;
        }
        match self.mode {
            Mode::Source => self.leave_source(cx),
            Mode::Page => {
                if self.source.read(cx).value() != self.text.as_str() {
                    self.pending_source = Some(self.text.clone());
                }
                self.mode = Mode::Source;
                self.selection = None;
                self.pending_focus_source = true;
                cx.notify();
            }
            Mode::Diagram { .. } => {}
        }
    }

    /// Gives the keyboard to the panel: the source editor in source mode, else
    /// the page.
    pub fn focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_source() {
            self.source.read(cx).focus_handle(cx).focus(window, cx);
        } else if let Some(editor) = self.diagram_editor().cloned() {
            editor.read(cx).code_focus(cx).focus(window, cx);
        } else if self.doc.is_some() {
            self.page_editor.read(cx).focus_handle(cx).focus(window, cx);
        } else {
            window.focus(&self.focus, cx);
        }
    }

    /// Whether the keyboard is in the panel: its source editor, the diagram
    /// editor, the title field or the link prompt.
    #[must_use]
    pub fn owns_focus(&self, window: &Window, cx: &App) -> bool {
        self.focus.contains_focused(window, cx)
            || self.source.read(cx).focus_handle(cx).is_focused(window)
            || self.page_focused(window, cx)
            || self
                .diagram_editor()
                .is_some_and(|editor| editor.read(cx).code_focus(cx).is_focused(window))
    }

    /// The source editor's focus handle.
    #[must_use]
    pub fn source_focus_handle(&self, cx: &App) -> FocusHandle {
        self.source.read(cx).focus_handle(cx)
    }

    /// Whether the source editor is showing.
    #[must_use]
    pub fn is_source(&self) -> bool {
        matches!(self.mode, Mode::Source)
    }

    /// Whether the diagram editor is open.
    #[must_use]
    pub fn is_diagram_open(&self) -> bool {
        matches!(self.mode, Mode::Diagram { .. })
    }

    /// The diagram editor, while open.
    #[must_use]
    pub fn diagram_editor(&self) -> Option<&Entity<DiagramEditor>> {
        match &self.mode {
            Mode::Diagram { editor, .. } => Some(editor),
            _ => None,
        }
    }

    /// Opens the diagram editor for a new diagram, after the block the
    /// cursor is in (the source's, or the page's).
    pub fn open_new_diagram(&mut self, cx: &mut Context<Self>) {
        if self.doc.is_none() {
            return;
        }
        let from_source = self.is_source();
        let after = if from_source {
            block_at(&self.text, self.source.read(cx).selected_range().end)
        } else {
            block_at(&self.text, self.page_editor.read(cx).cursor())
        };
        self.pending_diagram = Some((DiagramSpec::New { after }, from_source));
        cx.notify();
    }

    /// As [`Self::open_new_diagram`], starting from the template named
    /// `name` (the first one when there is none of that name).
    pub fn open_new_diagram_with(&mut self, name: &str, cx: &mut Context<Self>) {
        self.pending_template = domain::doc::DIAGRAM_TEMPLATES
            .iter()
            .position(|(template, _)| *template == name)
            .unwrap_or(0);
        self.open_new_diagram(cx);
    }

    /// Opens the diagram editor on the `index`th diagram of the doc.
    pub fn open_diagram(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.doc.is_none() || index >= diagram_spans(&self.text).len() {
            return;
        }
        let from_source = self.is_source();
        self.pending_diagram = Some((DiagramSpec::Edit { index }, from_source));
        cx.notify();
    }

    /// The line the host wants under the doc when it is alone (the agent's
    /// last message).
    pub fn set_last_agent_line(&mut self, line: Option<String>, cx: &mut Context<Self>) {
        self.agent_line = line;
        cx.notify();
    }

    /// The agent's last line, as given.
    #[must_use]
    pub fn last_agent_line(&self) -> Option<&str> {
        self.agent_line.as_deref()
    }

    /// The doc's markdown as the panel has it (including edits not saved yet).
    #[must_use]
    pub fn text(&self) -> String {
        self.text.clone()
    }

    /// The doc shown.
    #[must_use]
    pub fn doc(&self) -> Option<&Doc> {
        self.doc.as_ref()
    }

    /// The names of the doc's assets.
    #[must_use]
    pub fn asset_names(&self) -> Vec<String> {
        self.assets.iter().map(|(name, _)| name.clone()).collect()
    }

    /// The assets of the doc: names and bytes, for an export.
    #[must_use]
    pub fn assets(&self) -> &[(String, Vec<u8>)] {
        &self.assets
    }

    /// Runs one of the panel's actions; false for any other.
    pub fn handle_action(
        &mut self,
        action: &Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Action::Builtin(name) = action else {
            return false;
        };
        match name.as_str() {
            TOGGLE_ACTION => self.cycle_view(cx),
            NEW_ACTION => cx.emit(DocPanelEvent::New),
            SOURCE_ACTION => self.toggle_source(cx),
            EXPORT_ACTION => {
                if self.doc.is_some() {
                    self.menu = Some(Menu::Export);
                    cx.notify();
                }
            }
            RENAME_ACTION => {
                if self.doc.is_none() {
                    return false;
                }
                self.start_rename(window, cx);
            }
            DELETE_ACTION => {
                if self.doc.is_some() {
                    self.menu = Some(Menu::ConfirmDelete);
                    cx.notify();
                }
            }
            SWITCH_ACTION => {
                // The head shows the switcher from the second doc on.
                if self.docs.len() > 1 {
                    self.menu = Some(Menu::Switch);
                    cx.notify();
                }
            }
            other => {
                if let Some(tool) = Tool::from_action_name(other) {
                    if self.doc.is_some() {
                        self.run_tool(tool, window, cx);
                    }
                } else if let Mode::Diagram { editor, .. } = &self.mode {
                    let editor = editor.clone();
                    let handled = editor.update(cx, |e, cx| e.handle_action(other, cx));
                    if !handled {
                        return false;
                    }
                } else {
                    return false;
                }
            }
        }
        self.apply_pending(window, cx);
        true
    }

    // ---- text and saving ----

    /// A change of the text, from the person.
    fn edit_text(&mut self, text: String, cx: &mut Context<Self>) {
        self.text = text;
        self.rev += 1;
        self.saved = SavedState::Dirty;
        self.notice = None;
        self.save_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(SAVE_AFTER_MS))
                .await;
            this.update(cx, |this, cx| this.flush_save(cx)).ok();
        }));
        cx.notify();
    }

    fn source_changed(&mut self, cx: &mut Context<Self>) {
        let value = self.source.read(cx).value().to_string();
        if value != self.text {
            self.edit_text(value, cx);
        }
    }

    /// Asks the host to save now (the debounce ran out, or the person left
    /// the source).
    fn flush_save(&mut self, cx: &mut Context<Self>) {
        self.save_timer = None;
        let Some(doc) = &self.doc else { return };
        if self.saved != SavedState::Dirty {
            return;
        }
        let event = DocPanelEvent::Save {
            id: doc.id,
            text: self.text.clone(),
        };
        self.saved = SavedState::Saving;
        cx.emit(event);
        cx.notify();
    }

    fn leave_source(&mut self, cx: &mut Context<Self>) {
        self.flush_save(cx);
        self.push_to_page(cx);
        self.mode = Mode::Page;
        self.link = None;
        self.selection = None;
        cx.notify();
    }

    /// Does what the host's window-less calls recorded.
    fn apply_pending(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.pending_source.take() {
            self.source
                .update(cx, |state, cx| state.set_value(text, window, cx));
        }
        if let Some((spec, from_source)) = self.pending_diagram.take() {
            self.start_diagram(spec, from_source, window, cx);
        }
        if std::mem::take(&mut self.pending_focus_source) && self.is_source() {
            let focus = self.source.read(cx).focus_handle(cx);
            focus.focus(window, cx);
        }
    }

    /// Replaces the source editor's text, keeping its undo history, and
    /// selects `selection`.
    fn put_source(
        &mut self,
        text: String,
        selection: (usize, usize),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.edit_text(text.clone(), cx);
        self.source.update(cx, |state, cx| {
            state.replace_all(text, window, cx);
            state.set_selected_range(selection.0..selection.1, cx);
        });
        let focus = self.source.read(cx).focus_handle(cx);
        focus.focus(window, cx);
    }

    /// Whether the toolbar has an editor to act on: the source, or the page.
    fn can_format(&self) -> bool {
        matches!(self.mode, Mode::Source | Mode::Page) && self.doc.is_some()
    }

    /// The selection of the editor the toolbar acts on, as byte offsets.
    fn selection_range(&self, window: &mut Window, cx: &mut Context<Self>) -> (usize, usize) {
        if self.is_source() {
            let selected = self.source.read(cx).selected_range();
            (selected.start, selected.end)
        } else {
            self.page_selection(window, cx)
        }
    }

    /// Applies a formatting to `range` of the source or of the page (the
    /// editor's selection when `None`; on the page without one, the
    /// caret's line). Returns whether the text changed.
    pub fn apply_format(
        &mut self,
        format: Format,
        extra: &FormatExtra,
        range: Option<(usize, usize)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.can_format() {
            return false;
        }
        let (start, end) = match range {
            Some(range) => range,
            None => self.selection_range(window, cx),
        };
        let Some(result) = format_selection(&self.text, start, end, format, extra) else {
            return false;
        };
        if self.is_source() {
            self.put_source(result.markdown, (result.start, result.end), window, cx);
        } else {
            self.page_editor
                .update(cx, |editor, cx| editor.focus(window, cx));
            self.put_page(result.markdown, result.end, cx);
        }
        true
    }

    // ---- toolbar ----

    /// Runs a toolbar button.
    pub fn run_tool(&mut self, tool: Tool, window: &mut Window, cx: &mut Context<Self>) {
        match tool {
            Tool::Link => self.ask_link(window, cx),
            Tool::Image => self.pick_image(window, cx),
            Tool::Diagram => {
                self.open_new_diagram(cx);
                self.apply_pending(window, cx);
            }
            Tool::Bold | Tool::Italic if matches!(self.mode, Mode::Page) && self.doc.is_some() => {
                // The page editor toggles its own marks around the selection.
                let focus = self.page_editor.read(cx).focus_handle(cx);
                focus.focus(window, cx);
                if tool == Tool::Bold {
                    focus.dispatch_action(&zorite_editor::Bold, window, cx);
                } else {
                    focus.dispatch_action(&zorite_editor::Italic, window, cx);
                }
                self.page_changed(cx);
            }
            _ => {
                if let Some(format) = tool.format() {
                    self.apply_format(format, &FormatExtra::default(), None, window, cx);
                }
            }
        }
    }

    fn ask_link(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_format() {
            return;
        }
        let selected = self.selection_range(window, cx);
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("https://"));
        let events = cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.confirm_link(window, cx);
            }
        });
        let focus = input.read(cx).focus_handle(cx);
        focus.focus(window, cx);
        self.link = Some(LinkPrompt {
            input,
            range: selected,
            _events: events,
        });
        cx.notify();
    }

    /// Wraps the selection in a link to the typed address; an empty
    /// address does nothing.
    pub fn confirm_link(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(prompt) = self.link.take() else {
            return;
        };
        let url = prompt.input.read(cx).value().trim().to_owned();
        if url.is_empty() {
            self.focus_editor(window, cx);
            cx.notify();
            return;
        }
        let extra = FormatExtra {
            url: Some(url),
            ..FormatExtra::default()
        };
        self.apply_format(Format::Link, &extra, Some(prompt.range), window, cx);
        cx.notify();
    }

    /// Gives the keyboard back to the editor the toolbar acts on.
    fn focus_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_source() {
            self.source.read(cx).focus_handle(cx).focus(window, cx);
        } else if matches!(self.mode, Mode::Page) {
            self.page_editor.read(cx).focus_handle(cx).focus(window, cx);
        }
    }

    /// Whether the link prompt is open.
    #[must_use]
    pub fn is_asking_link(&self) -> bool {
        self.link.is_some()
    }

    /// Types an address into the open link prompt.
    pub fn set_link_address(&mut self, url: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(prompt) = &self.link {
            prompt
                .input
                .update(cx, |state, cx| state.set_value(url.to_owned(), window, cx));
        }
    }

    fn pick_image(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_format() {
            return;
        }
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Insert image".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let read = cx
                .background_executor()
                .spawn(async move {
                    let size = std::fs::metadata(&path)?.len();
                    if size > MAX_IMAGE_BYTES {
                        return Err(std::io::Error::other("the picture is larger than 20 MB"));
                    }
                    std::fs::read(&path)
                })
                .await;
            this.update_in(cx, |this, window, cx| match read {
                Ok(bytes) => {
                    this.insert_image(&name, bytes, window, cx);
                }
                Err(err) => {
                    this.notice = Some(format!("cannot read {name}: {err}"));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Stores `bytes` as a picture of the doc, named after `file_name`, and
    /// inserts its image line at the source's selection. Returns the name
    /// it is stored under, or `None` when the file is not a picture the
    /// page can draw or the source is not showing.
    pub fn insert_image(
        &mut self,
        file_name: &str,
        bytes: Vec<u8>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        if !self.can_format() || page::image_format(file_name).is_none() {
            self.notice = Some(format!("{file_name} is not a picture crest can show"));
            cx.notify();
            return None;
        }
        let name = source::asset_name(file_name, &self.asset_names());
        let extra = FormatExtra {
            url: None,
            alt: Some(source::file_stem_of(file_name)),
            name: Some(name.clone()),
        };
        self.assets.push((name.clone(), bytes.clone()));
        self.page.shared.borrow_mut().set_assets(&self.assets);
        self.apply_format(Format::Image, &extra, None, window, cx);
        cx.emit(DocPanelEvent::WriteAsset {
            name: name.clone(),
            bytes,
        });
        Some(name)
    }

    // ---- diagram editor ----

    fn start_diagram(
        &mut self,
        spec: DiagramSpec,
        from_source: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let initial = match spec {
            DiagramSpec::Edit { index } => diagram_spans(&self.text)
                .get(index)
                .map(|span| diagram_source(&self.text, span))
                .unwrap_or_default(),
            // A new diagram starts from the Flowchart template, as the mock does.
            DiagramSpec::New { .. } => domain::doc::DIAGRAM_TEMPLATES
                .get(std::mem::take(&mut self.pending_template))
                .map(|(_, source)| (*source).to_owned())
                .unwrap_or_default(),
        };
        let editor = cx.new(|cx| DiagramEditor::new(spec, &initial, window, cx));
        let events = cx.subscribe_in(
            &editor,
            window,
            |this, _, event: &DiagramEvent, window, cx| match event {
                DiagramEvent::Save(code) => this.finish_diagram(Some(code.clone()), window, cx),
                DiagramEvent::Cancel => this.finish_diagram(None, window, cx),
            },
        );
        let focus = editor.read(cx).code_focus(cx);
        focus.focus(window, cx);
        self.selection = None;
        self.mode = Mode::Diagram {
            editor,
            from_source,
            _events: events,
        };
        cx.notify();
    }

    /// Leaves the diagram editor: `Some(code)` puts it in the doc, `None`
    /// leaves the doc as it was.
    fn finish_diagram(
        &mut self,
        code: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Mode::Diagram {
            editor,
            from_source,
            ..
        } = std::mem::replace(&mut self.mode, Mode::Page)
        else {
            return;
        };
        let spec = editor.read(cx).spec();
        if let Some(code) = code {
            let next = match spec {
                DiagramSpec::Edit { index } => replace_diagram(&self.text, index, &code),
                DiagramSpec::New { after } => Some(insert_diagram_after(&self.text, after, &code)),
            };
            if let Some(next) = next.filter(|next| *next != self.text) {
                if from_source {
                    self.edit_text(next.clone(), cx);
                    self.pending_source = Some(next);
                } else {
                    // The caret goes where the diagram changed.
                    let caret = page::changed_region(&self.text, &next).0;
                    self.put_page(next, caret, cx);
                }
                self.flush_save(cx);
            }
        }
        if from_source {
            self.mode = Mode::Source;
            self.pending_focus_source = true;
        } else {
            self.page_editor.read(cx).focus_handle(cx).focus(window, cx);
        }
        self.apply_pending(window, cx);
        cx.notify();
    }

    // ---- the page editor ----

    /// The doc was shown again with `text` (already in `self.text`): the
    /// page editor takes it when it differs, keeping its undo history when
    /// it is the same text, and starts at the top for another doc.
    fn push_to_page_reloaded(&mut self, same_doc: bool, cx: &mut Context<Self>) {
        if self.page_editor.read(cx).text() == self.text.as_str() {
            return;
        }
        let text = self.text.clone();
        let at = if same_doc {
            self.page_editor.read(cx).cursor().min(text.len())
        } else {
            0
        };
        self.page_editor.update(cx, |editor, cx| {
            editor.set_text(text, cx);
            editor.set_cursor(at, cx);
        });
    }

    /// Scrolls the page by the least that brings the caret into view, once
    /// the editor has laid out the change.
    fn reveal_caret(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let editor = self.page_editor.clone();
        let scroll = self.page.scroll.clone();
        window.on_next_frame(move |_, cx| {
            let (Some(caret), view) = (editor.read(cx).caret_screen_bounds(), scroll.bounds())
            else {
                return;
            };
            let margin = px(24.);
            let mut offset = scroll.offset();
            if caret.bottom() + margin > view.bottom() {
                offset.y -= caret.bottom() + margin - view.bottom();
            } else if caret.top() < view.top() + margin {
                offset.y += (view.top() + margin - caret.top()).min(-offset.y);
            } else {
                return;
            }
            scroll.set_offset(offset);
        });
        let _ = cx;
    }

    /// The page editor's text, kept in step with [`Self::text`].
    fn page_text(&self, cx: &App) -> String {
        self.page_editor.read(cx).text().to_owned()
    }

    /// The person typed in the page: its text is the doc's text.
    fn page_changed(&mut self, cx: &mut Context<Self>) {
        let text = self.page_text(cx);
        if text != self.text {
            self.edit_text(text, cx);
        }
    }

    /// Gives the page editor the doc's text when it differs (after the
    /// source was edited, or the agent changed the doc).
    fn push_to_page(&mut self, cx: &mut Context<Self>) {
        if self.page_editor.read(cx).text() == self.text.as_str() {
            return;
        }
        let text = self.text.clone();
        let at = self.page_editor.read(cx).cursor().min(text.len());
        self.page_editor.update(cx, |editor, cx| {
            editor.set_text(text, cx);
            editor.set_cursor(at, cx);
        });
    }

    /// Puts `next` in the page editor as one undo step and in the doc's
    /// text (saved after the pause), with the caret at `caret`.
    fn put_page(&mut self, next: String, caret: usize, cx: &mut Context<Self>) {
        let (start, old_end, new_end) = page::changed_region(&self.text, &next);
        let piece = next[start..new_end].to_owned();
        self.page_editor.update(cx, |editor, cx| {
            if editor.text() == self.text.as_str() {
                editor.replace_range(start..old_end, &piece, cx);
            } else {
                editor.set_text(next.clone(), cx);
            }
            editor.set_cursor(caret, cx);
        });
        self.edit_text(next, cx);
    }

    /// The page's selection as byte offsets (equal at the caret): the text
    /// the editor copies, placed next to its caret.
    fn page_selection(&self, window: &mut Window, cx: &mut Context<Self>) -> (usize, usize) {
        let editor = self.page_editor.clone();
        let (text, caret) = {
            let state = editor.read(cx);
            (
                state.text().to_owned(),
                state.cursor().min(state.text().len()),
            )
        };
        let selected = self.page.selected_text(&editor, window, cx);
        let len = selected.len();
        if len == 0 {
            return (caret, caret);
        }
        if caret >= len && text.get(caret - len..caret) == Some(selected.as_str()) {
            (caret - len, caret)
        } else if text.get(caret..caret + len) == Some(selected.as_str()) {
            (caret, caret + len)
        } else {
            (caret, caret)
        }
    }

    /// The caret goes to the end of the page (a click under its text).
    fn caret_to_end(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(self.mode, Mode::Page) {
            return;
        }
        let end = self.page_editor.read(cx).text().len();
        self.page_editor.update(cx, |editor, cx| {
            editor.set_cursor(end, cx);
            editor.focus(window, cx);
        });
    }

    /// Copies the text selected in the page to the clipboard; false
    /// without one (the app's copy keys call this first).
    pub fn copy_page_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !matches!(self.mode, Mode::Page) || !self.page_focused(window, cx) {
            return false;
        }
        let editor = self.page_editor.clone();
        let text = self.page.selected_text(&editor, window, cx);
        if text.is_empty() {
            return false;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        true
    }

    /// Whether the page editor has the keyboard.
    #[must_use]
    pub fn page_focused(&self, window: &Window, cx: &App) -> bool {
        self.page_editor
            .read(cx)
            .focus_handle(cx)
            .is_focused(window)
    }

    /// The page editor, for the host and tests.
    #[must_use]
    pub fn page_editor(&self) -> &Entity<EditorState> {
        &self.page_editor
    }

    /// The release of the left button on the page: the selection bar for a
    /// selected passage; a double click inside a diagram's block opens the
    /// diagram editor.
    fn page_mouse_up(&mut self, event: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(self.mode, Mode::Page) {
            return;
        }
        let editor = self.page_editor.clone();
        let text = self
            .page
            .selected_text(&editor, window, cx)
            .trim()
            .to_owned();
        self.set_selection_bar(text, event.position, cx);
        if event.click_count == 2 {
            self.open_diagram_at_caret(cx);
        }
    }

    /// The release of the left button on the source: the selection bar for
    /// the passage selected in the textarea, none for a caret or blanks.
    fn source_mouse_up(
        &mut self,
        event: &MouseUpEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.source_selection_changed(event.position, cx);
    }

    /// Reads the source's selection and shows the bar at `at` for it (or
    /// hides the bar when nothing but blanks is selected).
    pub(crate) fn source_selection_changed(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) {
        if !self.is_source() {
            return;
        }
        let text = self.source.read(cx).selected_text().to_string();
        self.set_selection_bar(text.trim().to_owned(), at, cx);
    }

    fn set_selection_bar(&mut self, text: String, at: Point<Pixels>, cx: &mut Context<Self>) {
        let next = (!text.is_empty()).then_some(SelectionBar { at, text });
        if next != self.selection {
            self.selection = next;
            cx.notify();
        }
    }

    /// Opens the diagram editor on the diagram the caret is in, if any.
    pub fn open_diagram_at_caret(&mut self, cx: &mut Context<Self>) {
        let caret = self.page_editor.read(cx).cursor();
        let spans = diagram_spans(&self.text);
        if let Some(index) = spans
            .iter()
            .position(|span| span.start <= caret && caret <= span.end)
        {
            self.open_diagram(index, cx);
        }
    }

    // ---- page selection ----

    /// The passage selected on the page, while the bar shows.
    #[must_use]
    pub fn selected_passage(&self) -> Option<&str> {
        self.selection.as_ref().map(|bar| bar.text.as_str())
    }

    /// Sends the selected passage to the agent.
    pub fn ask_agent(&mut self, cx: &mut Context<Self>) {
        let (Some(bar), Some(doc)) = (self.selection.take(), &self.doc) else {
            return;
        };
        cx.emit(DocPanelEvent::AskAgent {
            id: doc.id,
            passage: bar.text,
        });
        cx.notify();
    }

    fn copy_selection(&mut self, cx: &mut Context<Self>) {
        if let Some(bar) = self.selection.take() {
            cx.write_to_clipboard(ClipboardItem::new_string(bar.text));
            cx.notify();
        }
    }

    // ---- title and menus ----

    fn start_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = &self.doc else { return };
        let title = doc.title.clone();
        let input = cx.new(|cx| InputState::new(window, cx).default_value(title));
        let events =
            cx.subscribe_in(
                &input,
                window,
                |this, _, event: &InputEvent, _, cx| match event {
                    InputEvent::PressEnter { .. } => this.confirm_rename(cx),
                    InputEvent::Blur => {
                        this.rename = None;
                        cx.notify();
                    }
                    _ => {}
                },
            );
        let focus = input.read(cx).focus_handle(cx);
        focus.focus(window, cx);
        self.rename = Some(RenameField {
            input,
            _events: events,
        });
        cx.notify();
    }

    fn confirm_rename(&mut self, cx: &mut Context<Self>) {
        let (Some(field), Some(doc)) = (self.rename.take(), &self.doc) else {
            return;
        };
        let title = field.input.read(cx).value().trim().to_owned();
        if !title.is_empty() && title != doc.title {
            cx.emit(DocPanelEvent::Rename { id: doc.id, title });
        }
        cx.notify();
    }

    /// Whether the title is being renamed.
    #[must_use]
    pub fn is_renaming(&self) -> bool {
        self.rename.is_some()
    }

    fn toggle_menu(&mut self, menu: Menu, cx: &mut Context<Self>) {
        self.menu = if self.menu == Some(menu) {
            None
        } else {
            Some(menu)
        };
        cx.notify();
    }

    fn export(&mut self, format: ExportFormat, cx: &mut Context<Self>) {
        self.menu = None;
        cx.emit(DocPanelEvent::Export(format));
        cx.notify();
    }

    /// The panel's keys: Esc steps back (menu, prompt, rename, selection
    /// bar, source), and in the source Ctrl/Cmd+B, I and K format.
    fn key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if matches!(self.mode, Mode::Diagram { .. }) {
            return false;
        }
        let stroke = &event.keystroke;
        let key = stroke.key.as_str();
        if key == "escape" {
            // One step back per press: the first of these that is open closes.
            let closed_overlay = self.menu.take().is_some()
                || self.selection.take().is_some()
                || self.rename.take().is_some();
            if closed_overlay {
            } else if self.link.take().is_some() {
                self.focus_editor(window, cx);
            } else if self.is_source() {
                self.leave_source(cx);
                window.focus(&self.focus, cx);
                cx.emit(DocPanelEvent::FocusPrompt);
            } else if self.page_focused(window, cx) {
                // The page gives the keyboard back to the prompt.
                self.flush_save(cx);
                window.focus(&self.focus, cx);
                cx.emit(DocPanelEvent::FocusPrompt);
            } else {
                return false;
            }
            cx.notify();
            return true;
        }
        let in_page = matches!(self.mode, Mode::Page) && self.page_focused(window, cx);
        if (self.is_source() || in_page) && stroke.modifiers.secondary() && !stroke.modifiers.alt {
            match key {
                // The page editor binds B and I itself (its Bold and Italic).
                "b" if !in_page => self.run_tool(Tool::Bold, window, cx),
                "i" if !in_page => self.run_tool(Tool::Italic, window, cx),
                "k" => self.run_tool(Tool::Link, window, cx),
                _ => return false,
            }
            return true;
        }
        false
    }

    // ---- drawing ----

    fn head(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (muted, danger, border, fg) = (
            theme.muted_foreground,
            theme.danger,
            theme.border,
            theme.foreground,
        );
        let hover_bg = theme.list_active;
        let accent = token(cx, "accent.bg");
        let glyph = |icon: IconName| Icon::new(icon).xsmall().text_color(muted);
        let mut lead = h_flex().gap_1p5().items_center().min_w_0().flex_1();
        match (&self.doc, &self.rename) {
            (Some(_), Some(field)) => {
                lead = lead
                    .child(glyph(IconName::FileText))
                    .child(div().w(px(220.)).child(Input::new(&field.input)));
            }
            (Some(doc), None) => {
                lead = lead.child(glyph(IconName::FileText)).child(
                    div()
                        .id("doc-title")
                        .debug_selector(|| "doc-title".to_owned())
                        .flex_shrink(1.0)
                        .min_w(px(60.))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .px_1()
                        .py_0p5()
                        .rounded_sm()
                        .cursor_pointer()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(fg)
                        .hover(move |style| style.bg(hover_bg))
                        .tooltip(|window, cx| {
                            gpui_component::tooltip::Tooltip::new("Rename").build(window, cx)
                        })
                        .child(SharedString::from(doc.title.clone()))
                        .on_click(cx.listener(|this, _, window, cx| this.start_rename(window, cx))),
                );
                if self.docs.len() > 1 {
                    lead = lead.child(
                        toolbar::icon_tool(
                            "doc-switch",
                            Icon::new(gpui_component::IconName::ChevronDown),
                            "Documents of this chat",
                            false,
                            cx,
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_menu(Menu::Switch, cx))),
                    );
                }
                if let Some(chat) = &self.chat_title {
                    let mut chat_title = div()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_xs()
                        .text_color(muted)
                        .child(SharedString::from(format!("· {chat}")));
                    // The chat's title gives way to the document's.
                    chat_title.style().flex_shrink = Some(100.0);
                    lead = lead.child(chat_title);
                }
            }
            (None, _) => lead = lead.child(div().text_color(muted).child("Docs")),
        }
        let text_button = |id: &'static str, icon: IconName, label: &'static str| {
            h_flex()
                .id(id)
                .debug_selector(move || id.to_owned())
                .gap_1()
                .items_center()
                .flex_none()
                .h(px(24.))
                .px_1p5()
                .rounded_sm()
                .cursor_pointer()
                .text_xs()
                .text_color(muted)
                .hover(move |style| style.text_color(fg).bg(hover_bg))
                .child(Icon::new(icon).xsmall())
                .child(label)
        };
        let mut tools = h_flex().gap_1().items_center().flex_none();
        if self.doc.is_some() {
            let failed = matches!(self.saved, SavedState::Failed(_));
            let by_agent = self.saved_label().starts_with("edited by");
            tools = tools
                .child(
                    div()
                        .id("doc-saved")
                        .debug_selector(|| "doc-saved".to_owned())
                        .mr_1()
                        .text_xs()
                        .text_color(if failed {
                            danger
                        } else if by_agent {
                            accent
                        } else {
                            muted
                        })
                        .child(SharedString::from(if failed {
                            "save failed".to_owned()
                        } else {
                            self.saved_label()
                        }))
                        .when(failed, |el| {
                            let reason = SharedString::from(self.saved_label());
                            el.tooltip(move |window, cx| {
                                gpui_component::tooltip::Tooltip::new(reason.clone())
                                    .build(window, cx)
                            })
                        }),
                )
                .child(
                    if self.is_source() {
                        text_button("doc-source-toggle", IconName::Eye, "preview")
                    } else {
                        text_button("doc-source-toggle", IconName::FileCode, "source")
                    }
                    .tooltip(|window, cx| {
                        gpui_component::tooltip::Tooltip::new("Toggle the markdown source")
                            .build(window, cx)
                    })
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_source(cx))),
                )
                .child(
                    text_button("doc-export", IconName::Download, "export")
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_menu(Menu::Export, cx))),
                )
                .child(toolbar::rule(cx));
        }
        let view = self.view;
        tools = tools
            .child(
                toolbar::icon_tool(
                    "doc-view-split",
                    Icon::new(IconName::Columns2),
                    "Split with the chat",
                    view == DocView::Split,
                    cx,
                )
                .on_click(cx.listener(|this, _, _, cx| this.set_view(DocView::Split, cx))),
            )
            .child(
                toolbar::icon_tool(
                    "doc-view-alone",
                    Icon::new(IconName::Maximize2),
                    "Doc alone",
                    view == DocView::Alone,
                    cx,
                )
                .on_click(cx.listener(|this, _, _, cx| this.set_view(DocView::Alone, cx))),
            )
            .child(
                toolbar::icon_tool(
                    "doc-view-hide",
                    Icon::new(gpui_component::IconName::Close),
                    "Hide the doc",
                    false,
                    cx,
                )
                .on_click(cx.listener(|this, _, _, cx| this.set_view(DocView::Hidden, cx))),
            );
        let path = self.doc.as_ref().map(Doc::path);
        v_flex()
            .border_b_1()
            .border_color(border)
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .min_h(px(34.))
                    .pl_2p5()
                    .pr_1p5()
                    .child(lead)
                    .child(tools),
            )
            .children(path.map(|path| {
                div()
                    .id("doc-path")
                    .debug_selector(|| "doc-path".to_owned())
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .pl(px(28.))
                    .pr_2()
                    .pb_1()
                    .text_xs()
                    .text_color(muted)
                    .child(truncate_left(&path, 64))
            }))
            .children(self.notice.clone().map(|notice| {
                div()
                    .px_2()
                    .pb_1()
                    .text_xs()
                    .text_color(danger)
                    .child(SharedString::from(notice))
            }))
            .into_any_element()
    }

    fn menu_view(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.menu?;
        let theme = cx.theme();
        let (bg, fg, border, muted) = (
            theme.popover,
            theme.popover_foreground,
            theme.border,
            theme.muted_foreground,
        );
        let mut column = v_flex()
            .id("doc-menu")
            .debug_selector(|| "doc-menu".to_owned())
            .gap_0p5()
            .p_1()
            .min_w(px(200.))
            .rounded_md()
            .border_1()
            .border_color(border)
            .bg(bg)
            .text_color(fg)
            .shadow_md()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
        match menu {
            Menu::Export => {
                column = column.child(div().px_1().text_xs().text_color(muted).child("Export"));
                for (id, format, label) in [
                    ("doc-export-md", ExportFormat::Markdown, "Markdown (.md)"),
                    ("doc-export-html", ExportFormat::Html, "Web page (.html)"),
                    ("doc-export-pdf", ExportFormat::Pdf, "PDF (print dialog)"),
                    ("doc-export-copy", ExportFormat::Copy, "Copy as markdown"),
                ] {
                    column = column.child(
                        quiet_button(id, label, cx)
                            .on_click(cx.listener(move |this, _, _, cx| this.export(format, cx))),
                    );
                }
            }
            Menu::Switch => {
                column = column.child(
                    div()
                        .px_1()
                        .text_xs()
                        .text_color(muted)
                        .child("Docs of this chat"),
                );
                let current = self.doc.as_ref().map(|d| d.id);
                for (ix, doc) in self.docs.iter().enumerate() {
                    let id = doc.id;
                    let label = if Some(id) == current {
                        format!("✓ {}", doc.title)
                    } else {
                        doc.title.clone()
                    };
                    column = column.child(
                        div()
                            .id(("doc-switch-row", ix))
                            .px_2()
                            .py_0p5()
                            .rounded_sm()
                            .cursor_pointer()
                            .hover(move |style| style.bg(muted.opacity(0.2)))
                            .child(SharedString::from(label))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.menu = None;
                                cx.emit(DocPanelEvent::Switch(id));
                                cx.notify();
                            })),
                    );
                }
                column = column
                    .child(quiet_button("doc-new", "New doc", cx).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.menu = None;
                            cx.emit(DocPanelEvent::New);
                            cx.notify();
                        },
                    )))
                    .child(quiet_button("doc-delete", "Delete this doc", cx).on_click(
                        cx.listener(|this, _, _, cx| this.toggle_menu(Menu::ConfirmDelete, cx)),
                    ));
            }
            Menu::ConfirmDelete => {
                let title = self
                    .doc
                    .as_ref()
                    .map(|d| d.title.clone())
                    .unwrap_or_default();
                column = column
                    .child(
                        div()
                            .px_1()
                            .child(SharedString::from(format!("Delete “{title}”?"))),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .child(tool_button("doc-delete-confirm", "Delete", cx).on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.menu = None;
                                    if let Some(doc) = &this.doc {
                                        cx.emit(DocPanelEvent::Delete(doc.id));
                                    }
                                    cx.notify();
                                }),
                            ))
                            .child(quiet_button("doc-delete-cancel", "Cancel", cx).on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.menu = None;
                                    cx.notify();
                                }),
                            )),
                    );
            }
        }
        Some(
            deferred(div().absolute().top(px(34.)).right(px(8.)).child(column))
                .with_priority(2)
                .into_any_element(),
        )
    }

    fn selection_view(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let bar = self.selection.clone()?;
        let theme = cx.theme();
        let (bg, fg, border) = (theme.popover, theme.popover_foreground, theme.border);
        let chip = h_flex()
            .id("doc-selection-bar")
            .debug_selector(|| "doc-selection-bar".to_owned())
            .gap_1()
            .p_1()
            .rounded_md()
            .border_1()
            .border_color(border)
            .bg(bg)
            .text_color(fg)
            .shadow_md()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_up(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                quiet_button("doc-selection-copy", "Copy", cx)
                    .on_click(cx.listener(|this, _, _, cx| this.copy_selection(cx))),
            )
            .child(
                tool_button("doc-selection-ask", "Ask agent", cx)
                    .on_click(cx.listener(|this, _, _, cx| this.ask_agent(cx))),
            );
        Some(
            deferred(
                anchored()
                    .position(bar.at + point(px(8.), px(12.)))
                    .snap_to_window_with_margin(px(8.))
                    .child(chip),
            )
            .with_priority(1)
            .into_any_element(),
        )
    }

    fn empty_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_2()
            .p_4()
            .text_color(muted)
            .child(div().child("No doc in this chat yet"))
            .child(div().text_xs().child(
                "Add a block with its copy menu, start one empty and write it, or ask the agent.",
            ))
            .child(
                quiet_button("doc-new-empty", "New doc", cx)
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(DocPanelEvent::New))),
            )
            .into_any_element()
    }
}

/// `path` shortened from the left with an ellipsis when it is longer than
/// `max` characters, so the file name stays visible.
fn truncate_left(path: &str, max: usize) -> SharedString {
    let count = path.chars().count();
    if count <= max {
        return SharedString::from(path.to_owned());
    }
    let tail: String = path.chars().skip(count - (max - 1)).collect();
    SharedString::from(format!("…{tail}"))
}

impl Render for DocPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.apply_pending(window, cx);
        let diagram = self.diagram_editor().cloned();
        let body = if self.doc.is_none() {
            self.empty_view(cx)
        } else if let Some(editor) = diagram {
            editor.into_any_element()
        } else if self.is_source() {
            source::render(self, cx)
        } else {
            page::render(self, window, cx)
        };
        let head = self.head(cx);
        let menu = self.menu_view(cx);
        let selection = self.selection_view(cx);
        let (bg, fg) = (cx.theme().background, cx.theme().foreground);
        v_flex()
            .id("doc-panel")
            .debug_selector(|| "doc-panel".to_owned())
            .track_focus(&self.focus)
            .relative()
            .size_full()
            .min_w_0()
            .bg(bg)
            .text_color(fg)
            .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if this.key(event, window, cx) {
                    cx.stop_propagation();
                }
            }))
            .child(head)
            .child(div().flex_1().min_h_0().child(body))
            .children(menu)
            .children(selection)
    }
}

#[cfg(test)]
mod tests;
