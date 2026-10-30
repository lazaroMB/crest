//! The picker's preview: the lines around the highlighted row, numbered,
//! with syntax colors and the match marked.
//!
//! A text match is previewed at once from the context lines `grep` already
//! returned ([`Preview::of_context`]); the file itself is read through the
//! [`FileSystemPort`](application::ports::fs::FileSystemPort) after
//! [`PREVIEW_DEBOUNCE`] ([`preview_request`]) and replaces it.

use std::ops::Range;
use std::time::Duration;

use application::ports::fs::{FileContent, ReadFileRequest};
use application::ports::search::ColumnRange;
use domain::workspace::WorkspacePath;
use gpui::HighlightStyle;
use gpui_component::Rope;
use gpui_component::highlighter::{HighlightTheme, Language, LanguageRegistry, SyntaxHighlighter};

use super::PickerItem;

/// Wait after the highlight moves before reading the file it is on.
pub const PREVIEW_DEBOUNCE: Duration = Duration::from_millis(80);
/// Lines shown above a matching line.
pub const LINES_BEFORE: usize = 10;
/// Lines read for a preview.
pub const PREVIEW_LINES: usize = 80;

/// Syntax styles of one line: byte ranges within it, in order.
pub type LineStyles = Vec<(Range<usize>, HighlightStyle)>;

/// The read the preview of `item` asks for: from [`LINES_BEFORE`] lines
/// above its line (or the start of the file), [`PREVIEW_LINES`] lines.
pub fn preview_request(item: &PickerItem) -> ReadFileRequest {
    let start = item.line.map_or(1, |line| {
        usize::try_from(line)
            .unwrap_or(usize::MAX)
            .saturating_sub(LINES_BEFORE)
            .max(1)
    });
    ReadFileRequest {
        path: item.path.clone(),
        offset: Some(start),
        limit: Some(PREVIEW_LINES),
    }
}

/// What the preview shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preview {
    /// The file.
    pub path: WorkspacePath,
    /// The 1-based line of the match, for a text match.
    pub line: Option<u64>,
    /// 1-based number of `lines[0]`.
    pub start_line: usize,
    /// The lines shown, without terminators.
    pub lines: Vec<String>,
    /// Range of the match within the line `line`.
    pub column_range: Option<ColumnRange>,
    /// The file is still being read.
    pub loading: bool,
    /// Why the file could not be read.
    pub error: Option<String>,
}

impl Preview {
    /// What is known of `item` before reading its file: the context lines
    /// of a text match, nothing for a file.
    pub fn of_context(item: &PickerItem) -> Self {
        let (start_line, lines) = match (item.line, &item.text) {
            (Some(line), Some(text)) => {
                let line = usize::try_from(line).unwrap_or(usize::MAX);
                let start = line.saturating_sub(item.context_before.len()).max(1);
                let mut lines = item.context_before.clone();
                lines.push(text.clone());
                lines.extend(item.context_after.iter().cloned());
                (start, lines)
            }
            _ => (1, Vec::new()),
        };
        Self {
            path: item.path.clone(),
            line: item.line,
            start_line,
            lines,
            column_range: item.column_range,
            loading: true,
            error: None,
        }
    }

    /// The preview of `item` once its file was read.
    pub fn read(item: &PickerItem, content: FileContent) -> Self {
        Self {
            start_line: content.start_line,
            lines: content.lines,
            loading: false,
            ..Self::of_context(item)
        }
    }

    /// The preview of `item` whose file could not be read: the context
    /// lines, if any, and the reason.
    pub fn failed(item: &PickerItem, error: String) -> Self {
        Self {
            loading: false,
            error: Some(error),
            ..Self::of_context(item)
        }
    }

    /// Whether this previews `item`.
    pub fn is_for(&self, item: &PickerItem) -> bool {
        self.path == item.path && self.line == item.line
    }

    /// Index in `lines` of the matching line, if shown.
    pub fn match_index(&self) -> Option<usize> {
        let line = usize::try_from(self.line?).ok()?;
        let index = line.checked_sub(self.start_line)?;
        (index < self.lines.len()).then_some(index)
    }
}

/// The highlighter language of `path`, from its extension, when a grammar
/// for it is compiled in.
pub fn language_of(path: &WorkspacePath) -> Option<&'static str> {
    let name = path.file_name()?;
    let (_, extension) = name.rsplit_once('.')?;
    let language = Language::from_str(&extension.to_ascii_lowercase());
    if language == Language::Plain {
        return None;
    }
    let name = language.name();
    LanguageRegistry::singleton().language(name)?;
    Some(name)
}

/// Syntax styles of each of `lines` of `path`, highlighted together as one
/// text with `theme`. Lines of a language without a grammar get none.
pub fn syntax_styles(
    path: &WorkspacePath,
    lines: &[String],
    theme: &HighlightTheme,
) -> Vec<LineStyles> {
    let mut out = vec![LineStyles::new(); lines.len()];
    let Some(language) = language_of(path) else {
        return out;
    };
    let text = lines.join("\n");
    let mut highlighter = SyntaxHighlighter::new(language);
    highlighter.update(None, &Rope::from_str(&text), None);
    let styles = highlighter.styles(&(0..text.len()), theme);
    let mut start = 0;
    for (line, line_styles) in lines.iter().zip(out.iter_mut()) {
        let end = start + line.len();
        for (range, style) in &styles {
            let (a, b) = (range.start.max(start), range.end.min(end));
            if a < b && *style != HighlightStyle::default() {
                line_styles.push((a - start..b - start, *style));
            }
        }
        start = end + 1;
    }
    out
}

/// `styles` with `extra` added over `mark`: ranges are split where the
/// mark starts and ends, and the part of `mark` no style covers gets
/// `extra` alone. `styles` must be ordered and not overlap.
pub fn overlay(
    styles: &[(Range<usize>, HighlightStyle)],
    mark: Range<usize>,
    extra: HighlightStyle,
) -> LineStyles {
    let mut cuts: Vec<usize> = styles
        .iter()
        .flat_map(|(range, _)| [range.start, range.end])
        .chain([mark.start, mark.end])
        .collect();
    cuts.sort_unstable();
    cuts.dedup();
    let mut out = LineStyles::new();
    for (&a, &b) in cuts.iter().zip(cuts.iter().skip(1)) {
        let base = styles
            .iter()
            .find(|(range, _)| range.start <= a && b <= range.end)
            .map(|(_, style)| *style);
        let marked = mark.start <= a && b <= mark.end;
        let style = match (base, marked) {
            (Some(style), true) => style.highlight(extra),
            (Some(style), false) => style,
            (None, true) => extra,
            (None, false) => continue,
        };
        out.push((a..b, style));
    }
    out
}

/// `range` when it is a non-empty range of whole characters of `text`.
pub fn valid_range(text: &str, range: ColumnRange) -> Option<Range<usize>> {
    (range.start < range.end
        && range.end <= text.len()
        && text.is_char_boundary(range.start)
        && text.is_char_boundary(range.end))
    .then_some(range.start..range.end)
}

#[cfg(test)]
mod tests {
    use gpui::{FontWeight, Hsla};

    use super::*;

    fn p(s: &str) -> WorkspacePath {
        WorkspacePath::parse(s).unwrap()
    }

    fn grep_item() -> PickerItem {
        PickerItem {
            path: p("src/main.rs"),
            line: Some(12),
            text: Some("    run();".into()),
            column_range: Some(ColumnRange { start: 4, end: 7 }),
            context_before: vec!["a".into(), "b".into()],
            context_after: vec!["c".into()],
            recent: false,
        }
    }

    #[test]
    fn the_read_starts_above_the_match() {
        let request = preview_request(&grep_item());
        assert_eq!(
            (request.offset, request.limit),
            (Some(2), Some(PREVIEW_LINES))
        );
        let near_top = PickerItem {
            line: Some(3),
            ..grep_item()
        };
        assert_eq!(preview_request(&near_top).offset, Some(1));
        let file = PickerItem {
            line: None,
            text: None,
            ..grep_item()
        };
        assert_eq!(preview_request(&file).offset, Some(1));
    }

    #[test]
    fn context_previews_the_match_until_the_file_is_read() {
        let item = grep_item();
        let preview = Preview::of_context(&item);
        assert_eq!(preview.start_line, 10);
        assert_eq!(preview.lines, ["a", "b", "    run();", "c"]);
        assert_eq!(preview.match_index(), Some(2));
        assert!(preview.loading && preview.is_for(&item));

        let content = FileContent::from_text(
            &(1..=30).map(|n| format!("line {n}\n")).collect::<String>(),
            &preview_request(&item),
        );
        let read = Preview::read(&item, content);
        assert_eq!((read.start_line, read.lines.len()), (2, 29));
        assert_eq!(read.match_index(), Some(10));
        assert!(!read.loading);

        let failed = Preview::failed(&item, "file is binary".into());
        assert_eq!(failed.lines.len(), 4);
        assert_eq!(failed.error.as_deref(), Some("file is binary"));
    }

    #[test]
    fn languages_come_from_the_extension() {
        assert_eq!(language_of(&p("src/main.rs")), Some("rust"));
        assert_eq!(language_of(&p("a/b.PY")), Some("python"));
        assert_eq!(language_of(&p("notes.txt")), None);
        assert_eq!(language_of(&p("Makefile")), None);
    }

    #[test]
    fn rust_lines_get_syntax_styles_per_line() {
        let lines = vec![
            "fn main() {".to_owned(),
            "    let x = 1;".to_owned(),
            "}".to_owned(),
        ];
        let styles = syntax_styles(&p("main.rs"), &lines, &HighlightTheme::default_dark());
        assert_eq!(styles.len(), 3);
        assert!(
            styles[0].iter().any(|(range, _)| *range == (0..2)),
            "{:?}",
            styles[0]
        );
        for (line, styles) in lines.iter().zip(&styles) {
            assert!(styles.iter().all(|(range, _)| range.end <= line.len()));
        }
        let plain = syntax_styles(&p("notes.txt"), &lines, &HighlightTheme::default_dark());
        assert!(plain.iter().all(Vec::is_empty));
    }

    #[test]
    fn the_mark_splits_the_styles_it_crosses() {
        let red = HighlightStyle {
            color: Some(Hsla::red()),
            ..Default::default()
        };
        let bold = HighlightStyle {
            font_weight: Some(FontWeight::BOLD),
            ..Default::default()
        };
        let out = overlay(&[(0..4, red), (6..8, red)], 2..7, bold);
        assert_eq!(
            out,
            [
                (0..2, red),
                (2..4, red.highlight(bold)),
                (4..6, bold),
                (6..7, red.highlight(bold)),
                (7..8, red),
            ]
        );
        assert_eq!(overlay(&[], 1..3, bold), [(1..3, bold)]);
    }

    #[test]
    fn only_whole_character_ranges_are_marked() {
        let range = |start, end| ColumnRange { start, end };
        assert_eq!(valid_range("abc", range(1, 3)), Some(1..3));
        assert_eq!(valid_range("abc", range(1, 4)), None);
        assert_eq!(valid_range("abc", range(2, 2)), None);
        assert_eq!(valid_range("é", range(1, 2)), None);
    }
}
