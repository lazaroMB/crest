//! Runs the checks of a plan (`acceptance`, `hypothesis_tests`,
//! `constraint_tests`) through the [`ShellPort`].
//!
//! A check is a command line. Plain lines are split into program and
//! arguments (quotes respected); lines that need a shell (pipes, `&&`,
//! redirections, variables, globs) run with the port's
//! [`ShellPort::line_shell`]: `sh -c '<line>'`, or PowerShell on Windows.

use crate::ports::shell::{CommandOutput, RunCommandRequest, ShellPort, ShellSpec};

/// Seconds a check may run: test suites and benchmarks take longer than
/// the agent's commands.
pub const CHECK_TIMEOUT_S: u64 = 30 * 60;

/// Lines of output kept for the journal and the agent.
const OUTPUT_TAIL_LINES: usize = 40;

/// Characters that only a shell understands.
const SHELL_CHARS: &[char] = &['|', '&', ';', '<', '>', '$', '`', '*', '?', '(', ')', '~'];

/// Splits `line` into words, honouring single and double quotes and
/// backslash escapes (on Windows a backslash is a path separator, kept as
/// is). `None` when a quote is left open.
fn split_words(line: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut quote: Option<char> = None;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (_, '\\') if cfg!(windows) => {
                word.push(c);
                in_word = true;
            }
            (Some(q), c) if c == q => quote = None,
            (Some('"'), '\\') => word.push(chars.next().unwrap_or('\\')),
            (Some(_), c) => word.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                in_word = true;
            }
            (None, '\\') => {
                word.push(chars.next().unwrap_or('\\'));
                in_word = true;
            }
            (None, c) if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            (None, c) => {
                word.push(c);
                in_word = true;
            }
        }
    }
    if quote.is_some() {
        return None;
    }
    if in_word {
        words.push(word);
    }
    Some(words)
}

/// Whether `line` has shell syntax outside quotes.
fn needs_shell(line: &str) -> bool {
    let mut quote: Option<char> = None;
    for c in line.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None if c == '\'' || c == '"' => quote = Some(c),
            None if SHELL_CHARS.contains(&c) => return true,
            None => {}
        }
    }
    false
}

/// The request that runs the command line `line`, through `shell` when it
/// needs one.
///
/// # Errors
///
/// A message when the line is empty or leaves a quote open.
pub fn command_request(line: &str, shell: &ShellSpec) -> Result<RunCommandRequest, String> {
    let line = line.trim();
    let mut request = if needs_shell(line) {
        shell.request(line)
    } else {
        let words = split_words(line).ok_or_else(|| format!("unclosed quote in `{line}`"))?;
        let Some((program, args)) = words.split_first() else {
            return Err("empty command".to_owned());
        };
        RunCommandRequest::new(program.clone(), args.iter().cloned())
    };
    request.timeout_s = CHECK_TIMEOUT_S;
    Ok(request)
}

/// The result of running one check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckRun {
    /// The command line as written in the plan.
    pub command: String,
    /// What it printed and how it ended; `None` when it could not run.
    pub output: Option<CommandOutput>,
    /// Why it could not run.
    pub error: Option<String>,
}

impl CheckRun {
    /// Whether the command ran and exited with 0.
    #[must_use]
    pub fn passed(&self) -> bool {
        self.output.as_ref().is_some_and(CommandOutput::success)
    }

    /// "passes", "fails (exit 1)", "did not finish (timeout)"…
    #[must_use]
    pub fn verdict(&self) -> String {
        match (&self.output, &self.error) {
            (Some(out), _) if out.success() => "passes".to_owned(),
            (Some(out), _) if out.timed_out => "did not finish (timeout)".to_owned(),
            (Some(out), _) => match out.status {
                Some(code) => format!("fails (exit {code})"),
                None => "fails (signal)".to_owned(),
            },
            (None, Some(err)) => format!("could not run: {err}"),
            (None, None) => "did not run".to_owned(),
        }
    }

    /// Stdout then stderr, without the trailing newline.
    #[must_use]
    pub fn text(&self) -> String {
        let Some(out) = &self.output else {
            return self.error.clone().unwrap_or_default();
        };
        let mut text = out.stdout.trim_end().to_owned();
        let stderr = out.stderr.trim_end();
        if !stderr.is_empty() {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(stderr);
        }
        text
    }

    /// The command, its verdict and the last lines of its output, in
    /// markdown, for the journal and the agent.
    #[must_use]
    pub fn describe(&self) -> String {
        let text = self.text();
        let lines: Vec<_> = text.lines().collect();
        let tail = lines[lines.len().saturating_sub(OUTPUT_TAIL_LINES)..].join("\n");
        if tail.is_empty() {
            format!("`{}` → {}", self.command, self.verdict())
        } else {
            format!("`{}` → {}\n```\n{tail}\n```", self.command, self.verdict())
        }
    }
}

/// Runs the command line `line`.
pub async fn run_check(shell: &dyn ShellPort, line: &str) -> CheckRun {
    let command = line.trim().to_owned();
    let request = match command_request(&command, &shell.line_shell()) {
        Ok(request) => request,
        Err(error) => {
            return CheckRun {
                command,
                output: None,
                error: Some(error),
            };
        }
    };
    match shell.run(request).await {
        Ok(output) => CheckRun {
            command,
            output: Some(output),
            error: None,
        },
        Err(err) => CheckRun {
            command,
            output: None,
            error: Some(err.to_string()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::shell::ShellError;
    use crate::testing::fs::ScriptedShell;

    fn words(line: &str) -> (String, Vec<String>) {
        let r = command_request(line, &ShellSpec::posix()).unwrap();
        assert_eq!(r.timeout_s, CHECK_TIMEOUT_S);
        (r.command, r.args)
    }

    #[test]
    fn splits_plain_lines_and_wraps_shell_syntax() {
        assert_eq!(
            words("cargo test -p app session_cache"),
            (
                "cargo".into(),
                vec![
                    "test".into(),
                    "-p".into(),
                    "app".into(),
                    "session_cache".into()
                ]
            )
        );
        assert_eq!(
            words(r#"grep -q "a b" 'c d'"#),
            ("grep".into(), vec!["-q".into(), "a b".into(), "c d".into()])
        );
        if cfg!(windows) {
            assert_eq!(words(r"type C:\a\b.txt").1, [r"C:\a\b.txt"]);
        } else {
            assert_eq!(words(r"echo e\ f").1, ["e f"]);
        }
        assert_eq!(
            words("make bench | tail -1"),
            (
                "sh".into(),
                vec!["-c".into(), "make bench | tail -1".into()]
            )
        );
        assert_eq!(words("echo 'a|b'").1, ["a|b"]);
        let shell = ShellSpec::posix();
        assert!(command_request("  ", &shell).is_err());
        assert!(command_request("echo 'open", &shell).is_err());
    }

    #[tokio::test]
    async fn runs_checks_and_describes_failures() {
        let shell = ScriptedShell::new()
            .on(
                "cargo",
                ["test"],
                CommandOutput {
                    status: Some(101),
                    stdout: "running 1 test\n".into(),
                    stderr: "test x ... FAILED\n".into(),
                    ..CommandOutput::default()
                },
            )
            .on_error(
                "missing",
                Vec::<String>::new(),
                ShellError::NotFound {
                    command: "missing".into(),
                },
            );
        let run = run_check(&shell, "cargo test").await;
        assert!(!run.passed());
        assert_eq!(run.verdict(), "fails (exit 101)");
        assert_eq!(
            run.describe(),
            "`cargo test` → fails (exit 101)\n```\nrunning 1 test\ntest x ... FAILED\n```"
        );
        let missing = run_check(&shell, "missing").await;
        assert!(missing.verdict().starts_with("could not run"));
    }
}
