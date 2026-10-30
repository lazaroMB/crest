//! [`StdinApproval`]: asks `[y]es / [s]ession <rule> / [p]roject <rule> /
//! [e]dits / [n]o` on stdin. The rule is the default rung of
//! [`rule_ladder`] (e.g. `run_command(cargo test *)`); `[e]dits`, "allow
//! all edits this session", is only offered for writes.

use std::io::{BufRead, IsTerminal};
use std::sync::{Arc, Mutex, PoisonError};

use application::ports::approval::{
    ApprovalError, ApprovalPort, ApprovalPreview, ApprovalRequest, ApprovalResponse,
};
use async_trait::async_trait;
use domain::permission::{PermissionRule, RuleScope, ToolEffect, default_rung, rule_ladder};

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
    /// Allow the rule for the session (`a`/`always` too, as before).
    Session,
    /// Allow the rule in the project's permissions file.
    Project,
    /// Allow all edits this session (writes only).
    Edits,
    No,
}

fn parse_answer(line: &str) -> Option<Answer> {
    match line.trim().to_lowercase().as_str() {
        "y" | "yes" | "si" | "sí" => Some(Answer::Yes),
        "s" | "session" | "sesion" | "sesión" | "a" | "always" | "siempre" => {
            Some(Answer::Session)
        }
        "p" | "project" | "proyecto" => Some(Answer::Project),
        "e" | "edits" | "ediciones" => Some(Answer::Edits),
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

    /// Reads one line of the input (the answers to approvals and to the
    /// gates of a flow share it); `None` once it is closed. A keyboard
    /// already echoed the line; piped lines are echoed so the transcript
    /// shows what was decided.
    ///
    /// # Errors
    ///
    /// Why the input cannot be read.
    pub async fn read_line(&self) -> Result<Option<String>, String> {
        let input = Arc::clone(&self.input);
        let (read, line) = tokio::task::spawn_blocking(move || {
            let mut line = String::new();
            let mut input = input.lock().unwrap_or_else(PoisonError::into_inner);
            input.read_line(&mut line).map(|read| (read, line))
        })
        .await
        .map_err(|err| err.to_string())?
        .map_err(|err| format!("cannot read stdin: {err}"))?;
        let mut w = self.terminal.lock();
        if self.echo {
            w.raw(line.trim_end());
            w.raw("\n");
        } else {
            w.assume_line_start();
        }
        Ok((read > 0).then(|| line.trim_end_matches(['\r', '\n']).to_owned()))
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
                w.line(Some(Style::Dim), &format!("    in {cwd}"));
            }
            ApprovalPreview::Text { text } => w.line(None, &format!("  {text}")),
        }
    }

    fn prompt(&self, req: &ApprovalRequest, rule: &PermissionRule) {
        let edits = if offers_edits(req) {
            " / [e]dits (writes only)"
        } else {
            ""
        };
        let mut w = self.terminal.lock();
        w.break_line();
        w.styled(
            Style::Bold,
            &format!(
                "Allow {} {}? [y]es / [s]ession {rule} / [p]roject {rule}{edits} / [n]o: ",
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

/// The "allow always" rule offered for `req`, as a session rule: the
/// default rung of its ladder (`cargo test *`, `src/*`, or the exact
/// command for shells and interpreters). It cannot be widened here.
fn always_rule(req: &ApprovalRequest) -> PermissionRule {
    let mut ladder = rule_ladder(&req.action());
    let rung = default_rung(&ladder);
    ladder.swap_remove(rung).rule
}

/// Whether "allow all edits" is offered: only for writes.
fn offers_edits(req: &ApprovalRequest) -> bool {
    req.effect == ToolEffect::Write
}

#[async_trait]
impl ApprovalPort for StdinApproval {
    async fn request(&self, req: ApprovalRequest) -> Result<ApprovalResponse, ApprovalError> {
        self.show(&req);
        if self.approve_all {
            self.terminal.lock().line(
                Some(Style::Dim),
                &format!("  approved by --yes: {} {}", req.tool, req.subject),
            );
            return Ok(ApprovalResponse::Approve);
        }
        let rule = always_rule(&req);
        loop {
            self.prompt(&req, &rule);
            let Some(line) = self.read_line().await.map_err(ApprovalError::Unavailable)? else {
                return Ok(ApprovalResponse::Deny {
                    reason: Some("no answer: stdin is closed".into()),
                });
            };
            match parse_answer(&line) {
                Some(Answer::Yes) => return Ok(ApprovalResponse::Approve),
                Some(Answer::Session) => {
                    return Ok(ApprovalResponse::ApproveAlways { rule });
                }
                Some(Answer::Project) => {
                    return Ok(ApprovalResponse::ApproveAlways {
                        rule: rule.with_scope(RuleScope::Project),
                    });
                }
                Some(Answer::Edits) if offers_edits(&req) => {
                    return Ok(ApprovalResponse::ApproveAllEdits);
                }
                Some(Answer::No) => return Ok(ApprovalResponse::Deny { reason: None }),
                Some(Answer::Edits) | None => {
                    let choices = if offers_edits(&req) {
                        "answer y, s, p, e or n"
                    } else {
                        "answer y, s, p or n"
                    };
                    self.terminal.lock().line(Some(Style::Yellow), choices);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use application::ports::approval::ApprovalRequestId;
    use domain::block::DiffSpec;
    use domain::conversation::ToolUseId;
    use domain::permission::RuleDecision;

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

    fn session_rule(text: &str) -> PermissionRule {
        PermissionRule::parse(text, RuleDecision::AllowAlways, RuleScope::Session).unwrap()
    }

    fn write_request() -> ApprovalRequest {
        ApprovalRequest {
            effect: ToolEffect::Write,
            tool: "edit_file".into(),
            subject: "src/ui/a.rs".into(),
            preview: ApprovalPreview::Diff(DiffSpec {
                path: "src/ui/a.rs".into(),
                old_text: None,
                new_text: "x\n".into(),
                unified: "+x\n".into(),
                approval: None,
            }),
            ..command_request()
        }
    }

    #[tokio::test]
    async fn shows_the_command_and_reads_the_answers() {
        let (approval, buf) = approval("what\ny\na\ns\np\ne\nn\n", false);
        let response = approval.request(command_request()).await.unwrap();
        assert_eq!(response, ApprovalResponse::Approve);
        let text = buf.text();
        assert!(
            text.starts_with("  $ cargo test 'a b'\n    in /ws\n"),
            "{text}"
        );
        assert!(
            text.contains(
                "Allow run_command cargo test? [y]es / [s]ession run_command(cargo test *) / \
                 [p]roject run_command(cargo test *) / [n]o: "
            ),
            "{text}"
        );
        assert!(text.contains("answer y, s, p or n"), "{text}");
        let session = ApprovalResponse::ApproveAlways {
            rule: session_rule("run_command(cargo test *)"),
        };
        // `a` (always) is still the session.
        assert_eq!(approval.request(command_request()).await.unwrap(), session);
        assert_eq!(approval.request(command_request()).await.unwrap(), session);
        assert_eq!(
            approval.request(command_request()).await.unwrap(),
            ApprovalResponse::ApproveAlways {
                rule: session_rule("run_command(cargo test *)").with_scope(RuleScope::Project),
            }
        );
        // `e` is not offered for a command: asked again, then `n`.
        assert_eq!(
            approval.request(command_request()).await.unwrap(),
            ApprovalResponse::Deny { reason: None }
        );
        assert!(!buf.text().contains("[e]dits"), "{}", buf.text());
        assert!(matches!(
            approval.request(command_request()).await.unwrap(),
            ApprovalResponse::Deny { reason: Some(_) }
        ));
    }

    #[tokio::test]
    async fn writes_offer_all_edits_and_the_folder() {
        let (approval, buf) = approval("e\ns\n", false);
        assert_eq!(
            approval.request(write_request()).await.unwrap(),
            ApprovalResponse::ApproveAllEdits
        );
        assert!(
            buf.text().contains(
                "[s]ession edit_file(src/ui/*) / [p]roject edit_file(src/ui/*) / \
                 [e]dits (writes only) / [n]o: "
            ),
            "{}",
            buf.text()
        );
        assert_eq!(
            approval.request(write_request()).await.unwrap(),
            ApprovalResponse::ApproveAlways {
                rule: session_rule("edit_file(src/ui/*)"),
            }
        );
    }

    #[tokio::test]
    async fn shells_are_offered_only_the_exact_command() {
        let (approval, buf) = approval("s\n", false);
        let mut req = command_request();
        req.subject = "sh -c 'rm -rf x'".into();
        req.preview = ApprovalPreview::Command {
            command: "sh".into(),
            args: vec!["-c".into(), "rm -rf x".into()],
            cwd: ".".into(),
        };
        let ApprovalResponse::ApproveAlways { rule } = approval.request(req).await.unwrap() else {
            panic!("{}", buf.text());
        };
        assert_eq!(
            rule.matcher,
            domain::permission::RuleMatcher::Exact("sh -c 'rm -rf x'".into())
        );
    }

    #[tokio::test]
    async fn lines_are_read_and_echoed_until_the_input_closes() {
        let (approval, buf) = approval("volver al spec\r\n", false);
        assert_eq!(
            approval.read_line().await.unwrap().as_deref(),
            Some("volver al spec")
        );
        assert_eq!(approval.read_line().await.unwrap(), None);
        assert_eq!(buf.text(), "volver al spec\n\n");
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
        assert_eq!(parse_answer("always"), Some(Answer::Session));
        assert_eq!(parse_answer("a"), Some(Answer::Session));
        assert_eq!(parse_answer(" S"), Some(Answer::Session));
        assert_eq!(parse_answer("sí"), Some(Answer::Yes));
        assert_eq!(parse_answer("Project\r"), Some(Answer::Project));
        assert_eq!(parse_answer("p"), Some(Answer::Project));
        assert_eq!(parse_answer("e"), Some(Answer::Edits));
        assert_eq!(parse_answer("edits"), Some(Answer::Edits));
        assert_eq!(parse_answer("\n"), Some(Answer::No));
        assert_eq!(parse_answer("maybe"), None);
    }
}
