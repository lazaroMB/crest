//! The keys each panel handles by itself, outside the keymap: a static
//! table the help lists under the keymap, one group per panel. They cannot
//! be remapped (yet); keep the table in step with the panels' handlers and
//! with the tables of `docs/usage.md`.

use super::{BUILTIN_SOURCE, KeyRow};

/// The action column of a panel key: it is fixed, not a keymap action.
pub const FIXED_ACTION: &str = "fixed";

/// Panel, keys and what they do.
pub const PANEL_KEYS: &[(&str, &str, &str)] = &[
    // The prompt.
    (
        "Prompt",
        "Up / Down",
        "On the first (last) line: the line sent before (after)",
    ),
    (
        "Prompt",
        "Up",
        "In an empty prompt while a turn runs: take the last queued prompt back",
    ),
    ("/ palette and @ paths", "Up / Down", "Choose a row"),
    (
        "/ palette and @ paths",
        "Tab",
        "Complete the command, or the directory or file",
    ),
    (
        "/ palette and @ paths",
        "Enter",
        "Run the command (or complete it up to its argument)",
    ),
    ("/ palette and @ paths", "Esc", "Hide the list"),
    // The chat.
    (
        "Chat (with the focus)",
        "Up / Down, k / j",
        "Select the previous or next block",
    ),
    (
        "Chat (with the focus)",
        "Home / End, g / G",
        "Select the first or last block",
    ),
    (
        "Chat (with the focus)",
        "Enter, o",
        "Expand or collapse the selected block",
    ),
    (
        "Chat (with the focus)",
        "t",
        "Switch the block between chart and table",
    ),
    ("Chat (with the focus)", "y", "Copy the selected block"),
    (
        "Chat (with the focus)",
        "PageUp / PageDown",
        "Scroll a page",
    ),
    (
        "Chat (with the focus)",
        "Esc",
        "Close the outline, else go back to the prompt",
    ),
    (
        "Chat (with the focus)",
        "Other letters",
        "Typed into the prompt",
    ),
    // Block widgets, once they have the keys.
    (
        "Block widgets (a click, or <leader><CR>)",
        "+ / - / 0",
        "Zoom in, out, fit (maps, drawings, diagrams, images, graphs, 3D models, waveforms, timelines)",
    ),
    (
        "Block widgets (a click, or <leader><CR>)",
        "Ctrl/Cmd + wheel",
        "Zoom about the pointer (a plain wheel scrolls the chat)",
    ),
    (
        "Block widgets (a click, or <leader><CR>)",
        "Drag, Shift + wheel",
        "Pan",
    ),
    (
        "Block widgets (a click, or <leader><CR>)",
        "Up / Down, Home / End, Enter",
        "List widget: move the selection, open the row",
    ),
    (
        "Block widgets (a click, or <leader><CR>)",
        "i, u, s, f, [ / ], m",
        "3D model: isometric, up axis, section plane (cycle, flip, move), measure",
    ),
    (
        "Block widgets (a click, or <leader><CR>)",
        "Esc",
        "Clear the cursor, selection or zoom; then give the keys back to the chat",
    ),
    // Side panels.
    ("Chats sidebar", "Up / Down, k / j", "Move the selection"),
    ("Chats sidebar", "Home / End", "First or last row"),
    (
        "Chats sidebar",
        "Enter",
        "Open the chat, or fold the project",
    ),
    (
        "Chats sidebar",
        "Left / Right",
        "Fold or unfold the project (Left on a chat: its project)",
    ),
    ("Chats sidebar", "F2, r", "Rename the chat"),
    ("Chats sidebar", "Delete", "Ask to delete the chat"),
    (
        "Chats sidebar",
        "Enter, y",
        "Confirm the delete (Esc cancels)",
    ),
    ("Workspace tree", "Up / Down", "Move the selection"),
    (
        "Workspace tree",
        "Left / Right",
        "Close or open the directory",
    ),
    (
        "Workspace tree",
        "Enter",
        "Reference the file in the prompt",
    ),
    ("Workspace tree", "Esc, typing", "Go back to the prompt"),
    // Pickers and dialogs.
    ("File and text pickers", "Up / Down", "Move the selection"),
    (
        "File and text pickers",
        "Enter",
        "Insert the reference into the prompt",
    ),
    ("File and text pickers", "Esc", "Close"),
    ("File and text pickers", "Alt+P", "Show or hide the preview"),
    (
        "File and text pickers",
        "Alt+C / Alt+R / Alt+W",
        "Text search: match case, regex, whole word",
    ),
    ("Project picker", "Up / Down", "Choose a row"),
    (
        "Project picker",
        "Enter",
        "Open the chosen project (or the typed path)",
    ),
    (
        "Project picker",
        "Ctrl+Enter",
        "Open the directory being listed",
    ),
    ("Project picker", "Tab, Right", "Enter the chosen directory"),
    (
        "Project picker",
        "Left",
        "With no filter: go up to the parent",
    ),
    (
        "Project picker",
        "Esc",
        "Back to the project you left (after /project)",
    ),
    ("Approval dialog", "y", "Approve"),
    ("Approval dialog", "a", "Approve always"),
    (
        "Approval dialog",
        "n",
        "Reject with a reason (Enter sends it, Esc drops it)",
    ),
    ("Approval dialog", "Esc", "Reject without a reason"),
    (
        "Approval dialog",
        "Up / Down, PageUp / PageDown",
        "Scroll the preview",
    ),
    ("Forms", "Tab / Shift+Tab", "Move between the fields"),
    ("Forms", "Enter", "Submit (in a one-line field)"),
    ("Forms", "Space", "Toggle a checkbox, pick a radio button"),
    ("Forms", "Esc", "Cancel (twice when something was typed)"),
    ("Help", "1–8, Tab / Shift+Tab", "Go to a section"),
    ("Help", "Up / Down, j / k", "Move"),
    (
        "Help",
        "Shift+Up / Shift+Down, Ctrl+D / Ctrl+U",
        "Scroll without moving",
    ),
    ("Help", "Enter", "Expand the row"),
    ("Help", "y", "Copy the row's command or name"),
    ("Help", "/", "Search"),
    ("Help", "Esc", "Close"),
    ("/login", "Enter", "Check the key"),
    ("/login", "Esc", "Close (also while the key is checked)"),
];

/// The rows of [`PANEL_KEYS`], grouped by panel.
pub fn panel_key_rows() -> Vec<KeyRow> {
    PANEL_KEYS
        .iter()
        .map(|(panel, keys, description)| KeyRow {
            keys: (*keys).to_owned(),
            action: FIXED_ACTION.to_owned(),
            description: (*description).to_owned(),
            source: BUILTIN_SOURCE.to_owned(),
            overridden: false,
            panel: Some((*panel).to_owned()),
        })
        .collect()
}
