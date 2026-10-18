//! `read_file`: text files sliced by line range.
//!
//! A request without a range reads the whole file, which must not exceed
//! [`MAX_READ_BYTES`]; it is binary (and rejected) when its first
//! [`BINARY_SNIFF_BYTES`] contain a NUL byte or it is not valid UTF-8.

use std::path::Path;

use application::ports::fs::{FileContent, FsError, MAX_READ_BYTES, ReadFileRequest};
use domain::workspace::WorkspacePath;

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
    let text = read_text(&path, &real).await?;
    Ok(FileContent::from_text(&text, request))
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

#[cfg(test)]
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
}
