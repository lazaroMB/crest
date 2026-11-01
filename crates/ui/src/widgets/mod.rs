//! Shared widgets: a timeline of spans, a filterable list, a node-link
//! graph and a tree table.
//!
//! Each widget draws one spec of [`domain::widget`] and keeps its state
//! (zoom, filter, open rows) in an entity per id, through
//! [`crate::blocks::block_state`], so it survives the timeline dropping
//! off-screen rows. `Custom` block nodes use them
//! ([`crate::blocks::custom`]), and domain blocks (traces, logs, plans,
//! waveforms) call the same `view`/`render` functions with a spec their
//! parser built.

pub mod graph;
pub mod list;
pub mod timeline;
pub mod tree;

use gpui::{App, Entity, Global, Hsla, WeakEntity, Window};
use gpui_component::input::InputState;

/// Theme color of palette slot `slot`, cycling the five chart colors.
pub fn palette(slot: usize, cx: &App) -> Hsla {
    crate::blocks::chart::series_color(slot, cx)
}

/// Text inputs of widgets (a list's filter), to tell whether one has the
/// focus.
#[derive(Default)]
struct TextInputs(Vec<WeakEntity<InputState>>);

impl Global for TextInputs {}

/// Registers a widget's text input, so [`owns_focus`] sees it.
pub(crate) fn register_input(input: &Entity<InputState>, cx: &mut App) {
    let inputs = &mut cx.default_global::<TextInputs>().0;
    inputs.retain(|weak| weak.upgrade().is_some());
    inputs.push(input.downgrade());
}

/// Whether the focus is in a widget's text input. The session then leaves
/// the keys to it instead of typing them in the prompt, and the timeline
/// does not take arrows and Enter for block navigation.
pub fn owns_focus(window: &Window, cx: &App) -> bool {
    cx.try_global::<TextInputs>().is_some_and(|inputs| {
        inputs
            .0
            .iter()
            .filter_map(WeakEntity::upgrade)
            .any(|input| {
                use gpui::Focusable as _;
                input.read(cx).focus_handle(cx).contains_focused(window, cx)
            })
    })
}
