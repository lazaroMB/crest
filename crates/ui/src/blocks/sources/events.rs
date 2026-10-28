//! Requests that interactive blocks send to the rest of the UI.
//!
//! Block renderers are stateless functions of `&App`, so they cannot hold
//! an entity of their own. They emit through one app-wide [`BlockEvents`]
//! entity instead; the shell subscribes to it once with
//! `cx.subscribe(&block_events(cx), ...)`.

use domain::block::{BlockId, ImageData};
use domain::workspace::WorkspacePath;
use gpui::{App, AppContext as _, Entity, EventEmitter, Global};

/// What a block asks the UI to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockEvent {
    /// Open `path` of the workspace, at `line` when known. Same shape as
    /// [`crate::picker::PickerEvent::Open`], so both share one handler.
    OpenFile {
        /// File to open.
        path: WorkspacePath,
        /// 1-based line.
        line: Option<u64>,
    },
    /// `url` was opened in the browser.
    OpenUrl {
        /// Address.
        url: String,
    },
    /// Show the picture of the image block `block` in a modal, with zoom
    /// and pan.
    InspectImage {
        /// The block.
        block: BlockId,
        /// Its media type.
        media_type: String,
        /// Its bytes or file.
        data: ImageData,
    },
    /// Re-run the envelope of the computed block `block` and compare its
    /// values (the card's Recompute button).
    Recompute {
        /// The block.
        block: BlockId,
    },
}

/// The emitter of every [`BlockEvent`].
#[derive(Debug, Default)]
pub struct BlockEvents;

impl EventEmitter<BlockEvent> for BlockEvents {}

struct GlobalBlockEvents(Entity<BlockEvents>);

impl Global for GlobalBlockEvents {}

/// The app-wide emitter, created on first use.
pub fn block_events(cx: &mut App) -> Entity<BlockEvents> {
    if let Some(global) = cx.try_global::<GlobalBlockEvents>() {
        return global.0.clone();
    }
    let entity = cx.new(|_| BlockEvents);
    cx.set_global(GlobalBlockEvents(entity.clone()));
    entity
}

/// Emits `event` to every subscriber of [`block_events`].
pub fn emit(event: BlockEvent, cx: &mut App) {
    block_events(cx).update(cx, |_, cx| cx.emit(event));
}
