//! `grep`, built on `rg --json`.
//!
//! Runs `rg --json -n -C <context> [-F] [-i] [--glob <glob>] -- <pattern>
//! [path]` with the same walk flags as the listings (hidden files, ignore
//! files, `.git` and sensitive paths excluded) and parses its JSON Lines
//! output as it arrives: `begin` opens a file, `match` and `context` carry
//! lines, everything else (`end`, `summary`) is ignored. Once `limit`
//! matches are collected the next match stops `rg`, so a broad pattern does
//! not scan the whole workspace.
//!
//! `rg` exits with 1 when nothing matches (an empty result) and with 2 on
//! errors: a rejected pattern becomes [`SearchError::InvalidPattern`].

use std::collections::VecDeque;
use std::ops::ControlFlow;

use application::ports::search::{ColumnRange, GrepHit, GrepRequest, GrepResult, SearchError};
use base64::Engine;
use domain::workspace::{PathError, WorkspacePath};
use serde::Deserialize;

use crate::RipgrepSearch;
use crate::files::check_rg_status;
use crate::process::{self, ProcessSpec};

/// One line of `rg --json` output, before looking at its type.
#[derive(Debug, Deserialize)]
struct RawEvent {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    data: serde_json::Value,
}

/// The events `grep` uses; `end`, `summary` and unknown ones are `Other`.
#[derive(Debug)]
enum Event {
    Begin(Begin),
    Match(Line),
    Context(Line),
    Other,
}

impl Event {
    fn parse(line: &str) -> Result<Self, serde_json::Error> {
        let raw: RawEvent = serde_json::from_str(line)?;
        Ok(match raw.kind.as_str() {
            "begin" => Self::Begin(serde_json::from_value(raw.data)?),
            "match" => Self::Match(serde_json::from_value(raw.data)?),
            "context" => Self::Context(serde_json::from_value(raw.data)?),
            _ => Self::Other,
        })
    }
}

#[derive(Debug, Deserialize)]
struct Begin {
    path: Data,
}

#[derive(Debug, Deserialize)]
struct Line {
    lines: Data,
    line_number: Option<u64>,
    #[serde(default)]
    submatches: Vec<Submatch>,
}

#[derive(Debug, Deserialize)]
struct Submatch {
    start: usize,
    end: usize,
}

/// Text that `rg` prints as `{"text": ...}` when it is valid UTF-8 and as
/// base64 `{"bytes": ...}` otherwise.
#[derive(Debug, Deserialize)]
struct Data {
    text: Option<String>,
    bytes: Option<String>,
}

impl Data {
    /// The raw bytes.
    fn into_bytes(self) -> Result<Vec<u8>, SearchError> {
        match (self.text, self.bytes) {
            (Some(text), _) => Ok(text.into_bytes()),
            (None, Some(bytes)) => base64::engine::general_purpose::STANDARD
                .decode(bytes)
                .map_err(|e| SearchError::Parse(format!("invalid base64 in rg output: {e}"))),
            (None, None) => Err(SearchError::Parse("rg data without text or bytes".into())),
        }
    }
}

/// Searches file contents: what `ContentSearchPort::grep` returns.
pub async fn grep(search: &RipgrepSearch, request: GrepRequest) -> Result<GrepResult, SearchError> {
    let path = request.path.clone().filter(|p| !p.is_root());
    if let Some(path) = &path {
        if !search.is_allowed(path) {
            return Ok(GrepResult::default());
        }
        check_inside(search, path).await?;
    }
    let mut spec = ProcessSpec::new(&search.rg, search.root.as_path())
        .args(["--json", "-n", "-C"])
        .arg(request.context.to_string())
        .args(search.rg_common_args(request.glob.as_deref()));
    if request.fixed {
        spec = spec.arg("-F");
    }
    if request.ignore_case {
        spec = spec.arg("-i");
    }
    spec = spec.arg("--").arg(&request.pattern);
    if let Some(path) = &path {
        spec = spec.arg(path.as_str());
    }

    let mut collector = Collector::new(search, &request);
    let outcome = process::for_each_line(&spec, search.timeout, |line| collector.on_line(&line))
        .await
        .map_err(RipgrepSearch::rg_error)?;
    if let Some(err) = collector.error {
        return Err(err);
    }
    if outcome.timed_out {
        return Err(SearchError::Timeout {
            after: search.timeout,
        });
    }
    if !outcome.stopped {
        check_rg_status(outcome.status, &outcome.stderr, collector.saw_events)?;
    }
    Ok(GrepResult {
        hits: collector.hits,
        truncated: collector.truncated,
        total: collector.total,
    })
}

/// Fails unless `path` (after resolving symlinks) is inside the workspace:
/// `rg` follows a symlink given explicitly on the command line.
async fn check_inside(search: &RipgrepSearch, path: &WorkspacePath) -> Result<(), SearchError> {
    let full = search.root.join(path);
    let resolved = tokio::fs::canonicalize(&full)
        .await
        .map_err(|e| SearchError::Io(format!("{path}: {e}")))?;
    if resolved.starts_with(search.root.as_path()) {
        Ok(())
    } else {
        Err(PathError::OutsideWorkspace {
            path: path.to_string(),
        }
        .into())
    }
}

/// Turns `rg --json` events into hits.
struct Collector<'a> {
    search: &'a RipgrepSearch,
    context: usize,
    limit: usize,
    hits: Vec<GrepHit>,
    total: usize,
    truncated: bool,
    saw_events: bool,
    error: Option<SearchError>,
    /// The file being reported; `None` while skipping a file (sensitive or
    /// with an unusable path).
    file: Option<WorkspacePath>,
    /// Index in `hits` of the first hit of the current file.
    file_start: usize,
    /// The last `context` lines of the current file (matches included),
    /// to fill `context_before`.
    recent: VecDeque<(u64, String)>,
}

impl<'a> Collector<'a> {
    fn new(search: &'a RipgrepSearch, request: &GrepRequest) -> Self {
        Self {
            search,
            context: usize::try_from(request.context).unwrap_or(usize::MAX),
            limit: request.limit,
            hits: Vec::new(),
            total: 0,
            truncated: false,
            saw_events: false,
            error: None,
            file: None,
            file_start: 0,
            recent: VecDeque::new(),
        }
    }

    fn on_line(&mut self, line: &str) -> ControlFlow<()> {
        if line.is_empty() {
            return ControlFlow::Continue(());
        }
        let event = match Event::parse(line) {
            Ok(event) => event,
            Err(e) => {
                self.error = Some(SearchError::Parse(format!("{e}: {line}")));
                return ControlFlow::Break(());
            }
        };
        self.saw_events = true;
        let result = match event {
            Event::Begin(begin) => self.begin(begin),
            Event::Match(line) => self.line(line, true),
            Event::Context(line) => self.line(line, false),
            Event::Other => Ok(ControlFlow::Continue(())),
        };
        result.unwrap_or_else(|err| {
            self.error = Some(err);
            ControlFlow::Break(())
        })
    }

    fn begin(&mut self, begin: Begin) -> Result<ControlFlow<()>, SearchError> {
        let bytes = begin.path.into_bytes()?;
        self.file = String::from_utf8(bytes)
            .ok()
            .and_then(|text| WorkspacePath::parse(&text).ok())
            .filter(|path| !path.is_root() && self.search.is_allowed(path));
        self.file_start = self.hits.len();
        self.recent.clear();
        Ok(ControlFlow::Continue(()))
    }

    fn line(&mut self, line: Line, is_match: bool) -> Result<ControlFlow<()>, SearchError> {
        let Some(path) = &self.file else {
            return Ok(ControlFlow::Continue(()));
        };
        if is_match && self.hits.len() >= self.limit {
            self.total += 1;
            self.truncated = true;
            return Ok(ControlFlow::Break(()));
        }
        let number = line
            .line_number
            .ok_or_else(|| SearchError::Parse("rg line without line_number".into()))?;
        let raw = strip_terminator(line.lines.into_bytes()?);
        let text = String::from_utf8_lossy(&raw).into_owned();

        for hit in self.hits[self.file_start..].iter_mut().rev() {
            match number.checked_sub(hit.line_number) {
                Some(distance) if distance >= 1 && distance <= self.context as u64 => {
                    hit.context_after.push(text.clone());
                }
                _ => break,
            }
        }
        if is_match {
            let context_before = self
                .recent
                .iter()
                .filter(|(n, _)| *n < number && number - n <= self.context as u64)
                .map(|(_, text)| text.clone())
                .collect();
            let column_range = line
                .submatches
                .first()
                .map(|m| column_range(&raw, m))
                .unwrap_or(ColumnRange { start: 0, end: 0 });
            self.total += 1;
            self.hits.push(GrepHit {
                path: path.clone(),
                line_number: number,
                column_range,
                line: text.clone(),
                context_before,
                context_after: Vec::new(),
            });
        }
        if self.context > 0 {
            if self.recent.len() == self.context {
                self.recent.pop_front();
            }
            self.recent.push_back((number, text));
        }
        Ok(ControlFlow::Continue(()))
    }
}

/// `bytes` without its line terminator.
fn strip_terminator(mut bytes: Vec<u8>) -> Vec<u8> {
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
        if bytes.last() == Some(&b'\r') {
            bytes.pop();
        }
    }
    bytes
}

/// The range of `m` (byte offsets into the raw line) within the line as
/// shown, where invalid UTF-8 is replaced by U+FFFD (3 bytes each).
fn column_range(raw: &[u8], m: &Submatch) -> ColumnRange {
    let start = m.start.min(raw.len());
    let end = m.end.clamp(start, raw.len());
    if std::str::from_utf8(raw).is_ok() {
        return ColumnRange { start, end };
    }
    let shown_len = |bytes: &[u8]| String::from_utf8_lossy(bytes).len();
    let shown_start = shown_len(&raw[..start]);
    ColumnRange {
        start: shown_start,
        end: shown_start + shown_len(&raw[start..end]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{Workspace, fake_tool, p};

    fn ws() -> Workspace {
        let ws = Workspace::new();
        ws.write(
            "src/a.rs",
            "fn one() {}\nlet x = -foo;\nfn two() {}\nfn three() {}\nfoo\n",
        )
        .write("src/b.rs", "nothing\n")
        .write("notes.md", "FOO bar\n");
        ws
    }

    fn request(pattern: &str) -> GrepRequest {
        GrepRequest {
            context: 0,
            ..GrepRequest::new(pattern)
        }
    }

    fn lines(result: &GrepResult) -> Vec<(&str, u64, &str)> {
        result
            .hits
            .iter()
            .map(|h| (h.path.as_str(), h.line_number, h.line.as_str()))
            .collect()
    }

    #[tokio::test]
    async fn pattern_starting_with_a_dash_is_not_a_flag() {
        let ws = ws();
        let result = grep(&ws.search(), request("-foo")).await.unwrap();
        assert_eq!(lines(&result), [("src/a.rs", 2, "let x = -foo;")]);
        assert_eq!(
            result.hits[0].column_range,
            ColumnRange { start: 8, end: 12 }
        );
        assert_eq!((result.total, result.truncated), (1, false));
    }

    #[tokio::test]
    async fn collects_context_before_and_after() {
        let ws = ws();
        let request = GrepRequest {
            context: 1,
            ..GrepRequest::new("fn t")
        };
        let result = grep(&ws.search(), request).await.unwrap();
        assert_eq!(result.hits.len(), 2);
        let (two, three) = (&result.hits[0], &result.hits[1]);
        assert_eq!(two.line_number, 3);
        assert_eq!(two.context_before, ["let x = -foo;"]);
        assert_eq!(two.context_after, ["fn three() {}"]);
        assert_eq!(three.context_before, ["fn two() {}"]);
        assert_eq!(three.context_after, ["foo"]);
    }

    #[tokio::test]
    async fn no_matches_is_an_empty_result() {
        let result = grep(&ws().search(), request("zzz")).await.unwrap();
        assert_eq!(result, GrepResult::default());
    }

    #[tokio::test]
    async fn invalid_regex_is_an_invalid_pattern() {
        let err = grep(&ws().search(), request("(")).await.unwrap_err();
        let SearchError::InvalidPattern(message) = err else {
            panic!("{err:?}");
        };
        assert!(message.contains("regex parse error"), "{message}");
        // As a fixed string it is fine.
        let fixed = GrepRequest {
            fixed: true,
            ..request("(")
        };
        assert!(grep(&ws().search(), fixed).await.is_ok());
    }

    #[tokio::test]
    async fn honors_flags_glob_and_path() {
        let ws = ws();
        let search = ws.search();
        let insensitive = GrepRequest {
            ignore_case: true,
            ..request("foo")
        };
        let all = grep(&search, insensitive.clone()).await.unwrap();
        let mut found = lines(&all);
        found.sort_unstable();
        assert_eq!(
            found,
            [
                ("notes.md", 1, "FOO bar"),
                ("src/a.rs", 2, "let x = -foo;"),
                ("src/a.rs", 5, "foo")
            ]
        );
        let globbed = GrepRequest {
            glob: Some("*.md".into()),
            ..insensitive.clone()
        };
        let found = grep(&search, globbed).await.unwrap();
        assert_eq!(lines(&found), [("notes.md", 1, "FOO bar")]);

        let in_file = GrepRequest {
            path: Some(p("src/a.rs")),
            ..request("fn")
        };
        assert_eq!(grep(&search, in_file).await.unwrap().hits.len(), 3);
        let in_dir = GrepRequest {
            path: Some(p("src")),
            ..insensitive
        };
        assert_eq!(grep(&search, in_dir).await.unwrap().hits.len(), 2);
    }

    #[tokio::test]
    async fn stops_at_the_limit() {
        let ws = Workspace::new();
        ws.write("big.txt", "hit\n".repeat(10_000));
        let limited = GrepRequest {
            limit: 3,
            ..request("hit")
        };
        let result = grep(&ws.search(), limited).await.unwrap();
        assert_eq!(result.hits.len(), 3);
        assert!(result.truncated);
        assert_eq!(result.total, 4);
    }

    #[tokio::test]
    async fn never_returns_sensitive_contents() {
        let ws = ws();
        ws.write(".env", "foo=secret\n")
            .write("keys/server.pem", "foo\n");
        let search = ws.search();
        let result = grep(&search, request("foo")).await.unwrap();
        assert!(
            result.hits.iter().all(|h| h.path.as_str() == "src/a.rs"),
            "{result:?}"
        );
        // Not even when asked for explicitly (rg would search it).
        let explicit = GrepRequest {
            path: Some(p(".env")),
            ..request("foo")
        };
        assert_eq!(
            grep(&search, explicit).await.unwrap(),
            GrepResult::default()
        );
    }

    #[tokio::test]
    async fn rejects_paths_escaping_through_symlinks() {
        let ws = ws();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("x.txt"), "foo\n").unwrap();
        std::os::unix::fs::symlink(outside.path(), ws.root.join("link")).unwrap();
        let request = GrepRequest {
            path: Some(p("link")),
            ..request("foo")
        };
        let err = grep(&ws.search(), request).await.unwrap_err();
        assert!(matches!(err, SearchError::Path(_)), "{err:?}");
    }

    #[tokio::test]
    async fn handles_lines_that_are_not_utf8() {
        let ws = Workspace::new();
        ws.write("latin1.txt", b"caf\xe9 foo\n");
        let result = grep(&ws.search(), request("foo")).await.unwrap();
        assert_eq!(result.hits.len(), 1);
        assert_eq!(result.hits[0].line, "caf\u{fffd} foo");
        let hit = &result.hits[0];
        // The match is at bytes 5..8 of the file but 7..10 of the shown line.
        assert_eq!(hit.column_range, ColumnRange { start: 7, end: 10 });
        assert_eq!(&hit.line[7..10], "foo");
    }

    #[tokio::test]
    async fn missing_rg_is_reported() {
        let ws = ws();
        let search = ws.search().with_rg_program("/nonexistent/rg");
        let err = grep(&search, request("foo")).await.unwrap_err();
        assert!(
            matches!(&err, SearchError::ToolMissing { tool, .. } if tool == "/nonexistent/rg"),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn garbage_output_is_a_parse_error() {
        let bin = tempfile::tempdir().unwrap();
        let rg = fake_tool(bin.path(), "rg", "echo not-json");
        let err = grep(&ws().search().with_rg_program(rg), request("foo"))
            .await
            .unwrap_err();
        assert!(matches!(err, SearchError::Parse(_)), "{err:?}");
    }

    #[test]
    fn parses_rg_events() {
        let event = Event::parse(
            r#"{"type":"match","data":{"path":{"text":"a"},"lines":{"bytes":"eCD/Cg=="},
            "line_number":3,"absolute_offset":0,"submatches":[{"match":{"text":"x"},"start":0,"end":1}]}}"#,
        )
        .unwrap();
        let Event::Match(line) = event else {
            panic!("{event:?}");
        };
        assert_eq!(line.lines.into_bytes().unwrap(), b"x \xff\n");
        let summary = Event::parse(r#"{"data":{"elapsed_total":{}},"type":"summary"}"#).unwrap();
        assert!(matches!(summary, Event::Other));
    }
}
