//! ANSI escape sequences in command output, as text plus styled runs.
//!
//! [`parse`] feeds the bytes through `anstyle-parse` and keeps what a
//! terminal would show: printable text, newlines and tabs, with the SGR
//! state (`ESC [ … m`: 16, 256 and 24-bit colors, bold, dim, italic,
//! underline, reset) recorded as [`Run`]s over byte ranges of the text.
//! Every other sequence (cursor moves, erases, titles and other OSC
//! commands) is dropped; an OSC 8 hyperlink keeps its text, underlined.
//! A carriage return not followed by a newline starts the line over, so
//! progress bars show their last state.
//!
//! Colors stay symbolic ([`Color`]); the shell block maps the 16 basic
//! ones to theme tokens.

use std::ops::Range;

use anstyle_parse::{DefaultCharAccumulator, Params, Parser, Perform};

/// A color as the output named it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Color {
    /// One of the 16 basic colors: 0-7 normal, 8-15 bright.
    Basic(u8),
    /// A 24-bit color (also used for 256-color indexes 16-255).
    Rgb(u8, u8, u8),
}

/// The SGR attributes in effect for a [`Run`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Style {
    /// Foreground color, `None` for the default.
    pub fg: Option<Color>,
    /// Background color, `None` for the default.
    pub bg: Option<Color>,
    /// Bold (SGR 1).
    pub bold: bool,
    /// Faint (SGR 2).
    pub dim: bool,
    /// Italic (SGR 3).
    pub italic: bool,
    /// Underlined (SGR 4, or inside an OSC 8 hyperlink).
    pub underline: bool,
    /// Swapped foreground and background (SGR 7).
    pub inverse: bool,
}

impl Style {
    /// Whether the style changes nothing from the default.
    #[must_use]
    pub fn is_plain(&self) -> bool {
        *self == Self::default()
    }
}

/// A styled span of [`Styled::text`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    /// Byte range in the text.
    pub range: Range<usize>,
    /// Its style (never plain).
    pub style: Style,
}

/// Output with the escapes taken out.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Styled {
    /// What a terminal would print.
    pub text: String,
    /// Styled spans, in order and not overlapping; plain text has none.
    pub runs: Vec<Run>,
}

/// Parses `input` into text and styled runs.
#[must_use]
pub fn parse(input: &str) -> Styled {
    let mut parser = Parser::<DefaultCharAccumulator>::new();
    let mut performer = Performer::default();
    for byte in input.bytes() {
        parser.advance(&mut performer, byte);
    }
    performer.finish()
}

/// Whether `input` has any escape sequence (the fast path skips parsing).
#[must_use]
pub fn has_escapes(input: &str) -> bool {
    input.contains('\x1b') || input.contains('\u{9b}') || input.contains('\r')
}

/// The RGB of a 256-color index (16-231 the 6×6×6 cube, 232-255 grays).
/// Indexes below 16 are the basic colors and come back as [`Color::Basic`].
#[must_use]
pub fn indexed(index: u8) -> Color {
    match index {
        0..=15 => Color::Basic(index),
        16..=231 => {
            let i = index - 16;
            let level = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            Color::Rgb(level(i / 36), level((i / 6) % 6), level(i % 6))
        }
        _ => {
            let v = 8 + (index - 232) * 10;
            Color::Rgb(v, v, v)
        }
    }
}

#[derive(Default)]
struct Performer {
    out: Styled,
    style: Style,
    /// Byte offset where the current style started.
    run_start: usize,
    /// Byte offset of the start of the current line.
    line_start: usize,
    /// A carriage return arrived and no newline followed it yet.
    pending_cr: bool,
    /// Inside an OSC 8 hyperlink.
    in_link: bool,
}

impl Performer {
    /// The style text gets now: SGR plus the hyperlink underline.
    fn effective(&self) -> Style {
        Style {
            underline: self.style.underline || self.in_link,
            ..self.style
        }
    }

    /// Closes the run of the current style at the end of the text.
    fn close_run(&mut self) {
        let end = self.out.text.len();
        let style = self.effective();
        if end > self.run_start && !style.is_plain() {
            // Merge with the previous run when it is the same style and
            // touches this one.
            match self.out.runs.last_mut() {
                Some(last) if last.style == style && last.range.end == self.run_start => {
                    last.range.end = end;
                }
                _ => self.out.runs.push(Run {
                    range: self.run_start..end,
                    style,
                }),
            }
        }
        self.run_start = end;
    }

    /// Changes the style, closing the run of the old one.
    fn restyle(&mut self, change: impl FnOnce(&mut Self)) {
        self.close_run();
        change(self);
    }

    /// Applies a carriage return that was not part of `\r\n`: the line is
    /// printed again from its start, so the old text goes.
    fn apply_cr(&mut self) {
        if !self.pending_cr {
            return;
        }
        self.pending_cr = false;
        self.close_run();
        let start = self.line_start;
        self.out.text.truncate(start);
        self.out.runs.retain_mut(|run| {
            run.range.end = run.range.end.min(start);
            run.range.start < run.range.end
        });
        self.run_start = start;
    }

    fn push(&mut self, c: char) {
        self.apply_cr();
        self.out.text.push(c);
    }

    fn sgr(&mut self, params: &Params) {
        let groups: Vec<&[u16]> = params.iter().collect();
        if groups.is_empty() {
            self.restyle(|p| p.style = Style::default());
            return;
        }
        let mut style = self.style;
        let mut i = 0;
        while i < groups.len() {
            let group = groups[i];
            let code = group.first().copied().unwrap_or(0);
            match code {
                0 => style = Style::default(),
                1 => style.bold = true,
                2 => style.dim = true,
                3 => style.italic = true,
                4 => style.underline = group.get(1).is_none_or(|&kind| kind != 0),
                7 => style.inverse = true,
                21 | 24 => style.underline = false,
                22 => (style.bold, style.dim) = (false, false),
                23 => style.italic = false,
                27 => style.inverse = false,
                30..=37 => style.fg = Some(Color::Basic((code - 30) as u8)),
                39 => style.fg = None,
                40..=47 => style.bg = Some(Color::Basic((code - 40) as u8)),
                49 => style.bg = None,
                90..=97 => style.fg = Some(Color::Basic((code - 90 + 8) as u8)),
                100..=107 => style.bg = Some(Color::Basic((code - 100 + 8) as u8)),
                38 | 48 => {
                    let (color, used) = extended(group, &groups[i + 1..]);
                    i += used;
                    if let Some(color) = color {
                        if code == 38 {
                            style.fg = Some(color);
                        } else {
                            style.bg = Some(color);
                        }
                    }
                }
                _ => {}
            }
            i += 1;
        }
        self.restyle(|p| p.style = style);
    }

    fn finish(mut self) -> Styled {
        self.close_run();
        self.out
    }
}

/// An extended color after 38/48: in the colon form (`38:5:n`,
/// `38:2::r:g:b`) everything is in `group`; in the semicolon form
/// (`38;5;n`, `38;2;r;g;b`) it takes the following groups. Returns the
/// color and how many following groups it used.
fn extended(group: &[u16], rest: &[&[u16]]) -> (Option<Color>, usize) {
    let byte = |v: u16| u8::try_from(v).ok();
    if group.len() > 1 {
        let color = match group.get(1) {
            Some(5) => group.get(2).and_then(|&n| byte(n)).map(indexed),
            Some(2) => {
                // `38:2:r:g:b` or `38:2:<colorspace>:r:g:b`.
                let rgb = if group.len() >= 6 {
                    &group[3..6]
                } else {
                    &group[2..group.len().min(5)]
                };
                match rgb {
                    [r, g, b] => match (byte(*r), byte(*g), byte(*b)) {
                        (Some(r), Some(g), Some(b)) => Some(Color::Rgb(r, g, b)),
                        _ => None,
                    },
                    _ => None,
                }
            }
            _ => None,
        };
        return (color, 0);
    }
    let first = |ix: usize| rest.get(ix).and_then(|g| g.first()).copied();
    match first(0) {
        Some(5) => (first(1).and_then(byte).map(indexed), 2),
        Some(2) => {
            let rgb = (
                first(1).and_then(byte),
                first(2).and_then(byte),
                first(3).and_then(byte),
            );
            match rgb {
                (Some(r), Some(g), Some(b)) => (Some(Color::Rgb(r, g, b)), 4),
                _ => (None, 4.min(rest.len())),
            }
        }
        _ => (None, 0),
    }
}

impl Perform for Performer {
    fn print(&mut self, c: char) {
        self.push(c);
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' => {
                // `\r\n` is a plain newline.
                self.pending_cr = false;
                self.out.text.push('\n');
                self.line_start = self.out.text.len();
            }
            b'\r' => self.pending_cr = true,
            b'\t' => self.push('\t'),
            // Backspace takes back one character of the line.
            0x08 => {
                self.apply_cr();
                if self.out.text.len() > self.line_start.max(self.run_start) {
                    self.out.text.pop();
                }
            }
            _ => {}
        }
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        // OSC 8 ; params ; uri: a non-empty uri opens a link, an empty one
        // closes it.
        if params.first() == Some(&b"8".as_slice()) {
            let open = params.get(2).is_some_and(|uri| !uri.is_empty());
            if open != self.in_link {
                self.restyle(|p| p.in_link = open);
            }
        }
    }

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], ignore: bool, action: u8) {
        if action == b'm' && intermediates.is_empty() && !ignore {
            self.sgr(params);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runs(input: &str) -> Vec<(String, Style)> {
        let styled = parse(input);
        styled
            .runs
            .iter()
            .map(|run| (styled.text[run.range.clone()].to_owned(), run.style))
            .collect()
    }

    fn fg(color: Color) -> Style {
        Style {
            fg: Some(color),
            ..Style::default()
        }
    }

    #[test]
    fn plain_text_has_no_runs() {
        let styled = parse("hello\nworld\n");
        assert_eq!(styled.text, "hello\nworld\n");
        assert!(styled.runs.is_empty());
        assert!(!has_escapes("hello"));
        assert!(has_escapes("\x1b[1m"));
    }

    #[test]
    fn basic_and_bright_colors_and_reset() {
        let styled = parse("\x1b[31merror\x1b[0m: \x1b[92mok\x1b[39m done");
        assert_eq!(styled.text, "error: ok done");
        assert_eq!(
            runs("\x1b[31merror\x1b[0m: \x1b[92mok\x1b[39m done"),
            vec![
                ("error".into(), fg(Color::Basic(1))),
                ("ok".into(), fg(Color::Basic(10))),
            ]
        );
        let bg = runs("\x1b[44mx\x1b[49m\x1b[103my");
        assert_eq!(bg[0].1.bg, Some(Color::Basic(4)));
        assert_eq!(bg[1].1.bg, Some(Color::Basic(11)));
    }

    #[test]
    fn attributes_combine_and_turn_off() {
        let all = runs("\x1b[1;2;3;4mx\x1b[22;23;24my\x1b[7mz\x1b[mw");
        assert_eq!(
            all[0].1,
            Style {
                bold: true,
                dim: true,
                italic: true,
                underline: true,
                ..Style::default()
            }
        );
        assert_eq!(all.len(), 2, "y is plain again: {all:?}");
        assert!(all[1].1.inverse);
        assert_eq!(
            parse("\x1b[1;2;3;4mx\x1b[22;23;24my\x1b[7mz\x1b[mw").text,
            "xyzw"
        );
    }

    #[test]
    fn extended_colors_in_both_forms() {
        assert_eq!(runs("\x1b[38;5;196mx")[0].1.fg, Some(Color::Rgb(255, 0, 0)));
        assert_eq!(runs("\x1b[38;5;3mx")[0].1.fg, Some(Color::Basic(3)));
        assert_eq!(
            runs("\x1b[38;5;244mx")[0].1.fg,
            Some(Color::Rgb(128, 128, 128))
        );
        assert_eq!(
            runs("\x1b[38;2;10;20;30mx")[0].1.fg,
            Some(Color::Rgb(10, 20, 30))
        );
        assert_eq!(
            runs("\x1b[48;2;1;2;3;1mx")[0].1,
            Style {
                bg: Some(Color::Rgb(1, 2, 3)),
                bold: true,
                ..Style::default()
            }
        );
        assert_eq!(runs("\x1b[38:5:21mx")[0].1.fg, Some(Color::Rgb(0, 0, 255)));
        assert_eq!(
            runs("\x1b[38:2::4:5:6mx")[0].1.fg,
            Some(Color::Rgb(4, 5, 6))
        );
        assert_eq!(runs("\x1b[38:2:4:5:6mx")[0].1.fg, Some(Color::Rgb(4, 5, 6)));
        assert_eq!(indexed(16), Color::Rgb(0, 0, 0));
        assert_eq!(indexed(231), Color::Rgb(255, 255, 255));
        assert_eq!(indexed(232), Color::Rgb(8, 8, 8));
    }

    #[test]
    fn other_escapes_are_dropped() {
        // Cursor moves, erase line, a window title and a private mode.
        let styled = parse("\x1b[2K\x1b[1Ga\x1b]0;title\x07b\x1b[?25lc\x1b(Bd");
        assert_eq!(styled.text, "abcd");
        assert!(styled.runs.is_empty());
    }

    #[test]
    fn hyperlinks_keep_their_text_underlined() {
        let input = "see \x1b]8;;https://example.com\x1b\\the docs\x1b]8;;\x1b\\ now";
        assert_eq!(parse(input).text, "see the docs now");
        assert_eq!(
            runs(input),
            vec![(
                "the docs".into(),
                Style {
                    underline: true,
                    ..Style::default()
                }
            )]
        );
    }

    #[test]
    fn carriage_return_redraws_the_line() {
        assert_eq!(parse("10%\r50%\r100%\ndone\r\n").text, "100%\ndone\n");
        let styled = parse("a\n\x1b[31mxx\x1b[0m\ryy");
        assert_eq!(styled.text, "a\nyy");
        assert!(styled.runs.is_empty());
        assert_eq!(parse("ab\x08c").text, "ac");
    }

    #[test]
    fn same_style_runs_merge() {
        assert_eq!(
            runs("\x1b[31ma\x1b[31mb"),
            vec![("ab".into(), fg(Color::Basic(1)))]
        );
    }
}
