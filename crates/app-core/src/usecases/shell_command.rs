//! `/!command`: a shell command the user runs in the workspace.
//!
//! The line runs with the port's [`ShellPort::line_shell`] (`sh -c`, or
//! PowerShell on Windows, so pipes and globs work) without asking for
//! approval: the user typed it. The result
//! is a [`BlockKind::Shell`] for the timeline and a context note that the
//! front end sends to the model with the next prompt, as Claude Code does
//! with `!` commands.

use domain::block::BlockKind;

use crate::ports::shell::ShellPort;

/// Most bytes of each stream kept in the block (the timeline draws it).
const MAX_SHOWN: usize = 200_000;
/// Most bytes of each stream sent to the model.
const MAX_CONTEXT: usize = 20_000;

/// What running a `/!` command produced.
#[derive(Debug, Clone, PartialEq)]
pub struct ShellRun {
    /// The block for the timeline ([`BlockKind::Shell`]).
    pub block: BlockKind,
    /// The note for the model: the command and its output, tagged.
    pub context: String,
}

/// Runs `line` with the shell's line shell in the workspace.
pub async fn run_user_command(shell: &dyn ShellPort, line: &str) -> ShellRun {
    let command = line.trim().to_owned();
    let request = shell.line_shell().request(&command);
    let (exit_code, stdout, stderr, timed_out, truncated) = match shell.run(request).await {
        Ok(output) => (
            output.status,
            output.stdout,
            output.stderr,
            output.timed_out,
            output.truncated,
        ),
        Err(err) => (None, String::new(), err.to_string(), false, false),
    };
    let context = context_note(&command, exit_code, &stdout, &stderr, timed_out);
    let (stdout, cut_out) = cut(&stdout, MAX_SHOWN);
    let (stderr, cut_err) = cut(&stderr, MAX_SHOWN);
    ShellRun {
        block: BlockKind::Shell {
            command,
            exit_code,
            stdout,
            stderr,
            timed_out,
            truncated: truncated || cut_out || cut_err,
        },
        context,
    }
}

/// The note the model gets: Claude Code's `bash-input` / `bash-stdout` /
/// `bash-stderr` tags, each stream cut to [`MAX_CONTEXT`].
fn context_note(
    command: &str,
    exit_code: Option<i32>,
    stdout: &str,
    stderr: &str,
    timed_out: bool,
) -> String {
    let status = match (exit_code, timed_out) {
        (_, true) => "timed out".to_owned(),
        (Some(code), _) => format!("exit code {code}"),
        (None, _) => "killed".to_owned(),
    };
    let (stdout, _) = cut(stdout.trim_end(), MAX_CONTEXT);
    let (stderr, _) = cut(stderr.trim_end(), MAX_CONTEXT);
    format!(
        "The user ran a command in the workspace ({status}):\n\
         <bash-input>{command}</bash-input>\n\
         <bash-stdout>{stdout}</bash-stdout>\n\
         <bash-stderr>{stderr}</bash-stderr>"
    )
}

/// `text` cut to at most `max` bytes on a character boundary, and whether
/// it was cut.
fn cut(text: &str, max: usize) -> (String, bool) {
    if text.len() <= max {
        return (text.to_owned(), false);
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (format!("{}\n…", &text[..end]), true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::shell::CommandOutput;
    use crate::testing::fs::ScriptedShell;

    #[tokio::test]
    async fn runs_the_line_with_sh_and_tags_the_context() {
        let shell = ScriptedShell::new().on(
            "sh",
            ["-c", "ls | head -1"],
            CommandOutput {
                status: Some(0),
                stdout: "Cargo.toml\n".into(),
                ..CommandOutput::default()
            },
        );
        let run = run_user_command(&shell, " ls | head -1 ").await;
        assert_eq!(
            run.block,
            BlockKind::Shell {
                command: "ls | head -1".into(),
                exit_code: Some(0),
                stdout: "Cargo.toml\n".into(),
                stderr: String::new(),
                timed_out: false,
                truncated: false,
            }
        );
        assert!(
            run.context
                .contains("<bash-input>ls | head -1</bash-input>")
        );
        assert!(
            run.context
                .contains("<bash-stdout>Cargo.toml</bash-stdout>")
        );
        assert!(run.context.contains("exit code 0"));
    }

    #[test]
    fn long_output_is_cut_on_a_char_boundary() {
        let (text, cut_it) = cut("ñañaña", 3);
        assert!(cut_it);
        assert_eq!(text, "ña\n…");
        assert_eq!(cut("abc", 3), ("abc".to_owned(), false));
    }
}
