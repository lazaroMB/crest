//! [`StdinApproval`]: asks `[y]es / [a]lways / [n]o` on stdin.

use std::io::{BufRead, IsTerminal};
use std::sync::{Arc, Mutex, PoisonError};

use application::ports::approval::{
    ApprovalError, ApprovalPort, ApprovalPreview, ApprovalRequest, ApprovalResponse,
};
use async_trait::async_trait;
use domain::permission::{PermissionRule, RuleDecision, RuleMatcher};

use super::terminal::{Style, Terminal};

/// Input the answers are read from.
pub type Input = Arc<Mutex<Box<dyn BufRead + Send>>>;

/// Shows the diff or command of each request and reads the answer from
/// `input`, one line per question. With `approve_all` (`--yes`) every
/// request is approved without reading anything.
#[derive(Clone)]
pub struct StdinApproval {
    terminal: Terminal,
    input: Input,
    approve_all: bool,
    echo: bool,
}

impl std::fmt::Debug for StdinApproval {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StdinApproval")
            .field("approve_all", &self.approve_all)
            .finish_non_exhaustive()
    }
}

/// One parsed answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Answer {
    Yes,
    Always,
    No,
}

fn parse_answer(line: &str) -> Option<Answer> {
    match line.trim().to_lowercase().as_str() {
        "y" | "yes" | "s" | "si" | "sí" => Some(Answer::Yes),
        "a" | "always" | "siempre" => Some(Answer::Always),
        "" | "n" | "no" => Some(Answer::No),
        _ => None,
    }
}

impl StdinApproval {
    /// Asks on `terminal`, reading from `input`.
    #[must_use]
    pub fn new(terminal: Terminal, input: Box<dyn BufRead + Send>, approve_all: bool) -> Self {
        Self {
            terminal,
            input: Arc::new(Mutex::new(input)),
            approve_all,
            echo: true,
        }
    }

    /// Asks on stdout, reading from stdin.
    #[must_use]
    pub fn stdin(terminal: Terminal, approve_all: bool) -> Self {
        let echo = !std::io::stdin().is_terminal();
        Self {
            echo,
            ..Self::new(
                terminal,
                Box::new(std::io::BufReader::new(std::io::stdin())),
                approve_all,
            )
        }
    }

    fn show(&self, req: &ApprovalRequest) {
        let mut w = self.terminal.lock();
        match &req.preview {
            ApprovalPreview::Diff(diff) => w.diff(&diff.path, &diff.unified),
            ApprovalPreview::Command { command, args, cwd } => {
                let mut line = format!("  $ {command}");
                for arg in args {
                    line.push(' ');
                    line.push_str(&shell_quote(arg));
                }
                w.line(Some(Style::Bold), &line);
                w.line(Some(Style::Dim), &format!("    en {cwd}"));
            }
            ApprovalPreview::Text { text } => w.line(None, &format!("  {text}")),
        }
    }

    fn prompt(&self, req: &ApprovalRequest) {
        let mut w = self.terminal.lock();
        w.break_line();
        w.styled(
            Style::Bold,
            &format!(
                "¿Permitir {} {}? [y]es / [a]lways / [n]o: ",
                req.tool, req.subject
            ),
        );
    }
}

/// Quotes `arg` for display when it has spaces or shell characters.
fn shell_quote(arg: &str) -> String {
    let plain = !arg.is_empty()
        && arg
            .chars()
            .all(|c| c.is_alphanumeric() || "-_./=:,+@%".contains(c));
    if plain {
        arg.to_owned()
    } else {
        format!("'{}'", arg.replace('\'', r"'\''"))
    }
}

/// The "allow always" rule for `req`: the same tool on the same exact path
/// or command, never anything broader.
fn always_rule(req: &ApprovalRequest) -> PermissionRule {
    PermissionRule::new(
        req.tool.clone(),
        RuleMatcher::Exact(req.subject.clone()),
        RuleDecision::AllowAlways,
    )
}

#[async_trait]
impl ApprovalPort for StdinApproval {
    async fn request(&self, req: ApprovalRequest) -> Result<ApprovalResponse, ApprovalError> {
        self.show(&req);
        if self.approve_all {
            self.terminal.lock().line(
                Some(Style::Dim),
                &format!("  aprobado por --yes: {} {}", req.tool, req.subject),
            );
            return Ok(ApprovalResponse::Approve);
        }
        loop {
            self.prompt(&req);
            let input = Arc::clone(&self.input);
            let line = tokio::task::spawn_blocking(move || {
                let mut line = String::new();
                let mut input = input.lock().unwrap_or_else(PoisonError::into_inner);
                input.read_line(&mut line).map(|read| (read, line))
            })
            .await
            .map_err(|err| ApprovalError::Unavailable(err.to_string()))?
            .map_err(|err| ApprovalError::Unavailable(format!("cannot read stdin: {err}")))?;
            let (read, line) = line;
            // A keyboard already echoed the answer and its newline; piped
            // answers are echoed so the transcript shows what was decided.
            let mut w = self.terminal.lock();
            if self.echo {
                w.raw(line.trim_end());
                w.raw("\n");
            } else {
                w.assume_line_start();
            }
            drop(w);
            if read == 0 {
                return Ok(ApprovalResponse::Deny {
                    reason: Some("no answer: stdin is closed".into()),
                });
            }
            match parse_answer(&line) {
                Some(Answer::Yes) => return Ok(ApprovalResponse::Approve),
                Some(Answer::Always) => {
                    return Ok(ApprovalResponse::ApproveAlways {
                        rule: always_rule(&req),
                    });
                }
                Some(Answer::No) => return Ok(ApprovalResponse::Deny { reason: None }),
                None => self
                    .terminal
                    .lock()
                    .line(Some(Style::Yellow), "responde y, a o n"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use application::ports::approval::ApprovalRequestId;
    use domain::block::DiffSpec;
    use domain::conversation::ToolUseId;
    use domain::permission::ToolEffect;

    use super::*;
    use crate::headless::terminal::tests::terminal;

    fn command_request() -> ApprovalRequest {
        ApprovalRequest {
            id: ApprovalRequestId(1),
            tool_use_id: ToolUseId::new("toolu_1"),
            tool: "run_command".into(),
            effect: ToolEffect::Execute,
            subject: "cargo test".into(),
            preview: ApprovalPreview::Command {
                command: "cargo".into(),
                args: vec!["test".into(), "a b".into()],
                cwd: "/ws".into(),
            },
        }
    }

    fn approval(
        answers: &str,
        approve_all: bool,
    ) -> (StdinApproval, crate::headless::terminal::tests::Buffer) {
        let (term, buf) = terminal(false);
        let input = Box::new(std::io::Cursor::new(answers.to_owned().into_bytes()));
        (StdinApproval::new(term, input, approve_all), buf)
    }

    #[tokio::test]
    async fn shows_the_command_and_reads_the_answers() {
        let (approval, buf) = approval("what\ny\na\nn\n", false);
        let response = approval.request(command_request()).await.unwrap();
        assert_eq!(response, ApprovalResponse::Approve);
        let text = buf.text();
        assert!(
            text.starts_with("  $ cargo test 'a b'\n    en /ws\n"),
            "{text}"
        );
        assert!(text.contains("responde y, a o n"), "{text}");
        assert_eq!(
            approval.request(command_request()).await.unwrap(),
            ApprovalResponse::ApproveAlways {
                rule: PermissionRule::new(
                    "run_command",
                    RuleMatcher::Exact("cargo test".into()),
                    RuleDecision::AllowAlways
                )
            }
        );
        assert_eq!(
            approval.request(command_request()).await.unwrap(),
            ApprovalResponse::Deny { reason: None }
        );
        assert!(matches!(
            approval.request(command_request()).await.unwrap(),
            ApprovalResponse::Deny { reason: Some(_) }
        ));
    }

    #[tokio::test]
    async fn approve_all_shows_the_diff_and_does_not_read() {
        let (approval, buf) = approval("", true);
        let mut req = command_request();
        req.preview = ApprovalPreview::Diff(DiffSpec {
            path: "a.rs".into(),
            old_text: None,
            new_text: "x\n".into(),
            unified: "+x\n".into(),
            approval: None,
        });
        assert_eq!(
            approval.request(req).await.unwrap(),
            ApprovalResponse::Approve
        );
        assert!(buf.text().contains("  diff a.rs\n  +x\n"), "{}", buf.text());
    }

    #[test]
    fn answers_are_parsed_leniently() {
        assert_eq!(parse_answer(" Y \n"), Some(Answer::Yes));
        assert_eq!(parse_answer("always"), Some(Answer::Always));
        assert_eq!(parse_answer("\n"), Some(Answer::No));
        assert_eq!(parse_answer("maybe"), None);
    }
}
