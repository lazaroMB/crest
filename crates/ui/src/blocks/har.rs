//! Renderer of the `har` block kind (`show_har`): an HTTP archive as a
//! waterfall, a request list and an inspector.
//!
//! A summary line comes first: what wrote the file, the request and
//! failure counts, the bytes transferred, the page timings and one chip
//! per timing phase in the color its bars have. The waterfall is
//! [`crate::widgets::timeline`] (a row per request, a bar per phase,
//! failed requests in the danger color, `DOMContentLoaded` and `load` as
//! ticks); the list is [`crate::widgets::list`] (status, type, size and
//! time columns, chips by status class, type, method and host). Selecting
//! a request in either opens the inspector: headers and query, request
//! and response bodies (JSON pretty-printed), and the phases as bars.
//! Cookies, authorization and token values stay masked until the user
//! reveals them; "copy as curl" copies the request.

use std::rc::Rc;

use domain::block::{BlockId, BlockKind};
use domain::devops::har::{
    HarBody, HarEntry, HarHeader, HarSpec, PHASES, format_bytes, is_sensitive_header,
    is_sensitive_param, redact_url,
};
use domain::widget::list::ListSpec;
use domain::widget::timeline::{TimeUnit, TimelineSpec, format_date_time, format_duration};
use gpui::{
    AnyElement, App, ClipboardItem, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, Window, div, prelude::FluentBuilder as _, px,
};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};

use super::custom::node_id;
use super::trace::caption;
use crate::timeline::TimelineEntry;
use crate::widgets::list::{ListEvent, ListView};
use crate::widgets::timeline::axis::color_slot;
use crate::widgets::timeline::{TimelineEvent, TimelineView};
use crate::widgets::{self, palette, readout, tool_button};

/// What a masked value shows.
pub const MASK: &str = "••••••";
/// Lines of a body shown in the inspector.
pub const BODY_LINES: usize = 200;
/// Width of a header name in the inspector.
const KEY_WIDTH: f32 = 180.0;

/// The inspector's tab.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Tab {
    /// Request and response headers, and the query.
    #[default]
    Headers,
    /// The posted body.
    Request,
    /// The response body.
    Response,
    /// The timing phases.
    Timing,
}

/// A header value as the inspector shows it: masked when sensitive and
/// not revealed.
#[must_use]
pub fn shown_value(header: &HarHeader, sensitive: bool, reveal: bool) -> String {
    if sensitive && !reveal {
        MASK.to_owned()
    } else {
        header.value.clone()
    }
}

/// A body's preview for the inspector: JSON pretty-printed, at most
/// [`BODY_LINES`] lines, with a note of what was left out.
#[must_use]
pub fn body_text(body: &HarBody) -> String {
    let Some(text) = &body.text else {
        return match (body.base64, body.size) {
            (true, Some(size)) => format!("(binary, {})", format_bytes(size)),
            (true, None) => "(binary)".to_owned(),
            (false, _) => "(not kept in the file)".to_owned(),
        };
    };
    let json = body
        .mime_type
        .as_deref()
        .is_some_and(|m| m.contains("json"))
        || text.trim_start().starts_with(['{', '[']);
    let pretty = json
        .then(|| serde_json::from_str::<serde_json::Value>(text).ok())
        .flatten()
        .and_then(|v| serde_json::to_string_pretty(&v).ok())
        .unwrap_or_else(|| text.clone());
    let total = pretty.lines().count();
    let mut out: String = pretty
        .lines()
        .take(BODY_LINES)
        .collect::<Vec<_>>()
        .join("\n");
    if total > BODY_LINES {
        out.push_str(&format!("\n… {} more lines", total - BODY_LINES));
    }
    if body.truncated {
        out.push_str("\n… cut to the preview size");
    }
    out
}

/// Per-block state: the spec, its waterfall and list, the selected
/// request and the inspector's tab and reveal switch.
pub struct HarView {
    id: BlockId,
    spec: Rc<HarSpec>,
    timeline: Entity<TimelineView>,
    list: Entity<ListView>,
    selected: Option<usize>,
    tab: Tab,
    reveal: bool,
    _subscriptions: [Subscription; 2],
}

impl HarView {
    fn new(id: BlockId, spec: &HarSpec, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let title = caption(spec.title.as_deref(), spec.path.as_deref(), "HAR");
        let mut timeline_spec: TimelineSpec = spec.timeline();
        timeline_spec.title = Some(title);
        let mut list_spec: ListSpec = spec.list();
        list_spec.title = Some("Requests".to_owned());
        let timeline = widgets::timeline::view(node_id(id, 0), &timeline_spec, window, cx);
        let list = widgets::list::view(node_id(id, 1), &list_spec, window, cx);
        let on_timeline = cx.subscribe(&timeline, |this: &mut Self, _, event, cx| {
            let TimelineEvent::Selected(span) = event;
            if let Some(span) = span {
                this.select(Some(span.row), cx);
            }
        });
        let on_list = cx.subscribe(&list, |this: &mut Self, _, event, cx| {
            let ListEvent::Selected(item) = event;
            if let Some(item) = item {
                this.select(Some(*item), cx);
            }
        });
        Self {
            id,
            spec: Rc::new(spec.clone()),
            timeline,
            list,
            selected: None,
            tab: Tab::default(),
            reveal: false,
            _subscriptions: [on_timeline, on_list],
        }
    }

    fn sync(&mut self, spec: &HarSpec, window: &mut Window, cx: &mut Context<Self>) {
        if *self.spec != *spec {
            let (tab, reveal) = (self.tab, self.reveal);
            *self = Self::new(self.id, spec, window, cx);
            self.tab = tab;
            self.reveal = reveal;
        }
    }

    /// The request open in the inspector.
    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    /// Opens request `ix` in the inspector (or closes it).
    pub fn select(&mut self, ix: Option<usize>, cx: &mut Context<Self>) {
        self.selected = ix.filter(|ix| *ix < self.spec.entries.len());
        cx.notify();
    }

    /// The inspector's tab.
    pub fn tab(&self) -> Tab {
        self.tab
    }

    /// Switches the inspector's tab.
    pub fn set_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        self.tab = tab;
        cx.notify();
    }

    /// Whether secrets show in clear.
    pub fn reveal(&self) -> bool {
        self.reveal
    }

    /// Shows or masks secrets.
    pub fn set_reveal(&mut self, reveal: bool, cx: &mut Context<Self>) {
        self.reveal = reveal;
        cx.notify();
    }

    fn summary(&self, cx: &App) -> AnyElement {
        let spec = &self.spec;
        let theme = cx.theme();
        let failed = spec.failed().len();
        let mut text = String::from("HAR");
        if let Some(creator) = &spec.creator {
            text.push_str(&format!(" · {creator}"));
        }
        text.push_str(&format!(" · {} requests", spec.entries.len()));
        if spec.total_entries > spec.entries.len() {
            text.push_str(&format!(" of {}", spec.total_entries));
        }
        text.push_str(&format!(
            " · {} · {}",
            format_bytes(spec.transferred()),
            format_duration(spec.duration(), TimeUnit::Ms)
        ));
        if let Some(at) = spec.started_at {
            text.push_str(&format!(" · {} UTC", format_date_time(at)));
        }
        for page in &spec.pages {
            if let Some(load) = page.on_load {
                text.push_str(&format!(" · load {}", format_duration(load, TimeUnit::Ms)));
            }
        }
        let phases = PHASES.iter().map(|phase| {
            h_flex()
                .gap_1()
                .child(
                    div()
                        .size_2()
                        .rounded_sm()
                        .bg(palette(color_slot(phase), cx)),
                )
                .child(*phase)
                .into_any_element()
        });
        h_flex()
            .flex_wrap()
            .gap_2()
            .text_xs()
            .text_color(theme.muted_foreground)
            .debug_selector(|| "har-summary".to_owned())
            .child(text)
            .when(failed > 0, |d| {
                d.child(
                    div()
                        .text_color(theme.danger)
                        .child(format!("{failed} failed")),
                )
            })
            .children(phases)
            .into_any_element()
    }

    fn header_rows(
        &self,
        title: &str,
        headers: &[HarHeader],
        sensitive: fn(&str) -> bool,
        cx: &App,
    ) -> AnyElement {
        let theme = cx.theme();
        let rows = headers.iter().map(|h| {
            let secret = sensitive(&h.name);
            h_flex()
                .gap_2()
                .items_start()
                .child(
                    div()
                        .w(px(KEY_WIDTH))
                        .flex_none()
                        .text_color(theme.muted_foreground)
                        .child(SharedString::from(h.name.clone())),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .when(secret, |d| d.text_color(theme.warning))
                        .child(shown_value(h, secret, self.reveal)),
                )
        });
        v_flex()
            .gap_0p5()
            .child(
                div()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(title.to_owned()),
            )
            .when(headers.is_empty(), |d| {
                d.child(div().text_color(theme.muted_foreground).child("none"))
            })
            .children(rows)
            .into_any_element()
    }

    fn body(&self, body: Option<&HarBody>, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let Some(body) = body else {
            return div()
                .text_color(theme.muted_foreground)
                .child("no body")
                .into_any_element();
        };
        let mut head = body.mime_type.clone().unwrap_or_else(|| "body".to_owned());
        if let Some(size) = body.size {
            head.push_str(&format!(" · {}", format_bytes(size)));
        }
        v_flex()
            .gap_1()
            .child(div().text_color(theme.muted_foreground).child(head))
            .child(
                div()
                    .font_family(theme.mono_font_family.clone())
                    .whitespace_normal()
                    .debug_selector(|| "har-body".to_owned())
                    .child(body_text(body)),
            )
            .into_any_element()
    }

    fn timing(&self, entry: &HarEntry, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let total = entry.time.max(f64::EPSILON);
        let rows = entry.timings.phases().into_iter().map(|(name, t)| {
            let share = t.map_or(0.0, |t| (t / total).clamp(0.0, 1.0));
            h_flex()
                .gap_2()
                .child(div().w(px(70.)).child(name))
                .child(
                    div()
                        .w(px(90.))
                        .text_color(theme.muted_foreground)
                        .child(t.map_or("—".to_owned(), |t| format_duration(t, TimeUnit::Ms))),
                )
                .child(
                    div().flex_1().h(px(8.)).child(
                        div()
                            .h_full()
                            .w(gpui::relative(share as f32))
                            .rounded_sm()
                            .bg(palette(color_slot(name), cx)),
                    ),
                )
        });
        v_flex()
            .gap_0p5()
            .debug_selector(|| "har-timing".to_owned())
            .children(rows)
            .child(div().text_color(theme.muted_foreground).child(format!(
                "total {}",
                format_duration(entry.time, TimeUnit::Ms)
            )))
            .into_any_element()
    }

    fn inspector(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let ix = self.selected?;
        let entry = self.spec.entries.get(ix)?.clone();
        let theme = cx.theme();
        let status_color = if entry.failed() {
            theme.danger
        } else {
            theme.muted_foreground
        };
        let url = if self.reveal {
            entry.url.clone()
        } else {
            redact_url(&entry.url)
        };
        let mut facts = Vec::new();
        if let Some(v) = &entry.http_version {
            facts.push(v.clone());
        }
        if let Some(size) = entry.size() {
            facts.push(format_bytes(size));
        }
        facts.push(format_duration(entry.time, TimeUnit::Ms));
        if let Some(ip) = &entry.server_ip {
            facts.push(ip.clone());
        }
        if let Some(error) = &entry.error {
            facts.push(error.clone());
        }
        let status = if entry.status == 0 {
            "no response".to_owned()
        } else {
            format!(
                "{} {}",
                entry.status,
                entry.status_text.clone().unwrap_or_default()
            )
        };
        let tab = |id: &'static str, label: &'static str, which: Tab, cx: &mut Context<Self>| {
            tool_button(id, label, cx)
                .when(self.tab != which, |b| b.opacity(0.6))
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.set_tab(which, cx);
                }))
        };
        let curl = entry.to_curl(self.reveal);
        let toolbar = h_flex()
            .flex_wrap()
            .gap_1()
            .child(tab("har-tab-headers", "headers", Tab::Headers, cx))
            .child(tab("har-tab-request", "request", Tab::Request, cx))
            .child(tab("har-tab-response", "response", Tab::Response, cx))
            .child(tab("har-tab-timing", "timing", Tab::Timing, cx))
            .child(div().flex_1())
            .child(
                tool_button(
                    "har-reveal",
                    if self.reveal {
                        "mask secrets"
                    } else {
                        "reveal secrets"
                    },
                    cx,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    let reveal = !this.reveal;
                    this.set_reveal(reveal, cx);
                })),
            )
            .child(
                tool_button("har-curl", "copy as curl", cx).on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    cx.write_to_clipboard(ClipboardItem::new_string(curl.clone()));
                }),
            )
            .child(tool_button("har-close", "close", cx).on_click(cx.listener(
                |this, _, _, cx| {
                    cx.stop_propagation();
                    this.select(None, cx);
                },
            )));
        let content = match self.tab {
            Tab::Headers => v_flex()
                .gap_2()
                .child(self.header_rows(
                    "Request headers",
                    &entry.request_headers,
                    is_sensitive_header,
                    cx,
                ))
                .child(self.header_rows(
                    "Response headers",
                    &entry.response_headers,
                    is_sensitive_header,
                    cx,
                ))
                .when(!entry.query.is_empty(), |d| {
                    d.child(self.header_rows("Query", &entry.query, is_sensitive_param, cx))
                })
                .into_any_element(),
            Tab::Request => self.body(entry.request_body.as_ref(), cx),
            Tab::Response => self.body(entry.response_body.as_ref(), cx),
            Tab::Timing => self.timing(&entry, cx),
        };
        let theme = cx.theme();
        Some(
            v_flex()
                .gap_1()
                .p_2()
                .rounded_md()
                .border_1()
                .border_color(theme.border)
                .text_xs()
                .debug_selector(|| "har-inspector".to_owned())
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            div()
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .child(entry.method.clone()),
                        )
                        .child(div().text_color(status_color).child(status))
                        .child(readout(entry.resource_type.clone(), cx))
                        .child(
                            div()
                                .text_color(theme.muted_foreground)
                                .child(facts.join(" · ")),
                        ),
                )
                .child(div().min_w_0().child(url))
                .child(toolbar)
                .child(content)
                .into_any_element(),
        )
    }
}

impl Render for HarView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .w_full()
            .gap_1()
            .debug_selector(|| "har".to_owned())
            .child(self.summary(cx))
            .child(self.timeline.clone())
            .child(self.list.clone())
            .children(self.inspector(cx))
    }
}

/// The HAR entity of block `id`, in sync with `spec`.
pub fn view(id: BlockId, spec: &HarSpec, window: &mut Window, cx: &mut App) -> Entity<HarView> {
    let view = super::block_state(id, window, cx, |window, cx| {
        HarView::new(id, spec, window, cx)
    });
    view.update(cx, |view, cx| view.sync(spec, window, cx));
    view
}

/// Draws the block held by `entry`.
pub fn render(entry: &TimelineEntry, window: &mut Window, cx: &mut App) -> AnyElement {
    let BlockKind::Har(spec) = &entry.block.kind else {
        return super::placeholder(entry, cx);
    };
    view(entry.block.id, spec, window, cx).into_any_element()
}

#[cfg(test)]
mod tests {
    use application::ports::events::DomainEvent;
    use domain::block::Block;
    use domain::devops::har::HarTimings;
    use gpui::{Modifiers, TestAppContext, point, size};

    use super::*;
    use crate::timeline::TimelineView as Timeline;

    fn spec() -> HarSpec {
        let entry = |method: &str, url: &str, status, start| HarEntry {
            method: method.into(),
            url: url.into(),
            status,
            resource_type: "fetch".into(),
            start,
            time: 50.0,
            timings: HarTimings {
                send: Some(10.0),
                wait: Some(40.0),
                ..HarTimings::default()
            },
            request_headers: vec![HarHeader {
                name: "Cookie".into(),
                value: "sid=abc".into(),
            }],
            response_body: Some(HarBody {
                mime_type: Some("application/json".into()),
                size: Some(9),
                text: Some("{\"ok\":1}".into()),
                truncated: false,
                base64: false,
            }),
            ..HarEntry::default()
        };
        HarSpec {
            title: None,
            path: Some("net/app.har".into()),
            creator: None,
            started_at: None,
            pages: Vec::new(),
            entries: vec![
                entry("GET", "https://app.test/", 200, 0.0),
                entry("POST", "https://app.test/pay", 503, 60.0),
            ],
            total_entries: 2,
        }
    }

    #[test]
    fn bodies_and_secrets_for_the_inspector() {
        let spec = spec();
        let body = spec.entries[0].response_body.clone().unwrap_or_default();
        assert_eq!(body_text(&body), "{\n  \"ok\": 1\n}");
        let binary = HarBody {
            base64: true,
            size: Some(2048),
            ..HarBody::default()
        };
        assert_eq!(body_text(&binary), "(binary, 2.0 kB)");
        let cookie = &spec.entries[0].request_headers[0];
        assert_eq!(shown_value(cookie, true, false), MASK);
        assert_eq!(shown_value(cookie, true, true), "sid=abc");
    }

    #[gpui::test]
    fn a_click_in_the_list_opens_the_inspector(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, cx| Timeline::new(cx));
        let block = Block::new(BlockKind::Har(Box::new(spec())));
        let id = block.id;
        view.update(cx, |view, cx| {
            view.apply_all([DomainEvent::BlockAdded { block }], cx);
        });
        cx.draw(point(px(0.), px(0.)), size(px(900.), px(1200.)), |_, _| {
            div().size_full().child(view.clone())
        });
        cx.run_until_parked();
        for selector in ["har", "har-summary", "timeline-canvas", "filter-list-input"] {
            assert!(cx.debug_bounds(selector).is_some(), "{selector}");
        }
        let har = cx.update(|window, cx| super::view(id, &spec(), window, cx));
        har.update(cx, |har, cx| har.select(Some(1), cx));
        cx.run_until_parked();
        cx.draw(point(px(0.), px(0.)), size(px(900.), px(1200.)), |_, _| {
            div().size_full().child(view.clone())
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("har-inspector").is_some());
        let at = cx
            .debug_bounds("har-tab-response")
            .map(|b| b.center())
            .unwrap_or_default();
        cx.simulate_click(at, Modifiers::none());
        cx.run_until_parked();
        assert_eq!(har.read_with(cx, |h, _| h.tab()), Tab::Response);
        let at = cx
            .debug_bounds("har-reveal")
            .map(|b| b.center())
            .unwrap_or_default();
        cx.simulate_click(at, Modifiers::none());
        cx.run_until_parked();
        assert!(har.read_with(cx, |h, _| h.reveal()));
        // A list selection also opens a request.
        let list = har.read_with(cx, |h, _| h.list.clone());
        list.update(cx, |_, cx| cx.emit(ListEvent::Selected(Some(0))));
        cx.run_until_parked();
        assert_eq!(har.read_with(cx, |h, _| h.selected()), Some(0));
    }
}
