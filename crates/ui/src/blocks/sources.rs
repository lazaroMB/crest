//! Renderer of the `sources` block kind: the numbered list of sources a
//! workflow node cited.
//!
//! Each entry is a link where it can be: a file (`path:12-20`) emits
//! [`BlockEvent::OpenFile`] and a URL opens the browser and emits
//! [`BlockEvent::OpenUrl`], on click or on `Enter` while focused. Commits
//! and commands are shown as text.

pub mod events;

use domain::block::{BlockKind, SourceRef};
use domain::workspace::WorkspacePath;
use gpui::{
    AnyElement, App, ElementId, InteractiveElement as _, IntoElement as _, KeyDownEvent,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};

pub use events::{BlockEvent, BlockEvents, block_events, emit};

use crate::timeline::TimelineEntry;

/// Characters of a commit hash shown in the list.
const SHORT_SHA: usize = 7;

/// Lines of a command's output excerpt shown under it.
const EXCERPT_LINES: usize = 3;

/// One-line label of a source: `path:12-20`, `abc1234 summary`,
/// `title — host` or `$ command`.
pub fn entry_label(source: &SourceRef) -> String {
    match source {
        SourceRef::File {
            path,
            line_start,
            line_end,
        } => match (line_start, line_end) {
            (Some(start), Some(end)) if end != start => format!("{path}:{start}-{end}"),
            (Some(start), _) => format!("{path}:{start}"),
            _ => path.clone(),
        },
        SourceRef::Commit { sha, summary } => {
            let short: String = sha.chars().take(SHORT_SHA).collect();
            format!("{short} {summary}")
        }
        SourceRef::Url { url, title } => match title.as_deref().map(str::trim) {
            Some(title) if !title.is_empty() => format!("{title} — {}", host(url)),
            _ => url.clone(),
        },
        SourceRef::Command { command, .. } => format!("$ {command}"),
    }
}

/// The host of `url`, without scheme, credentials, port or path; the whole
/// string when it has no scheme.
pub fn host(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    host.split(':').next().unwrap_or(host)
}

/// What activating `source` does, or `None` when it is not a link. Files
/// outside the workspace (absolute or escaping paths) are not links.
pub fn action(source: &SourceRef) -> Option<BlockEvent> {
    match source {
        SourceRef::File {
            path, line_start, ..
        } => Some(BlockEvent::OpenFile {
            path: WorkspacePath::parse(path).ok()?,
            line: line_start.map(u64::from),
        }),
        SourceRef::Url { url, .. } => Some(BlockEvent::OpenUrl { url: url.clone() }),
        SourceRef::Commit { .. } | SourceRef::Command { .. } => None,
    }
}

/// Performs `event`: opens a URL in the browser, then tells subscribers.
pub fn activate(event: &BlockEvent, cx: &mut App) {
    if let BlockEvent::OpenUrl { url } = event {
        cx.open_url(url);
    }
    emit(event.clone(), cx);
}

fn render_entry(entry: &TimelineEntry, ix: usize, source: &SourceRef, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let number = div()
        .text_color(theme.muted_foreground)
        .child(SharedString::from(format!("[{}]", ix + 1)));
    let label = div()
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .child(SharedString::from(entry_label(source)));
    let id = ElementId::Name(SharedString::from(format!(
        "source-{}-{ix}",
        entry.block.id
    )));
    let line = h_flex().gap_1().child(number);
    let line = match action(source) {
        Some(event) => {
            let on_key = event.clone();
            line.child(
                label
                    .id(id)
                    .debug_selector(|| format!("source-{ix}"))
                    .tab_index(0)
                    .cursor_pointer()
                    .text_color(theme.link)
                    .hover(|style| style.underline())
                    .focus(|style| style.underline())
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        activate(&event, cx);
                    })
                    .on_key_down(move |key: &KeyDownEvent, _, cx| {
                        if key.keystroke.key == "enter" {
                            cx.stop_propagation();
                            activate(&on_key, cx);
                        }
                    }),
            )
        }
        None => line.child(label.text_color(theme.foreground)),
    };
    let SourceRef::Command { output_excerpt, .. } = source else {
        return line.into_any_element();
    };
    let excerpt: Vec<&str> = output_excerpt.lines().take(EXCERPT_LINES).collect();
    if excerpt.is_empty() {
        return line.into_any_element();
    }
    v_flex()
        .child(line)
        .child(
            div()
                .pl_6()
                .font_family(theme.mono_font_family.clone())
                .text_color(theme.muted_foreground)
                .child(SharedString::from(excerpt.join("\n"))),
        )
        .into_any_element()
}

/// Draws the block held by `entry`.
pub fn render(entry: &TimelineEntry, _window: &mut Window, cx: &mut App) -> AnyElement {
    let BlockKind::Sources { sources } = &entry.block.kind else {
        return super::placeholder(entry, cx);
    };
    let theme = cx.theme();
    let header = div()
        .text_color(theme.muted_foreground)
        .child(SharedString::from(format!("Sources ({})", sources.len())));
    v_flex()
        .gap_0p5()
        .child(header)
        .children(
            sources
                .iter()
                .enumerate()
                .map(|(ix, source)| render_entry(entry, ix, source, cx)),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests;
