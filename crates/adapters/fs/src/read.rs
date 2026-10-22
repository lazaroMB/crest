//! `read_file`: text files sliced by line range.
//!
//! A request without a range reads the whole file, which must not exceed
//! [`MAX_READ_BYTES`]; it is binary (and rejected) when its first
//! [`BINARY_SNIFF_BYTES`] contain a NUL byte or it is not valid UTF-8.
//!
//! A request with an explicit range may read files of any size, so it
//! streams the file line by line instead: only the requested lines are
//! decoded and kept, the rest are just counted for
//! [`FileContent::total_lines`]. The NUL sniff is the same, but UTF-8 is
//! only checked on the returned lines, since the others are never decoded.

use std::path::Path;

use application::ports::fs::{FileContent, FsError, MAX_READ_BYTES, ReadFileRequest};
use domain::workspace::WorkspacePath;
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::guard::WorkspaceGuard;

/// How many leading bytes are searched for a NUL to detect binaries.
pub(crate) const BINARY_SNIFF_BYTES: usize = 8 * 1024;

/// Reads the slice of a text file that `request` selects.
pub(crate) async fn read_file(
    guard: &WorkspaceGuard,
    request: &ReadFileRequest,
) -> Result<FileContent, FsError> {
    let (path, real) = guard.resolve_readable(request.path.as_str()).await?;
    let meta = tokio::fs::metadata(&real)
        .await
        .map_err(|e| io_error(&path, &real, &e))?;
    if meta.is_dir() {
        return Err(FsError::IsDirectory { path });
    }
    if meta.len() > MAX_READ_BYTES && !request.has_explicit_range() {
        return Err(FsError::TooLarge {
            size: meta.len(),
            limit: MAX_READ_BYTES,
        });
    }
    if request.has_explicit_range() {
        return read_range(path, &real, request).await;
    }
    let text = read_text(&path, &real).await?;
    Ok(FileContent::from_text(&text, request))
}

/// Reads the whole file `path` as bytes, up to `max`.
pub(crate) async fn read_bytes(
    guard: &WorkspaceGuard,
    path: &WorkspacePath,
    max: u64,
) -> Result<Vec<u8>, FsError> {
    let (path, real) = guard.resolve_readable(path.as_str()).await?;
    let meta = tokio::fs::metadata(&real)
        .await
        .map_err(|e| io_error(&path, &real, &e))?;
    if meta.is_dir() {
        return Err(FsError::IsDirectory { path });
    }
    if meta.len() > max {
        return Err(FsError::TooLarge {
            size: meta.len(),
            limit: max,
        });
    }
    tokio::fs::read(&real)
        .await
        .map_err(|e| io_error(&path, &real, &e))
}

/// Streams the lines of `real` that `request` selects, with the same
/// result as [`FileContent::from_text`] on the whole (text) file.
async fn read_range(
    path: WorkspacePath,
    real: &Path,
    request: &ReadFileRequest,
) -> Result<FileContent, FsError> {
    let io = |e: std::io::Error| io_error(&path, real, &e);
    let file = tokio::fs::File::open(real).await.map_err(io)?;
    let mut reader = BufReader::new(file);
    let start_line = request.start_line();
    let mut sniff = Sniffer::default();
    let mut lines = Vec::new();
    let mut total_lines = 0;
    let mut buf = Vec::new();
    while lines.len() < request.line_limit() {
        buf.clear();
        if reader.read_until(b'\n', &mut buf).await.map_err(io)? == 0 {
            break;
        }
        sniff.feed(&path, &buf)?;
        total_lines += 1;
        if total_lines >= start_line {
            lines.push(decode_line(&path, &mut buf)?);
        }
    }
    // Count the rest without decoding: one line per `\n`, plus a final
    // line without terminator.
    let mut last = None;
    loop {
        let chunk = reader.fill_buf().await.map_err(io)?;
        let Some(&byte) = chunk.last() else { break };
        sniff.feed(&path, chunk)?;
        total_lines += chunk.iter().filter(|&&b| b == b'\n').count();
        last = Some(byte);
        let len = chunk.len();
        reader.consume(len);
    }
    if last.is_some_and(|b| b != b'\n') {
        total_lines += 1;
    }
    let last_shown = start_line - 1 + lines.len();
    Ok(FileContent {
        path,
        truncated: last_shown < total_lines,
        lines,
        start_line,
        total_lines,
    })
}

/// A line read with its terminator, decoded without it (`\n` or `\r\n`,
/// as [`str::lines`] strips them).
fn decode_line(path: &WorkspacePath, buf: &mut Vec<u8>) -> Result<String, FsError> {
    if buf.last() == Some(&b'\n') {
        buf.pop();
        if buf.last() == Some(&b'\r') {
            buf.pop();
        }
    }
    String::from_utf8(std::mem::take(buf)).map_err(|_| FsError::Binary { path: path.clone() })
}

/// Looks for a NUL in the first [`BINARY_SNIFF_BYTES`] of a stream.
#[derive(Default)]
struct Sniffer {
    seen: usize,
}

impl Sniffer {
    fn feed(&mut self, path: &WorkspacePath, bytes: &[u8]) -> Result<(), FsError> {
        let window = BINARY_SNIFF_BYTES
            .saturating_sub(self.seen)
            .min(bytes.len());
        self.seen = self.seen.saturating_add(bytes.len());
        if has_nul(&bytes[..window]) {
            return Err(FsError::Binary { path: path.clone() });
        }
        Ok(())
    }
}

/// Reads the whole file at `real` as text, failing with
/// [`FsError::Binary`] when it does not look like text.
pub(crate) async fn read_text(path: &WorkspacePath, real: &Path) -> Result<String, FsError> {
    let bytes = tokio::fs::read(real)
        .await
        .map_err(|e| io_error(path, real, &e))?;
    decode_text(path, bytes)
}

/// `bytes` as text, or [`FsError::Binary`] when they contain a NUL in the
/// first [`BINARY_SNIFF_BYTES`] or are not valid UTF-8.
pub(crate) fn decode_text(path: &WorkspacePath, bytes: Vec<u8>) -> Result<String, FsError> {
    let binary = || FsError::Binary { path: path.clone() };
    if has_nul(&bytes[..bytes.len().min(BINARY_SNIFF_BYTES)]) {
        return Err(binary());
    }
    String::from_utf8(bytes).map_err(|_| binary())
}

fn has_nul(bytes: &[u8]) -> bool {
    bytes.contains(&0)
}

/// Maps an I/O error on `real`; a file that vanished after the guard
/// resolved it is reported as [`FsError::NotFound`].
pub(crate) fn io_error(path: &WorkspacePath, real: &Path, error: &std::io::Error) -> FsError {
    match error.kind() {
        std::io::ErrorKind::NotFound => FsError::NotFound { path: path.clone() },
        std::io::ErrorKind::IsADirectory => FsError::IsDirectory { path: path.clone() },
        _ => FsError::Io(format!("{}: {error}", real.display())),
    }
}

// The fixtures build symlinks and permission modes, which are Unix-only.
#[cfg(test)]
#[cfg(unix)]
mod tests {
    use std::os::unix::fs::symlink;

    use domain::workspace::PathError;

    use super::*;

    struct Fixture {
        _dir: tempfile::TempDir,
        ws: std::path::PathBuf,
        guard: WorkspaceGuard,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        std::fs::create_dir(&ws).unwrap();
        let guard = WorkspaceGuard::open(&ws).unwrap();
        Fixture {
            _dir: dir,
            ws,
            guard,
        }
    }

    fn req(path: &str, offset: Option<usize>, limit: Option<usize>) -> ReadFileRequest {
        ReadFileRequest {
            path: WorkspacePath::parse(path).unwrap(),
            offset,
            limit,
        }
    }

    fn p(path: &str) -> WorkspacePath {
        WorkspacePath::parse(path).unwrap()
    }

    #[tokio::test]
    async fn reads_whole_file_with_line_numbers() {
        let f = fixture();
        std::fs::write(f.ws.join("a.txt"), "one\r\ntwo\nthree").unwrap();
        let content = read_file(&f.guard, &req("a.txt", None, None))
            .await
            .unwrap();
        assert_eq!(content.lines, ["one", "two", "three"]);
        assert_eq!((content.start_line, content.total_lines), (1, 3));
        assert!(!content.truncated);
        assert_eq!(
            content.render_numbered(),
            "     1\tone\n     2\ttwo\n     3\tthree\n"
        );
    }

    #[tokio::test]
    async fn reads_requested_range() {
        let f = fixture();
        std::fs::write(f.ws.join("a.txt"), "1\n2\n3\n4\n5\n").unwrap();
        let content = read_file(&f.guard, &req("a.txt", Some(2), Some(2)))
            .await
            .unwrap();
        assert_eq!(content.lines, ["2", "3"]);
        assert_eq!((content.start_line, content.total_lines), (2, 5));
        assert!(content.truncated);
    }

    #[tokio::test]
    async fn rejects_binary_files() {
        let f = fixture();
        std::fs::write(f.ws.join("nul.bin"), b"text\0more").unwrap();
        std::fs::write(f.ws.join("latin1.txt"), b"caf\xe9\n").unwrap();
        for name in ["nul.bin", "latin1.txt"] {
            assert_eq!(
                read_file(&f.guard, &req(name, None, None)).await,
                Err(FsError::Binary { path: p(name) }),
                "{name}"
            );
        }
    }

    #[test]
    fn nul_is_only_sniffed_in_the_first_8_kib() {
        let path = p("a.txt");
        let mut late = vec![b'a'; BINARY_SNIFF_BYTES];
        late.push(0);
        assert!(decode_text(&path, late).is_ok());
        let mut early = vec![b'a'; BINARY_SNIFF_BYTES - 1];
        early.push(0);
        assert_eq!(decode_text(&path, early), Err(FsError::Binary { path }));
    }

    #[tokio::test]
    async fn rejects_files_over_the_limit_without_range() {
        let f = fixture();
        let size = MAX_READ_BYTES + 1;
        std::fs::write(f.ws.join("big.txt"), vec![b'a'; size as usize]).unwrap();
        assert_eq!(
            read_file(&f.guard, &req("big.txt", None, None)).await,
            Err(FsError::TooLarge {
                size,
                limit: MAX_READ_BYTES
            })
        );
    }

    #[tokio::test]
    async fn reads_large_files_with_explicit_range() {
        let f = fixture();
        let text: String = (1..=300_000).map(|n| format!("line {n}\n")).collect();
        assert!(text.len() as u64 > MAX_READ_BYTES);
        std::fs::write(f.ws.join("big.txt"), &text).unwrap();
        let content = read_file(&f.guard, &req("big.txt", Some(299_999), None))
            .await
            .unwrap();
        assert_eq!(content.lines, ["line 299999", "line 300000"]);
        assert_eq!(content.total_lines, 300_000);
        assert!(!content.truncated);
        let head = read_file(&f.guard, &req("big.txt", None, Some(1)))
            .await
            .unwrap();
        assert_eq!(head.lines, ["line 1"]);
        assert!(head.truncated);
    }

    #[tokio::test]
    async fn ranged_reads_match_whole_file_slicing() {
        let f = fixture();
        let texts = ["", "a", "a\n", "a\r\nb", "a\n\nb\n", "a\rb\r", "\n\n"];
        let ranges = [
            (None, Some(0)),
            (Some(0), None),
            (Some(1), Some(1)),
            (Some(2), Some(1)),
            (Some(2), None),
            (Some(5), Some(2)),
        ];
        for text in texts {
            std::fs::write(f.ws.join("t.txt"), text).unwrap();
            for (offset, limit) in ranges {
                let request = req("t.txt", offset, limit);
                assert_eq!(
                    read_file(&f.guard, &request).await.unwrap(),
                    FileContent::from_text(text, &request),
                    "{text:?} {offset:?} {limit:?}"
                );
            }
        }
    }

    #[tokio::test]
    async fn ranged_reads_reject_binaries() {
        let f = fixture();
        let mut late_nul = vec![b'a'; 100];
        late_nul.extend_from_slice(b"\nb\0\n");
        std::fs::write(f.ws.join("nul.bin"), late_nul).unwrap();
        std::fs::write(f.ws.join("latin1.txt"), b"ok\ncaf\xe9\n").unwrap();
        assert_eq!(
            read_file(&f.guard, &req("nul.bin", None, Some(1))).await,
            Err(FsError::Binary { path: p("nul.bin") }),
            "NUL after the returned lines is still sniffed"
        );
        assert_eq!(
            read_file(&f.guard, &req("latin1.txt", Some(2), None)).await,
            Err(FsError::Binary {
                path: p("latin1.txt")
            })
        );
        assert!(
            read_file(&f.guard, &req("latin1.txt", None, Some(1)))
                .await
                .is_ok(),
            "lines that are not returned are not decoded"
        );
    }

    #[tokio::test]
    async fn rejects_directories_and_missing_files() {
        let f = fixture();
        std::fs::create_dir(f.ws.join("src")).unwrap();
        assert_eq!(
            read_file(&f.guard, &req("src", None, None)).await,
            Err(FsError::IsDirectory { path: p("src") })
        );
        assert_eq!(
            read_file(&f.guard, &req("nope.txt", None, None)).await,
            Err(FsError::NotFound {
                path: p("nope.txt")
            })
        );
    }

    #[tokio::test]
    async fn denies_sensitive_and_escaping_paths() {
        let f = fixture();
        std::fs::write(f.ws.join(".env"), "KEY=1\n").unwrap();
        assert!(matches!(
            read_file(&f.guard, &req(".env", None, None)).await,
            Err(FsError::SensitivePath { .. })
        ));
        let outside = f.ws.parent().unwrap().join("secret.txt");
        std::fs::write(&outside, "secret\n").unwrap();
        symlink(&outside, f.ws.join("link.txt")).unwrap();
        assert!(matches!(
            read_file(&f.guard, &req("link.txt", None, None)).await,
            Err(FsError::Path(PathError::OutsideWorkspace { .. }))
        ));
    }

    #[tokio::test]
    async fn bytes_are_read_whole_within_the_limit_and_the_guard() {
        let f = fixture();
        std::fs::write(f.ws.join("cube.stl"), b"solid\0\x01\xff").unwrap();
        std::fs::write(f.ws.join(".env"), "KEY=1\n").unwrap();
        assert_eq!(
            read_bytes(&f.guard, &p("cube.stl"), 8).await.unwrap(),
            b"solid\0\x01\xff"
        );
        assert_eq!(
            read_bytes(&f.guard, &p("cube.stl"), 4).await,
            Err(FsError::TooLarge { size: 8, limit: 4 })
        );
        assert!(matches!(
            read_bytes(&f.guard, &p(".env"), 64).await,
            Err(FsError::SensitivePath { .. })
        ));
        std::fs::create_dir(f.ws.join("dir")).unwrap();
        assert!(matches!(
            read_bytes(&f.guard, &p("dir"), 64).await,
            Err(FsError::IsDirectory { .. })
        ));
    }
}
