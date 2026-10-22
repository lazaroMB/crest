//! [`CommandRewritePort`] over [rtk](https://github.com/rtk-ai/rtk), the
//! CLI proxy that compacts the output of common commands (`git status`,
//! `cargo test`, `ls`…) before the model reads it.
//!
//! For each command crest asks `rtk rewrite "<command line>"`, the same
//! contract rtk's own agent hooks use: exit 0 or 3 print the rewritten
//! line, 1 means there is no compact equivalent and 2 that an rtk deny
//! rule matched. crest's own approval already ran on the original command,
//! so rtk's allow/ask distinction is ignored and a deny keeps the command
//! as it was. The rewritten line is split into a program and arguments
//! (crest runs no shell); a line with shell syntax, or one that does not
//! start with `rtk`, is not used.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use application::ports::shell::{CommandRewritePort, RunCommandRequest};
use async_trait::async_trait;
use domain::workspace::WorkspaceRoot;
use tokio::process::Command;

use crate::SCRUBBED_ENV_VARS;

/// Longest wait for `rtk rewrite`; past it the command runs as is.
const REWRITE_TIMEOUT: Duration = Duration::from_secs(2);

/// Variable that relaxes rtk's approval gate; never passed on (rtk's own
/// hook scrubs it too).
const REWRITE_HOST_VAR: &str = "RTK_REWRITE_HOST";

/// Rewrites commands to their rtk equivalents.
#[derive(Debug, Clone)]
pub struct RtkRewriter {
    program: PathBuf,
    root: WorkspaceRoot,
}

impl RtkRewriter {
    /// The rewriter over the `rtk` binary at `program` (a path, or a name
    /// looked up on `PATH`), running in `root`; `None` when it is not
    /// found.
    #[must_use]
    pub fn detect(program: &str, root: WorkspaceRoot) -> Option<Self> {
        find_program(program).map(|program| Self { program, root })
    }

    /// The binary in use.
    #[must_use]
    pub fn program(&self) -> &Path {
        &self.program
    }

    fn environment() -> Vec<(OsString, OsString)> {
        std::env::vars_os()
            .filter(|(key, _)| {
                !platform::process::same_env_name(key, REWRITE_HOST_VAR)
                    && !SCRUBBED_ENV_VARS
                        .iter()
                        .any(|scrubbed| platform::process::same_env_name(key, scrubbed))
            })
            .collect()
    }
}

#[async_trait]
impl CommandRewritePort for RtkRewriter {
    fn name(&self) -> &str {
        "rtk"
    }

    async fn rewrite(&self, request: &RunCommandRequest) -> Option<RunCommandRequest> {
        // Already an rtk command, or crest's own binary path: leave it.
        if platform::process::program_name(&request.command) == "rtk" {
            return None;
        }
        let line = request.display();
        let mut command = Command::new(&self.program);
        platform::process::background(command.as_std_mut());
        let child = command
            .arg("rewrite")
            .arg(&line)
            .current_dir(self.root.as_path())
            .env_clear()
            .envs(Self::environment())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .output();
        let output = match tokio::time::timeout(REWRITE_TIMEOUT, child).await {
            Ok(Ok(output)) => output,
            Ok(Err(err)) => {
                tracing::debug!(%err, "rtk rewrite could not run");
                return None;
            }
            Err(_) => {
                tracing::debug!("rtk rewrite timed out");
                return None;
            }
        };
        if !matches!(output.status.code(), Some(0 | 3)) {
            return None;
        }
        let rewritten = String::from_utf8(output.stdout).ok()?;
        let rewritten = rewritten.trim();
        if rewritten.is_empty() || rewritten == line {
            return None;
        }
        let mut words = split_words(rewritten)?.into_iter();
        if words.next().as_deref() != Some("rtk") {
            return None;
        }
        Some(RunCommandRequest {
            command: self.program.to_string_lossy().into_owned(),
            args: words.collect(),
            timeout_s: request.timeout_s,
            raw: false,
        })
    }
}

/// `program` itself when it is a path to a file, else the first match on
/// `PATH` (with the `PATHEXT` extensions on Windows: `rtk.exe`).
fn find_program(program: &str) -> Option<PathBuf> {
    let path = Path::new(program);
    if path.components().count() > 1 {
        return path.is_file().then(|| path.to_path_buf());
    }
    platform::process::find_program(program, None)
}

/// Splits a command line into words with POSIX quoting (`'…'`, `"…"`,
/// backslash escapes outside quotes, except on Windows where an unquoted
/// backslash is a path separator); `None` when it uses anything a shell would
/// interpret (pipes, redirections, `&&`, variables, substitutions, globs)
/// or a quote is left open.
fn split_words(line: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            '\'' => {
                in_word = true;
                loop {
                    match chars.next()? {
                        '\'' => break,
                        c => word.push(c),
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match chars.next()? {
                        '"' => break,
                        '\\' => match chars.next()? {
                            c @ ('"' | '\\') => word.push(c),
                            c => {
                                word.push('\\');
                                word.push(c);
                            }
                        },
                        '$' | '`' => return None,
                        c => word.push(c),
                    }
                }
            }
            '\\' if cfg!(windows) => {
                in_word = true;
                word.push('\\');
            }
            '\\' => {
                in_word = true;
                word.push(chars.next()?);
            }
            '|' | '&' | ';' | '<' | '>' | '$' | '`' | '(' | ')' | '*' | '?' | '\n' => {
                return None;
            }
            c => {
                in_word = true;
                word.push(c);
            }
        }
    }
    if in_word {
        words.push(word);
    }
    Some(words)
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    #[test]
    fn splits_quoted_words_and_refuses_shell_syntax() {
        assert_eq!(
            split_words(r#"rtk git log --format='%h %s' "a \"b\"" c\ d"#),
            Some(
                ["rtk", "git", "log", "--format=%h %s", r#"a "b""#, "c d"]
                    .map(String::from)
                    .to_vec()
            )
        );
        assert_eq!(
            split_words("rtk ''"),
            Some(vec!["rtk".into(), String::new()])
        );
        for shell in [
            "rtk ls | head",
            "rtk a && b",
            "rtk echo $HOME",
            "rtk cat > f",
            "rtk ls *.rs",
            "rtk 'open",
            "rtk \"$(x)\"",
        ] {
            assert_eq!(split_words(shell), None, "{shell}");
        }
    }

    /// A fake `rtk` that answers `rewrite` with `stdout` and `code`.
    #[allow(clippy::unwrap_used)]
    fn fake_rtk(dir: &Path, stdout: &str, code: i32) -> RtkRewriter {
        let path = dir.join("rtk");
        let script = format!(
            "#!/bin/sh\n[ \"$1\" = rewrite ] || exit 9\nprintf '%s\\n' '{stdout}'\nexit {code}\n"
        );
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        let root = WorkspaceRoot::new(dir.to_str().unwrap()).unwrap();
        RtkRewriter::detect(path.to_str().unwrap(), root).unwrap()
    }

    #[tokio::test]
    async fn follows_the_exit_code_contract() {
        let dir = tempfile::tempdir().unwrap();
        let request = RunCommandRequest::new("git", ["status"]);
        for (code, rewrites) in [(0, true), (3, true), (1, false), (2, false), (7, false)] {
            let rtk = fake_rtk(dir.path(), "rtk git status", code);
            let rewritten = rtk.rewrite(&request).await;
            assert_eq!(rewritten.is_some(), rewrites, "exit {code}");
            if let Some(rewritten) = rewritten {
                assert_eq!(rewritten.command, rtk.program().to_string_lossy());
                assert_eq!(rewritten.args, ["git", "status"]);
                assert_eq!(rewritten.timeout_s, request.timeout_s);
            }
        }
    }

    #[tokio::test]
    async fn ignores_lines_it_cannot_run_without_a_shell() {
        let dir = tempfile::tempdir().unwrap();
        let request = RunCommandRequest::new("cat", ["a.txt"]);
        for line in ["rtk read a.txt | head", "other read a.txt", "cat a.txt"] {
            let rtk = fake_rtk(dir.path(), line, 0);
            assert_eq!(rtk.rewrite(&request).await, None, "{line}");
        }
        let rtk = fake_rtk(dir.path(), "rtk read a.txt", 0);
        let rewritten = rtk.rewrite(&request).await.unwrap();
        assert_eq!(rewritten.args, ["read", "a.txt"]);
        assert_eq!(
            rtk.rewrite(&RunCommandRequest::new("rtk", ["gain"])).await,
            None,
            "rtk itself is left alone"
        );
    }

    #[test]
    fn detect_needs_the_binary() {
        let root = WorkspaceRoot::new("/").unwrap();
        assert!(RtkRewriter::detect("/nonexistent/rtk", root.clone()).is_none());
        assert!(RtkRewriter::detect("crest-no-such-rtk-binary", root).is_none());
    }
}
