//! The terminal shared by the headless sink and the stdin approval: one
//! output stream, optional ANSI colors and the line state between writes.

use std::collections::HashSet;
use std::io::{self, IsTerminal, Write};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// ANSI style of a piece of output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    /// Secondary information (thinking, usage, retries).
    Dim,
    /// Headings and questions.
    Bold,
    /// Successes and added lines.
    Green,
    /// Errors and removed lines.
    Red,
    /// Warnings and denials.
    Yellow,
    /// Tool calls and diff hunks.
    Cyan,
}

impl Style {
    const fn code(self) -> &'static str {
        match self {
            Self::Dim => "2",
            Self::Bold => "1",
            Self::Green => "32",
            Self::Red => "31",
            Self::Yellow => "33",
            Self::Cyan => "36",
        }
    }
}

struct State {
    out: Box<dyn Write + Send>,
    color: bool,
    at_line_start: bool,
    shown_diffs: HashSet<String>,
}

/// Output of the headless mode. Cloning it shares the stream.
#[derive(Clone)]
pub struct Terminal {
    state: Arc<Mutex<State>>,
}

impl std::fmt::Debug for Terminal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Terminal").finish_non_exhaustive()
    }
}

/// Exclusive access to the terminal for one burst of output.
pub struct Writer<'a> {
    state: MutexGuard<'a, State>,
}

impl Terminal {
    /// A terminal writing to `out`, with colors when `color` is set.
    #[must_use]
    pub fn new(out: Box<dyn Write + Send>, color: bool) -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                out,
                color,
                at_line_start: true,
                shown_diffs: HashSet::new(),
            })),
        }
    }

    /// Standard output, colored when it is a TTY and `NO_COLOR` is unset.
    #[must_use]
    pub fn stdout() -> Self {
        let color = io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none();
        Self::new(Box::new(io::stdout()), color)
    }

    /// Locks the terminal for writing.
    #[must_use]
    pub fn lock(&self) -> Writer<'_> {
        Writer {
            state: self.state.lock().unwrap_or_else(PoisonError::into_inner),
        }
    }
}

impl Writer<'_> {
    /// Writes `text` as is (streamed model text).
    pub fn raw(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.write(text);
        self.state.at_line_start = text.ends_with('\n');
    }

    /// Writes `text` in `style`.
    pub fn styled(&mut self, style: Style, text: &str) {
        if text.is_empty() {
            return;
        }
        if self.state.color {
            let code = style.code();
            self.write(&format!("\x1b[{code}m{text}\x1b[0m"));
        } else {
            self.write(text);
        }
        self.state.at_line_start = text.ends_with('\n');
    }

    /// Writes a whole line in `style`, starting a new line first if the
    /// cursor is in the middle of one.
    pub fn line(&mut self, style: Option<Style>, text: &str) {
        self.break_line();
        match style {
            Some(style) => self.styled(style, text),
            None => self.raw(text),
        }
        self.raw("\n");
    }

    /// Records that the cursor is at the start of a line because something
    /// else (the user's terminal echoing an answer) moved it there.
    pub fn assume_line_start(&mut self) {
        self.state.at_line_start = true;
    }

    /// Ends the current line, if any.
    pub fn break_line(&mut self) {
        if !self.state.at_line_start {
            self.raw("\n");
        }
    }

    /// Prints a unified diff, colored by line, unless the same diff was
    /// already printed.
    pub fn diff(&mut self, path: &str, unified: &str) {
        if !self.state.shown_diffs.insert(unified.to_owned()) {
            return;
        }
        self.line(Some(Style::Bold), &format!("  diff {path}"));
        for line in unified.lines() {
            let style = if line.starts_with("+++") || line.starts_with("---") {
                Style::Bold
            } else if line.starts_with('+') {
                Style::Green
            } else if line.starts_with('-') {
                Style::Red
            } else if line.starts_with("@@") {
                Style::Cyan
            } else {
                Style::Dim
            };
            self.line(Some(style), &format!("  {line}"));
        }
    }

    /// Flushes the stream; errors are ignored (a closed stdout must not
    /// stop the turn).
    pub fn flush(&mut self) {
        let _ = self.state.out.flush();
    }

    fn write(&mut self, text: &str) {
        let _ = self.state.out.write_all(text.as_bytes());
    }
}

impl Drop for Writer<'_> {
    fn drop(&mut self) {
        self.flush();
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A `Write` whose bytes can be read back.
    #[derive(Clone, Default)]
    pub(crate) struct Buffer(Arc<Mutex<Vec<u8>>>);

    impl Buffer {
        pub(crate) fn text(&self) -> String {
            String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
        }
    }

    impl Write for Buffer {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    pub(crate) fn terminal(color: bool) -> (Terminal, Buffer) {
        let buf = Buffer::default();
        (Terminal::new(Box::new(buf.clone()), color), buf)
    }

    #[test]
    fn lines_start_after_streamed_text() {
        let (term, buf) = terminal(false);
        let mut w = term.lock();
        w.raw("hola");
        w.line(None, "→ grep");
        w.line(None, "fin");
        drop(w);
        assert_eq!(buf.text(), "hola\n→ grep\nfin\n");
    }

    #[test]
    fn colors_only_when_enabled() {
        let (term, buf) = terminal(true);
        term.lock().styled(Style::Red, "x");
        assert_eq!(buf.text(), "\x1b[31mx\x1b[0m");
        let (term, buf) = terminal(false);
        term.lock().styled(Style::Red, "x");
        assert_eq!(buf.text(), "x");
    }

    #[test]
    fn a_diff_is_printed_once() {
        let (term, buf) = terminal(false);
        let unified = "--- a/x\n+++ b/x\n@@ -1 +1 @@\n-a\n+b\n";
        term.lock().diff("x", unified);
        term.lock().diff("x", unified);
        assert_eq!(
            buf.text(),
            "  diff x\n  --- a/x\n  +++ b/x\n  @@ -1 +1 @@\n  -a\n  +b\n"
        );
    }
}
