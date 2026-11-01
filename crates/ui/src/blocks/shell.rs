//! Renderer of the `shell` block kind: a `/!` command and what it printed.
//!
//! `$ command` with its exit status, then the output in the monospace
//! font, stderr in the error color. Long output shows its first
//! [`COLLAPSED_LINES`] lines until the block is expanded (`Enter` on the
//! selected block, or a click).
//!
//! ANSI colors and attributes in the output are drawn (see
//! [`super::ansi`]); the 16 basic colors take the theme's `ansi.*`
//! tokens, so a red error looks like the theme's red.

use domain::block::BlockKind;
use gpui::{
    AnyElement, App, FontStyle, FontWeight, HighlightStyle, Hsla, IntoElement as _,
    ParentElement as _, SharedString, Styled as _, StyledText, UnderlineStyle, Window, div, px,
    rgb,
};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};

use super::ansi::{self, Color, Style};
use crate::theme::token;
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

/// The theme token of basic ANSI color `index` (0-15) as a foreground
/// (`background: false`) or a background. Bright colors share the token
/// of their normal one; black and bright black are the muted text (a
/// muted surface behind text), white is the default text.
#[must_use]
pub fn basic_token(index: u8, background: bool) -> &'static str {
    match index % 8 {
        0 if background => "muted.bg",
        0 => "muted.fg",
        1 => "ansi.red",
        2 => "ansi.green",
        3 => "ansi.yellow",
        4 => "ansi.blue",
        5 => "ansi.magenta",
        6 => "ansi.cyan",
        _ => "foreground",
    }
}

fn color(color: Color, background: bool, cx: &App) -> Hsla {
    match color {
        Color::Basic(index) => token(cx, basic_token(index, background)),
        Color::Rgb(r, g, b) => rgb(u32::from_be_bytes([0, r, g, b])).into(),
    }
}

/// The highlight of an ANSI style over text drawn in `base`.
fn highlight(style: Style, base: Hsla, cx: &App) -> HighlightStyle {
    let mut fg = style.fg.map(|c| color(c, false, cx));
    let mut bg = style.bg.map(|c| color(c, true, cx));
    if style.inverse {
        let swapped_fg = bg.unwrap_or(cx.theme().background);
        bg = Some(fg.unwrap_or(base));
        fg = Some(swapped_fg);
    }
    HighlightStyle {
        color: fg,
        background_color: bg,
        font_weight: style.bold.then_some(FontWeight::BOLD),
        font_style: style.italic.then_some(FontStyle::Italic),
        underline: style.underline.then(|| UnderlineStyle {
            thickness: px(1.),
            color: None,
            wavy: false,
        }),
        fade_out: style.dim.then_some(0.4),
        ..HighlightStyle::default()
    }
}

/// One stream as drawn: its shown lines without escapes, and their ANSI
/// runs (`None` when it has no escapes).
struct Shown {
    stderr: bool,
    text: SharedString,
    runs: Option<Vec<ansi::Run>>,
}

/// The output of a shell block cut to `limit` lines per stream and parsed,
/// with how many lines were left out. Made once per revision of the block
/// and expansion ([`super::derived`]), not on every frame.
struct Output {
    streams: Vec<Shown>,
    hidden: usize,
}

fn shown_output(stdout: &str, stderr: &str, limit: usize) -> Output {
    let mut streams = Vec::new();
    let mut hidden = 0;
    for (text, is_stderr) in [(stdout, false), (stderr, true)] {
        if text.trim().is_empty() {
            continue;
        }
        let (shown, left) = head(text, limit);
        hidden += left;
        streams.push(if ansi::has_escapes(&shown) {
            let styled = ansi::parse(&shown);
            Shown {
                stderr: is_stderr,
                text: styled.text.into(),
                runs: Some(styled.runs),
            }
        } else {
            Shown {
                stderr: is_stderr,
                text: shown.into(),
                runs: None,
            }
        });
    }
    Output { streams, hidden }
}

/// A stream with its ANSI styling, drawn in `base` where it has none.
fn output(shown: &Shown, base: Hsla, cx: &App) -> AnyElement {
    let container = div().text_color(base).whitespace_normal();
    let Some(runs) = &shown.runs else {
        return container.child(shown.text.clone()).into_any_element();
    };
    let highlights: Vec<_> = runs
        .iter()
        .map(|run| (run.range.clone(), highlight(run.style, base, cx)))
        .collect();
    container
        .child(StyledText::new(shown.text.clone()).with_highlights(highlights))
        .into_any_element()
}

/// Draws the block held by `entry`.
pub fn render(entry: &TimelineEntry, window: &mut Window, cx: &mut App) -> AnyElement {
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
    let shown = super::derived(
        entry.block.id,
        entry.rev,
        entry.expanded,
        window,
        cx,
        |&expanded| {
            let limit = if expanded {
                usize::MAX
            } else {
                COLLAPSED_LINES
            };
            shown_output(stdout, stderr, limit)
        },
    );
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
    let hidden = shown.hidden;
    let mut body = v_flex().pl_4().text_sm();
    for stream in &shown.streams {
        let color = if stream.stderr {
            theme.danger
        } else {
            theme.foreground
        };
        body = body.child(output(stream, color, cx));
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

    #[test]
    fn basic_colors_take_theme_tokens() {
        assert_eq!(basic_token(1, false), "ansi.red");
        assert_eq!(basic_token(9, false), "ansi.red");
        assert_eq!(basic_token(4, true), "ansi.blue");
        assert_eq!(basic_token(0, false), "muted.fg");
        assert_eq!(basic_token(0, true), "muted.bg");
        assert_eq!(basic_token(8, false), "muted.fg");
        assert_eq!(basic_token(15, false), "foreground");
    }

    #[gpui::test]
    fn draws_colored_output(cx: &mut gpui::TestAppContext) {
        use application_core::ports::events::DomainEvent;
        use domain::block::Block;

        use crate::timeline::TimelineView;

        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, cx| TimelineView::new(cx));
        let block = Block::new(BlockKind::Shell {
            command: "cargo test".into(),
            exit_code: Some(101),
            stdout: "\x1b[1m\x1b[32mok\x1b[0m\n\x1b[38;5;196mred\x1b[0m \x1b]8;;https://x.y\x1b\\link\x1b]8;;\x1b\\\n".into(),
            stderr: "\x1b[31merror\x1b[0m: failed\n".into(),
            timed_out: false,
            truncated: false,
        });
        view.update(cx, |view, cx| {
            view.apply_all([DomainEvent::BlockAdded { block }], cx);
        });
        cx.draw(
            gpui::point(gpui::px(0.), gpui::px(0.)),
            gpui::size(gpui::px(800.), gpui::px(600.)),
            |_, _| div().size_full().child(view.clone()),
        );
        cx.run_until_parked();
    }
}
