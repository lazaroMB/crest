//! Renderer of the `trace` block kind (`show_trace`): a distributed
//! trace as a waterfall on the timeline widget.
//!
//! A summary line comes first: the format, the trace, span and error
//! counts, the end-to-end time and one chip per service in the color its
//! spans have. The `critical path` button switches the waterfall to only
//! the spans on the critical path (each mode keeps its own zoom). The
//! waterfall is [`crate::widgets::timeline`]: rows indented by depth,
//! failed spans in the danger color, events as ticks, attributes in the
//! tooltip and in the detail line of the selected span.

use std::rc::Rc;

use domain::block::{BlockId, BlockKind};
use domain::devops::trace::TraceSpec;
use domain::widget::timeline::{TimeUnit, TimelineSpec, format_date_time, format_duration};
use gpui::{
    AnyElement, App, Context, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    prelude::FluentBuilder as _,
};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};

use super::custom::node_id;
use crate::timeline::TimelineEntry;
use crate::widgets::timeline::axis::color_slot;
use crate::widgets::{self, palette, readout, tool_button};

/// The file name of `path`.
pub(crate) fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// The caption of a devops block: its title, else the file name, else
/// `fallback`.
pub(crate) fn caption(title: Option<&str>, path: Option<&str>, fallback: &str) -> String {
    title
        .filter(|t| !t.trim().is_empty())
        .map(str::to_owned)
        .or_else(|| path.map(|p| file_name(p).to_owned()))
        .unwrap_or_else(|| fallback.to_owned())
}

/// Per-block state: the spec, the two timelines (all spans, critical
/// path) and which one shows.
pub struct TraceView {
    id: BlockId,
    spec: Rc<TraceSpec>,
    full: Rc<TimelineSpec>,
    critical: Rc<TimelineSpec>,
    critical_only: bool,
}

impl TraceView {
    fn new(id: BlockId, spec: &TraceSpec) -> Self {
        let title = caption(spec.title.as_deref(), spec.path.as_deref(), "Trace");
        let mut full = spec.timeline.clone();
        full.title = Some(title.clone());
        let mut critical = spec.critical_timeline();
        critical.title = Some(format!("{title} · critical path"));
        Self {
            id,
            spec: Rc::new(spec.clone()),
            full: Rc::new(full),
            critical: Rc::new(critical),
            critical_only: false,
        }
    }

    fn sync(&mut self, spec: &TraceSpec) {
        if *self.spec != *spec {
            let critical_only = self.critical_only;
            *self = Self::new(self.id, spec);
            self.critical_only = critical_only;
        }
    }

    /// Whether only the critical path shows.
    pub fn critical_only(&self) -> bool {
        self.critical_only
    }

    /// Shows only the critical path, or every span.
    pub fn set_critical_only(&mut self, on: bool, cx: &mut Context<Self>) {
        self.critical_only = on;
        cx.notify();
    }

    /// The timeline shown now.
    pub fn shown(&self) -> &TimelineSpec {
        if self.critical_only {
            &self.critical
        } else {
            &self.full
        }
    }

    fn summary(&self, cx: &mut Context<Self>) -> AnyElement {
        let spec = &self.spec;
        let theme = cx.theme();
        let errors = spec.errors().len();
        let mut text = format!(
            "{} · {} trace{} · {} span{}",
            spec.format.name(),
            spec.traces,
            if spec.traces == 1 { "" } else { "s" },
            spec.span_count(),
            if spec.span_count() == 1 { "" } else { "s" },
        );
        if spec.total_spans > spec.span_count() {
            text.push_str(&format!(" of {}", spec.total_spans));
        }
        text.push_str(&format!(
            " · {}",
            format_duration(spec.duration(), TimeUnit::Ms)
        ));
        if let Some(at) = spec.started_at {
            text.push_str(&format!(" · {} UTC", format_date_time(at)));
        }
        let services = spec.services.iter().map(|service| {
            let color = palette(color_slot(service), cx);
            h_flex()
                .gap_1()
                .child(div().size_2().rounded_sm().bg(color))
                .child(SharedString::from(service.clone()))
                .into_any_element()
        });
        let label = if self.critical_only {
            "all spans"
        } else {
            "critical path"
        };
        h_flex()
            .flex_wrap()
            .gap_2()
            .text_xs()
            .text_color(theme.muted_foreground)
            .debug_selector(|| "trace-summary".to_owned())
            .child(text)
            .when(errors > 0, |d| {
                d.child(div().text_color(cx.theme().danger).child(format!(
                    "{errors} error{}",
                    if errors == 1 { "" } else { "s" }
                )))
            })
            .children(services)
            .child(div().flex_1())
            .when(!spec.critical_path.is_empty(), |d| {
                d.child(readout(format!("{} on path", spec.critical_path.len()), cx))
                    .child(
                        tool_button("trace-critical", label, cx).on_click(cx.listener(
                            |this, _, _, cx| {
                                cx.stop_propagation();
                                let on = !this.critical_only;
                                this.set_critical_only(on, cx);
                            },
                        )),
                    )
            })
            .into_any_element()
    }
}

impl Render for TraceView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (ordinal, shown) = if self.critical_only {
            (1, self.critical.clone())
        } else {
            (0, self.full.clone())
        };
        let timeline = widgets::timeline::render(node_id(self.id, ordinal), &shown, window, cx);
        v_flex()
            .w_full()
            .gap_1()
            .debug_selector(|| "trace".to_owned())
            .child(self.summary(cx))
            .child(timeline)
    }
}

/// The trace entity of block `id`, in sync with `spec`.
pub fn view(
    id: BlockId,
    spec: &TraceSpec,
    window: &mut Window,
    cx: &mut App,
) -> gpui::Entity<TraceView> {
    let view = super::block_state(id, window, cx, |_, _| TraceView::new(id, spec));
    view.update(cx, |view, _| view.sync(spec));
    view
}

/// Draws the block held by `entry`.
pub fn render(entry: &TimelineEntry, window: &mut Window, cx: &mut App) -> AnyElement {
    let BlockKind::Trace(spec) = &entry.block.kind else {
        return super::placeholder(entry, cx);
    };
    view(entry.block.id, spec, window, cx).into_any_element()
}

#[cfg(test)]
mod tests {
    use application::ports::events::DomainEvent;
    use domain::block::Block;
    use domain::devops::trace::{SERVICE, TraceFormat, critical_path};
    use domain::widget::timeline::{Span, SpanStatus, TimelineRow};
    use gpui::{Modifiers, TestAppContext, point, px, size};
    use serde_json::Map;

    use super::*;
    use crate::timeline::TimelineView as Timeline;

    fn spec() -> TraceSpec {
        let row = |id: &str, depth, start, end, service: &str, failed: bool| {
            let mut attrs = Map::new();
            attrs.insert(SERVICE.into(), service.into());
            TimelineRow {
                id: id.into(),
                label: id.into(),
                depth,
                group: None,
                spans: vec![Span {
                    start,
                    end,
                    label: None,
                    color_key: Some(service.into()),
                    status: failed.then_some(SpanStatus::Error),
                    attrs,
                }],
                markers: Vec::new(),
            }
        };
        let timeline = TimelineSpec {
            title: None,
            unit: TimeUnit::Ms,
            rows: vec![
                row("GET /checkout", 0, 0.0, 100.0, "api", false),
                row("cache", 1, 12.0, 20.0, "api", false),
                row("charge", 1, 10.0, 90.0, "payments", true),
                row("SELECT", 2, 20.0, 40.0, "db", false),
            ],
            range: None,
        };
        TraceSpec {
            title: None,
            path: Some("traces/checkout.json".into()),
            format: TraceFormat::Otlp,
            traces: 1,
            services: vec!["api".into(), "payments".into(), "db".into()],
            started_at: Some(1_792_847_107.0),
            total_spans: 4,
            critical_path: critical_path(&timeline),
            timeline,
        }
    }

    #[test]
    fn captions_fall_back_to_the_file_name() {
        assert_eq!(caption(None, Some("a/b/t.json"), "Trace"), "t.json");
        assert_eq!(caption(Some(" "), None, "Trace"), "Trace");
        assert_eq!(caption(Some("x"), Some("a"), "Trace"), "x");
    }

    #[gpui::test]
    fn draws_the_waterfall_and_toggles_the_critical_path(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, cx| Timeline::new(cx));
        let block = Block::new(BlockKind::Trace(Box::new(spec())));
        let id = block.id;
        view.update(cx, |view, cx| {
            view.apply_all([DomainEvent::BlockAdded { block }], cx);
        });
        cx.draw(point(px(0.), px(0.)), size(px(900.), px(700.)), |_, _| {
            div().size_full().child(view.clone())
        });
        cx.run_until_parked();
        for selector in [
            "trace",
            "trace-summary",
            "timeline-canvas",
            "trace-critical",
        ] {
            assert!(cx.debug_bounds(selector).is_some(), "{selector}");
        }
        let trace = cx.update(|window, cx| super::view(id, &spec(), window, cx));
        assert_eq!(trace.read_with(cx, |t, _| t.shown().rows.len()), 4);
        let at = cx
            .debug_bounds("trace-critical")
            .map(|b| b.center())
            .unwrap_or_default();
        cx.simulate_click(at, Modifiers::none());
        cx.run_until_parked();
        assert!(trace.read_with(cx, |t, _| t.critical_only()));
        let rows: Vec<String> = trace.read_with(cx, |t, _| {
            t.shown().rows.iter().map(|r| r.id.clone()).collect()
        });
        assert_eq!(rows, ["GET /checkout", "charge", "SELECT"]);
    }
}
