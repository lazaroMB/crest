//! An element whose height follows from the width it is given.
//!
//! [`measured`] takes the whole width of its parent and asks a function for
//! the height at that width during layout, so a block can grow or shrink
//! with the window (a chart keeps its aspect ratio, labels that need more
//! room add height). The child is laid out on its own at the resulting size
//! in prepaint, the way GPUI's `container_query` does.
//!
//! The timeline's `list` gives every row a definite width and measures the
//! visible rows again each frame, so a resized window lays the block out
//! anew without extra bookkeeping.

use std::rc::Rc;

use gpui::{
    AnyElement, App, AvailableSpace, Bounds, Element, ElementId, GlobalElementId,
    InspectorElementId, IntoElement, LayoutId, Pixels, Size, Style, Window, px, relative, size,
};

/// Width assumed when layout asks for the natural (max-content) size, where
/// no width is offered yet.
pub const FALLBACK_WIDTH: f32 = 640.0;

/// The height a measured element takes at a given width.
pub type HeightFor = dyn Fn(Pixels, &mut Window, &mut App) -> Pixels;

/// An element as wide as its parent and as tall as `height_for` says for
/// that width, drawing `child` at exactly that size.
pub fn measured(
    height_for: impl Fn(Pixels, &mut Window, &mut App) -> Pixels + 'static,
    child: impl IntoElement,
) -> Measured {
    Measured {
        height_for: Rc::new(height_for),
        child: Some(child.into_any_element()),
        child_for: None,
    }
}

/// Builds the child of [`measured_with`] for the size it was given.
pub type ChildFor = dyn FnOnce(Size<Pixels>, &mut Window, &mut App) -> AnyElement;

/// [`measured`] with a child built once its size is known, for content that
/// takes only part of the row (a picture as wide as its fitted size).
pub fn measured_with(
    height_for: impl Fn(Pixels, &mut Window, &mut App) -> Pixels + 'static,
    child_for: impl FnOnce(Size<Pixels>, &mut Window, &mut App) -> AnyElement + 'static,
) -> Measured {
    Measured {
        height_for: Rc::new(height_for),
        child: None,
        child_for: Some(Box::new(child_for)),
    }
}

/// An element sized by a height-for-width function; see [`measured`].
pub struct Measured {
    height_for: Rc<HeightFor>,
    child: Option<AnyElement>,
    child_for: Option<Box<ChildFor>>,
}

/// The width a measure pass offers, or `None` under a min-content probe.
fn offered_width(known: Option<Pixels>, available: AvailableSpace) -> Option<Pixels> {
    known.or(match available {
        AvailableSpace::Definite(width) => Some(width),
        AvailableSpace::MaxContent => Some(px(FALLBACK_WIDTH)),
        AvailableSpace::MinContent => None,
    })
}

/// The size [`measured`] takes for the width a measure pass offers. A
/// min-content probe gets no width of its own and the fallback's height.
fn measured_size(
    known: Option<Pixels>,
    available: AvailableSpace,
    height_for: impl FnOnce(Pixels) -> Pixels,
) -> Size<Pixels> {
    match offered_width(known, available) {
        Some(width) => size(width, height_for(width)),
        None => size(px(0.), height_for(px(FALLBACK_WIDTH))),
    }
}

impl Element for Measured {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        _cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        let height_for = self.height_for.clone();
        let layout_id =
            window.request_measured_layout(style, move |known, available, window, cx| {
                measured_size(known.width, available.width, |width| {
                    height_for(width, window, cx)
                })
            });
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(child_for) = self.child_for.take() {
            self.child = Some(child_for(bounds.size, window, cx));
        }
        if let Some(child) = self.child.as_mut() {
            child.layout_as_root(bounds.size.map(AvailableSpace::Definite), window, cx);
            child.prepaint_at(bounds.origin, window, cx);
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(child) = self.child.as_mut() {
            child.paint(window, cx);
        }
    }
}

impl IntoElement for Measured {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{
        InteractiveElement as _, ParentElement as _, Styled as _, TestAppContext, div, point,
    };

    #[test]
    fn the_offered_width_decides_the_height() {
        let half = |w: Pixels| w * 0.5;
        assert_eq!(
            measured_size(None, AvailableSpace::Definite(px(800.)), half),
            size(px(800.), px(400.))
        );
        assert_eq!(
            measured_size(Some(px(300.)), AvailableSpace::Definite(px(800.)), half),
            size(px(300.), px(150.))
        );
        assert_eq!(
            measured_size(None, AvailableSpace::MaxContent, half),
            size(px(FALLBACK_WIDTH), px(FALLBACK_WIDTH / 2.))
        );
        assert_eq!(
            measured_size(None, AvailableSpace::MinContent, half).width,
            px(0.)
        );
    }

    #[gpui::test]
    fn takes_the_parent_width_and_its_height(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        for width in [400., 1000.] {
            cx.simulate_resize(size(px(width), px(800.)));
            cx.draw(point(px(0.), px(0.)), size(px(width), px(800.)), |_, _| {
                div().w_full().child(measured(
                    |w, _, _| w * 0.25,
                    div()
                        .size_full()
                        .debug_selector(|| "measured-child".to_owned()),
                ))
            });
            let bounds = cx
                .debug_bounds("measured-child")
                .unwrap_or_else(|| panic!("child drawn"));
            assert_eq!(bounds.size, size(px(width), px(width * 0.25)));
        }
    }
}
