//! Where each measured region of the window is, so the tour can point at it.

use gpui::{App, Bounds, Global, IntoElement, Pixels, Styled as _, canvas, px};

use super::state::Anchor;

/// The last bounds each anchor reported, in window pixels.
#[derive(Debug, Default, Clone)]
pub struct Frames {
    /// The New chat row.
    pub new_chat: Option<Bounds<Pixels>>,
    /// The chats sidebar.
    pub chats: Option<Bounds<Pixels>>,
    /// The timeline.
    pub timeline: Option<Bounds<Pixels>>,
    /// The prompt.
    pub prompt: Option<Bounds<Pixels>>,
}

impl Global for Frames {}

impl Frames {
    fn slot(&mut self, anchor: Anchor) -> &mut Option<Bounds<Pixels>> {
        match anchor {
            Anchor::NewChat => &mut self.new_chat,
            Anchor::Chats => &mut self.chats,
            Anchor::Timeline => &mut self.timeline,
            Anchor::Prompt => &mut self.prompt,
        }
    }

    /// The bounds of `anchor`, if it has been painted.
    #[must_use]
    pub fn get(&self, anchor: Anchor) -> Option<Bounds<Pixels>> {
        match anchor {
            Anchor::NewChat => self.new_chat,
            Anchor::Chats => self.chats,
            Anchor::Timeline => self.timeline,
            Anchor::Prompt => self.prompt,
        }
    }
}

/// An invisible layer that records the bounds of its parent for `anchor`.
/// The parent has to be `relative`.
pub fn probe(anchor: Anchor) -> gpui::AnyElement {
    canvas(
        move |bounds, window, cx| {
            let frames = cx.default_global::<Frames>();
            let slot = frames.slot(anchor);
            if shifted(*slot, bounds) {
                *slot = Some(bounds);
                window.refresh();
            }
        },
        |_, _, _, _| {},
    )
    .absolute()
    .top(px(0.))
    .left(px(0.))
    .size_full()
    .into_any_element()
}

fn shifted(prev: Option<Bounds<Pixels>>, next: Bounds<Pixels>) -> bool {
    let Some(prev) = prev else {
        return true;
    };
    let d = |a: Pixels, b: Pixels| (f32::from(a) - f32::from(b)).abs() > 0.5;
    d(prev.origin.x, next.origin.x)
        || d(prev.origin.y, next.origin.y)
        || d(prev.size.width, next.size.width)
        || d(prev.size.height, next.size.height)
}

/// Reads the frames recorded so far.
pub fn frames(cx: &App) -> Frames {
    cx.try_global::<Frames>().cloned().unwrap_or_default()
}
