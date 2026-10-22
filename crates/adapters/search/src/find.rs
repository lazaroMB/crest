//! `find_file`: the workspace listing filtered by `fzf --filter`.
//!
//! The listing is the one `list_files` uses (`rg --files`, or the built-in
//! walker without `rg`): sorted and already free of sensitive paths. It is
//! written to the stdin of `fzf --filter <query> --scheme=path`, which
//! prints the matches best first and exits; the first `limit` are kept.
//!
//! The plan pipes `rg` straight into `fzf`. Collecting the listing first
//! costs nothing extra (`fzf --filter` reads all of its input before
//! printing anything) and buys a deterministic order: `fzf` breaks score
//! ties by input position, and `rg` walks in parallel, so its raw output
//! order varies between runs. It also keeps sensitive paths from ever
//! reaching `fzf` and reuses the walker fallback when `rg` is missing.
//!
//! When `fzf` is missing or older than 0.36 (the first with
//! `--scheme=path`), matching falls back to a case-insensitive substring
//! search ranked like the application's in-memory fake: matches in the file
//! name first, then shorter paths, then by path.

use application::ports::search::{FileList, FindFileRequest, SearchError};
use domain::workspace::WorkspacePath;

use crate::RipgrepSearch;
use crate::detect::{self, Tool};
use crate::process::{self, ProcessError, ProcessSpec};

/// fzf exits with 1 when nothing matches, 2 on error and 130 when
/// interrupted.
const FZF_NO_MATCH: i32 = 1;
const FZF_ERROR: i32 = 2;

/// Fuzzy-finds files: what `FileSearchPort::find_file` returns.
pub async fn find_file(
    search: &RipgrepSearch,
    request: FindFileRequest,
) -> Result<FileList, SearchError> {
    let files = search.all_files(None).await?;
    if fzf_usable(search).await {
        match fzf_filter(search, &request.query, &files).await {
            Ok(Some(matches)) => return Ok(FileList::from_matches(matches, request.limit)),
            Ok(None) => {}
            Err(err) => return Err(err),
        }
    }
    Ok(FileList::from_matches(
        substring_matches(files, &request.query),
        request.limit,
    ))
}

/// Whether the configured `fzf` is installed and recent enough. Checked
/// once per adapter.
async fn fzf_usable(search: &RipgrepSearch) -> bool {
    search
        .fzf_status
        .get_or_init(|| async {
            let status = detect::detect(Tool::Fzf, &search.fzf).await;
            if let Some(problem) = status.problem(Tool::Fzf) {
                tracing::warn!("{problem}");
            }
            status
        })
        .await
        .is_ok()
}

/// Runs `fzf --filter`; `None` when `fzf` disappeared since it was
/// detected (the caller falls back to substring matching).
async fn fzf_filter(
    search: &RipgrepSearch,
    query: &str,
    files: &[WorkspacePath],
) -> Result<Option<Vec<WorkspacePath>>, SearchError> {
    let mut input = Vec::new();
    for file in files {
        input.extend_from_slice(file.as_str().as_bytes());
        input.push(b'\n');
    }
    let spec = ProcessSpec::new(&search.fzf, search.root.as_path())
        .arg("--filter")
        .arg(query)
        .arg("--scheme=path")
        // The user's defaults (`--exact`, `--tiebreak`...) would change
        // the ranking.
        .env("FZF_DEFAULT_OPTS", "")
        .env("FZF_DEFAULT_OPTS_FILE", "");
    let out = match process::run(&spec, Some(input), search.timeout).await {
        Ok(out) => out,
        Err(ProcessError::NotFound { .. }) => return Ok(None),
        Err(err) => return Err(err.into_search_error(Tool::Fzf.install_hint())),
    };
    if out.timed_out {
        return Err(SearchError::Timeout {
            after: search.timeout,
        });
    }
    match out.status.code() {
        Some(0 | FZF_NO_MATCH) => {}
        Some(FZF_ERROR) => {
            return Err(SearchError::Io(format!(
                "fzf failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        _ => return Err(SearchError::Cancelled),
    }
    let matches = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| WorkspacePath::parse(line).ok())
        .filter(|path| search.is_allowed(path))
        .collect();
    Ok(Some(matches))
}

/// The paths containing `query` (case-insensitive), ranked by whether the
/// file name contains it, then by length, then by path.
fn substring_matches(files: Vec<WorkspacePath>, query: &str) -> Vec<WorkspacePath> {
    let query = query.to_lowercase();
    let contains = |s: &str| s.to_lowercase().contains(&query);
    let mut matching: Vec<WorkspacePath> = files
        .into_iter()
        .filter(|path| contains(path.as_str()))
        .collect();
    matching.sort_by_cached_key(|path| {
        let in_name = path.file_name().is_some_and(contains);
        (!in_name, path.as_str().len(), path.clone())
    });
    matching
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use super::*;
    #[cfg(unix)]
    use crate::test_support::fake_tool;
    use crate::test_support::{Workspace, p, strs};

    fn ws() -> Workspace {
        let ws = Workspace::new();
        for file in [
            "src/main.rs",
            "src/domain/mainframe_helper.rs",
            "docs/maintenance.md",
            "m/a/i/n.txt",
            "crates/app/src/Main.java",
            "README.md",
            ".env.main",
        ] {
            ws.write(file, "");
        }
        ws
    }

    async fn fzf_installed() -> bool {
        let ok = detect::detect(Tool::Fzf, OsStr::new("fzf")).await.is_ok();
        if !ok {
            eprintln!("fzf >= 0.36 not installed; skipping");
        }
        ok
    }

    #[tokio::test]
    async fn fzf_orders_by_score() {
        if !fzf_installed().await {
            return;
        }
        let ws = ws();
        let list = find_file(&ws.search(), FindFileRequest::new("main"))
            .await
            .unwrap();
        assert_eq!(list.paths[0], p("src/main.rs"), "{:?}", list.paths);
        // fzf matches fuzzily: the letters may be spread over the path.
        assert!(list.paths.contains(&p("m/a/i/n.txt")), "{:?}", list.paths);
        assert_eq!(list.paths.last(), Some(&p("m/a/i/n.txt")));
        assert!(!list.paths.contains(&p(".env.main")));
        assert_eq!(list.total, list.paths.len());

        let limited = FindFileRequest {
            query: "main".into(),
            limit: 2,
        };
        let first = find_file(&ws.search(), limited).await.unwrap();
        assert_eq!(first.paths, list.paths[..2]);
        assert!(first.truncated);
        assert_eq!(first.total, list.total);
    }

    #[tokio::test]
    async fn fzf_order_is_deterministic() {
        if !fzf_installed().await {
            return;
        }
        let ws = Workspace::new();
        for i in 0..50 {
            ws.write(&format!("d{i:02}/same.rs"), "");
        }
        let search = ws.search();
        let first = find_file(&search, FindFileRequest::new("same"))
            .await
            .unwrap();
        for _ in 0..3 {
            let again = find_file(&search, FindFileRequest::new("same"))
                .await
                .unwrap();
            assert_eq!(again, first);
        }
    }

    const SUBSTRING_RANKING: [&str; 4] = [
        "src/main.rs",
        "docs/maintenance.md",
        "crates/app/src/Main.java",
        "src/domain/mainframe_helper.rs",
    ];

    #[tokio::test]
    async fn falls_back_to_substrings_without_fzf() {
        let ws = ws();
        let search = ws.search().with_fzf_program("/nonexistent/fzf");
        let list = find_file(&search, FindFileRequest::new("MAIN"))
            .await
            .unwrap();
        // Not fuzzy: `m/a/i/n.txt` does not match; name matches rank first.
        assert_eq!(strs(&list.paths), SUBSTRING_RANKING);
        assert_eq!((list.total, list.truncated), (4, false));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn falls_back_to_substrings_with_an_old_fzf() {
        let ws = ws();
        let bin = tempfile::tempdir().unwrap();
        let old = fake_tool(
            bin.path(),
            "fzf",
            "[ \"$1\" = --version ] && echo '0.29.0 (devel)' && exit 0; exit 2",
        );
        let search = ws.search().with_fzf_program(old);
        let list = find_file(&search, FindFileRequest::new("main"))
            .await
            .unwrap();
        assert_eq!(strs(&list.paths), SUBSTRING_RANKING);
    }

    #[tokio::test]
    async fn works_without_rg_either() {
        let ws = ws();
        let search = ws
            .search()
            .with_rg_program("/nonexistent/rg")
            .with_fzf_program("/nonexistent/fzf");
        let request = FindFileRequest {
            query: "main".into(),
            limit: 1,
        };
        let list = find_file(&search, request).await.unwrap();
        assert_eq!(strs(&list.paths), ["src/main.rs"]);
        assert!(list.truncated);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn fzf_errors_are_reported() {
        let ws = ws();
        let bin = tempfile::tempdir().unwrap();
        let broken = fake_tool(
            bin.path(),
            "fzf",
            "[ \"$1\" = --version ] && echo '0.74.5 (x)' && exit 0; echo boom >&2; exit 2",
        );
        let search = ws.search().with_fzf_program(broken);
        let err = find_file(&search, FindFileRequest::new("main"))
            .await
            .unwrap_err();
        assert_eq!(err, SearchError::Io("fzf failed: boom".into()));
    }

    #[test]
    fn substring_ranking_matches_the_fake() {
        let files = vec![p("a/main/x.rs"), p("b/main.rs"), p("main.rs"), p("zz.rs")];
        let ranked = substring_matches(files, "Main");
        assert_eq!(strs(&ranked), ["main.rs", "b/main.rs", "a/main/x.rs"]);
    }
}
