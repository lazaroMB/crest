//! Renderer of the `logs` block kind (`show_logs`): a log file on the
//! filterable list widget.
//!
//! A summary line gives the format, the line count (and whether the file
//! was cut) and the count of each level in its color. When the lines have
//! times, a histogram follows: one bar per time bucket, stacked errors
//! (danger), warnings (warning) and the rest (muted), with the bucket's
//! time and counts in a tooltip. Then the list itself
//! ([`crate::widgets::list`]): text or `/regex/` filter, chips by level
//! and by the facet fields, a row opens to its fields (and folded stack
//! trace), follow mode and copy.

use std::rc::Rc;

use domain::block::{BlockId, BlockKind};
use domain::devops::logs::{LogHistogram, LogSpec};
use domain::widget::list::ListSpec;
use domain::widget::timeline::{TimeUnit, format_date_time, format_duration};
use gpui::{
    AnyElement, App, Context, ElementId, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    prelude::FluentBuilder as _, px,
};
use gpui_component::tooltip::Tooltip;
use gpui_component::{ActiveTheme as _, h_flex, v_flex};

use super::custom::node_id;
use super::trace::caption;
use crate::timeline::TimelineEntry;
use crate::widgets::{self, list::level_color};

/// Height of the histogram's tallest bar.
pub const HISTOGRAM_HEIGHT: f32 = 36.0;

/// The tooltip of bucket `ix`: its start time and counts.
#[must_use]
pub fn bucket_text(histogram: &LogHistogram, ix: usize) -> String {
    let Some(bucket) = histogram.buckets.get(ix) else {
        return String::new();
    };
    let start = histogram.start + histogram.width * ix as f64;
    format!(
        "{} UTC + {} · {} lines ({} errors, {} warnings)",
        format_date_time(start),
        format_duration(histogram.width, TimeUnit::S),
        bucket.total(),
        bucket.errors,
        bucket.warnings
    )
}

/// Per-block state: the spec and the list it shows (titled).
pub struct LogsView {
    id: BlockId,
    spec: Rc<LogSpec>,
    list: Rc<ListSpec>,
}

impl LogsView {
    fn new(id: BlockId, spec: &LogSpec) -> Self {
        let mut list = spec.list.clone();
        list.title = Some(caption(list.title.as_deref(), spec.path.as_deref(), "Log"));
        Self {
            id,
            spec: Rc::new(spec.clone()),
            list: Rc::new(list),
        }
    }

    fn sync(&mut self, spec: &LogSpec) {
        if *self.spec != *spec {
            *self = Self::new(self.id, spec);
        }
    }

    /// The list drawn (the spec's, with the block's title).
    pub fn list(&self) -> &ListSpec {
        &self.list
    }

    fn summary(&self, cx: &App) -> AnyElement {
        let spec = &self.spec;
        let theme = cx.theme();
        let mut text = format!("{} · {} lines", spec.format.name(), spec.total_lines);
        if spec.total_lines > spec.list.items.len() {
            text.push_str(&format!(" ({} shown)", spec.list.items.len()));
        }
        if spec.truncated {
            text.push_str(" · head of a larger file");
        }
        let levels = spec.level_counts().into_iter().map(|(level, n)| {
            let color = if level == "-" {
                theme.muted_foreground
            } else {
                level_color(&level, cx)
            };
            div()
                .text_color(color)
                .child(format!("{level} {n}"))
                .into_any_element()
        });
        h_flex()
            .flex_wrap()
            .gap_2()
            .text_xs()
            .text_color(theme.muted_foreground)
            .debug_selector(|| "logs-summary".to_owned())
            .child(text)
            .children(levels)
            .into_any_element()
    }

    fn histogram(&self, cx: &App) -> Option<AnyElement> {
        let histogram = self.spec.histogram.clone()?;
        let theme = cx.theme();
        let (danger, warning, other) = (theme.danger, theme.warning, theme.muted_foreground);
        let peak = histogram
            .buckets
            .iter()
            .map(|b| b.total())
            .max()
            .unwrap_or(0)
            .max(1);
        let block = self.id;
        let histogram = Rc::new(histogram);
        let bars = histogram.buckets.iter().enumerate().map(|(ix, bucket)| {
            let scale = HISTOGRAM_HEIGHT / peak as f32;
            let segment = |count: u32, color| div().w_full().h(px(count as f32 * scale)).bg(color);
            let tip = bucket_text(&histogram, ix);
            div()
                .id(ElementId::Name(SharedString::from(format!(
                    "logs-bucket-{block}-{ix}"
                ))))
                .debug_selector(move || format!("logs-bucket-{ix}"))
                .flex_1()
                .min_w(px(1.))
                .h_full()
                .flex()
                .flex_col()
                .justify_end()
                .child(segment(bucket.errors, danger))
                .child(segment(bucket.warnings, warning))
                .child(segment(bucket.other, other.opacity(0.5)))
                .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
        });
        let start = format_date_time(histogram.start);
        let end =
            format_date_time(histogram.start + histogram.width * histogram.buckets.len() as f64);
        Some(
            v_flex()
                .gap_0p5()
                .debug_selector(|| "logs-histogram".to_owned())
                .child(
                    h_flex()
                        .h(px(HISTOGRAM_HEIGHT))
                        .gap(px(1.))
                        .items_end()
                        .children(bars),
                )
                .child(
                    h_flex()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(start)
                        .child(div().flex_1())
                        .child(end),
                )
                .into_any_element(),
        )
    }
}

impl Render for LogsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let list = self.list.clone();
        let widget = widgets::list::render(node_id(self.id, 0), &list, window, cx);
        v_flex()
            .w_full()
            .gap_1()
            .debug_selector(|| "logs".to_owned())
            .child(self.summary(cx))
            .when_some(self.histogram(cx), |d, h| d.child(h))
            .child(widget)
    }
}

/// The logs entity of block `id`, in sync with `spec`.
pub fn view(
    id: BlockId,
    spec: &LogSpec,
    window: &mut Window,
    cx: &mut App,
) -> gpui::Entity<LogsView> {
    let view = super::block_state(id, window, cx, |_, _| LogsView::new(id, spec));
    view.update(cx, |view, _| view.sync(spec));
    view
}

/// Draws the block held by `entry`.
pub fn render(entry: &TimelineEntry, window: &mut Window, cx: &mut App) -> AnyElement {
    let BlockKind::Logs(spec) = &entry.block.kind else {
        return super::placeholder(entry, cx);
    };
    view(entry.block.id, spec, window, cx).into_any_element()
}

#[cfg(test)]
mod tests {
    use application::ports::events::DomainEvent;
    use domain::block::Block;
    use domain::devops::logs::{BucketCounts, LogFormat};
    use gpui::{Modifiers, TestAppContext, point, size};
    use serde_json::json;

    use super::*;
    use crate::timeline::TimelineView as Timeline;

    fn spec() -> LogSpec {
        let list: ListSpec = serde_json::from_value(json!({
            "facets": ["service"],
            "columns": ["service"],
            "items": [
                { "level": "info", "ts": "13:05:00", "text": "started", "fields": { "service": "api" } },
                { "level": "error", "ts": "13:05:01", "text": "db down", "fields": { "service": "db" } },
                { "level": "warn", "ts": "13:05:02", "text": "slow", "fields": { "service": "db" } }
            ]
        }))
        .unwrap_or_else(|e| panic!("{e}"));
        LogSpec {
            path: Some("logs/app.log".into()),
            format: LogFormat::Json,
            total_lines: 3,
            truncated: false,
            list,
            histogram: Some(LogHistogram {
                start: 1_792_847_100.0,
                width: 1.0,
                buckets: vec![
                    BucketCounts {
                        errors: 0,
                        warnings: 0,
                        other: 1,
                    },
                    BucketCounts {
                        errors: 1,
                        warnings: 0,
                        other: 0,
                    },
                    BucketCounts {
                        errors: 0,
                        warnings: 1,
                        other: 0,
                    },
                ],
            }),
        }
    }

    #[test]
    fn bucket_tooltips_name_time_and_counts() {
        let spec = spec();
        let histogram = spec.histogram.as_ref().unwrap();
        assert_eq!(
            bucket_text(histogram, 1),
            "2026-10-24 13:05:01 UTC + 1 s · 1 lines (1 errors, 0 warnings)"
        );
        assert_eq!(bucket_text(histogram, 9), "");
    }

    #[gpui::test]
    fn draws_summary_histogram_and_a_filterable_list(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, cx| Timeline::new(cx));
        let block = Block::new(BlockKind::Logs(Box::new(spec())));
        let id = block.id;
        view.update(cx, |view, cx| {
            view.apply_all([DomainEvent::BlockAdded { block }], cx);
        });
        cx.draw(point(px(0.), px(0.)), size(px(900.), px(800.)), |_, _| {
            div().size_full().child(view.clone())
        });
        cx.run_until_parked();
        for selector in [
            "logs",
            "logs-summary",
            "logs-histogram",
            "logs-bucket-2",
            "filter-list-input",
            "filter-list-chip-level-error",
        ] {
            assert!(cx.debug_bounds(selector).is_some(), "{selector}");
        }
        let logs = cx.update(|window, cx| super::view(id, &spec(), window, cx));
        assert_eq!(
            logs.read_with(cx, |l, _| l.list().title.clone()).as_deref(),
            Some("app.log")
        );
        let at = cx
            .debug_bounds("filter-list-chip-service-db")
            .map(|b| b.center())
            .unwrap_or_default();
        cx.simulate_click(at, Modifiers::none());
        cx.run_until_parked();
        let list = cx.update(|window, cx| {
            let list = logs.read(cx).list().clone();
            widgets::list::view(node_id(id, 0), &list, window, cx)
        });
        assert_eq!(list.read_with(cx, |l, _| l.visible().to_vec()), [1, 2]);
    }
}
