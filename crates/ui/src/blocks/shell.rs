//! Renderer of the `shell` block kind: a `/!` command and what it printed.
//!
//! `$ command` with its exit status, then the output in the monospace
//! font, stderr in the error color. Long output shows its first
//! [`COLLAPSED_LINES`] lines until the block is expanded (`Enter` on the
//! selected block, or a click).

use domain::block::BlockKind;
use gpui::{
    AnyElement, App, FontWeight, IntoElement as _, ParentElement as _, SharedString, Styled as _,
    Window, div,
};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};

use crate::timeline::TimelineEntry;

/// Lines of output shown while the block is collapsed.
pub const COLLAPSED_LINES: usize = 20;

/// The status after the command: `exit 0`, `exit 2`, `timed out`...
pub fn status_label(exit_code: Option<i32>, timed_out: bool) -> String {
    match (exit_code, timed_out) {
        (_, true) => "timed out".to_owned(),
        (Some(code), _) => format!("exit {code}"),
        (None, _) => "killed".to_owned(),
    }
}

/// The first `limit` lines of `text` and how many were left out.
pub fn head(text: &str, limit: usize) -> (String, usize) {
    let lines: Vec<&str> = text.trim_end_matches('\n').lines().collect();
    if lines.len() <= limit {
        return (lines.join("\n"), 0);
    }
    (lines[..limit].join("\n"), lines.len() - limit)
}

/// Draws the block held by `entry`.
pub fn render(entry: &TimelineEntry, _window: &mut Window, cx: &mut App) -> AnyElement {
    let BlockKind::Shell {
        command,
        exit_code,
        stdout,
        stderr,
        timed_out,
        truncated,
    } = &entry.block.kind
    else {
        return super::placeholder(entry, cx);
    };
    let theme = cx.theme();
    let ok = *exit_code == Some(0) && !timed_out;
    let status_color = if ok { theme.success } else { theme.danger };
    let header = h_flex()
        .gap_2()
        .child(
            div()
                .text_color(theme.primary)
                .font_weight(FontWeight::BOLD)
                .child("$"),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(SharedString::from(command.clone())),
        )
        .child(
            div()
                .text_xs()
                .text_color(status_color)
                .child(SharedString::from(status_label(*exit_code, *timed_out))),
        );
    let limit = if entry.expanded {
        usize::MAX
    } else {
        COLLAPSED_LINES
    };
    let mut hidden = 0;
    let mut body = v_flex().pl_4().text_sm();
    for (text, color) in [(stdout, theme.foreground), (stderr, theme.danger)] {
        if text.trim().is_empty() {
            continue;
        }
        let (shown, left) = head(text, limit);
        hidden += left;
        body = body.child(
            div()
                .text_color(color)
                .whitespace_normal()
                .child(SharedString::from(shown)),
        );
    }
    let mut notes = Vec::new();
    if hidden > 0 {
        notes.push(format!("{hidden} more lines · Enter to expand"));
    }
    if *truncated {
        notes.push("output cut to the size limit".to_owned());
    }
    if !notes.is_empty() {
        body = body.child(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(SharedString::from(notes.join(" · "))),
        );
    }
    v_flex()
        .gap_1()
        .font_family(theme.mono_font_family.clone())
        .child(header)
        .child(body)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_and_head() {
        assert_eq!(status_label(Some(0), false), "exit 0");
        assert_eq!(status_label(Some(2), false), "exit 2");
        assert_eq!(status_label(None, true), "timed out");
        assert_eq!(status_label(None, false), "killed");
        assert_eq!(head("a\nb\nc\n", 2), ("a\nb".to_owned(), 1));
        assert_eq!(head("a\n", 2), ("a".to_owned(), 0));
    }
}
