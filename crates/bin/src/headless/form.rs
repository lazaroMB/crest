//! [`StdinForm`]: asks the fields of an `ask_user` form one by one on
//! stdin.
//!
//! Each field shows its label (`*` when required), its description and its
//! options, then reads one line: `Enter` alone keeps the default or skips
//! an optional field, a choice takes the option's number or value (several
//! separated by commas), a boolean takes `y` or `n`, and a text area reads
//! lines until one with a single `.`. A value the field does not accept
//! is explained and asked again. `/cancel` on any line cancels the form.
//! Without a terminal on stdin nobody can answer: every form is cancelled
//! with that reason.

use std::collections::BTreeMap;
use std::io::IsTerminal as _;

use application::ports::form::{FormPort, FormRequest, FormResponse};
use async_trait::async_trait;
use domain::form::{FieldKind, FormField, FormOption};
use serde_json::Value;

use super::StdinApproval;
use super::terminal::{Style, Terminal};

/// The line that cancels the form.
const CANCEL: &str = "/cancel";

/// Asks forms on the terminal, reading the answers through the approval
/// input (they share stdin).
#[derive(Debug, Clone)]
pub struct StdinForm {
    terminal: Terminal,
    input: StdinApproval,
    /// Why nobody can answer, when stdin is not a terminal.
    unavailable: Option<String>,
}

/// What reading one field gave.
enum Read {
    /// The value, or `None` for an optional field left empty.
    Value(Option<Value>),
    /// The user typed `/cancel`.
    Cancel,
    /// The input is closed.
    Closed,
}

fn option_label(option: &FormOption) -> &str {
    option.label.as_deref().unwrap_or(&option.value)
}

/// The option `token` names: its 1-based number or its value.
fn find_option<'a>(options: &'a [FormOption], token: &str) -> Option<&'a FormOption> {
    let token = token.trim();
    token
        .parse::<usize>()
        .ok()
        .and_then(|n| n.checked_sub(1))
        .and_then(|i| options.get(i))
        .or_else(|| options.iter().find(|o| o.value == token))
}

/// The value `line` gives `kind` (`line` is not empty), or what is wrong.
fn parse(kind: &FieldKind, line: &str) -> Result<Value, String> {
    let text = line.trim();
    let value = match kind {
        FieldKind::String { .. } | FieldKind::Multiline { .. } => Value::String(line.to_owned()),
        FieldKind::Number { .. } => text
            .parse::<f64>()
            .ok()
            .and_then(serde_json::Number::from_f64)
            .map(Value::Number)
            .ok_or_else(|| kind.hint())?,
        FieldKind::Integer { .. } => Value::from(text.parse::<i64>().map_err(|_| kind.hint())?),
        FieldKind::Boolean { .. } => match text.to_lowercase().as_str() {
            "y" | "yes" | "true" | "s" | "si" | "sí" => Value::Bool(true),
            "n" | "no" | "false" => Value::Bool(false),
            _ => return Err(kind.hint()),
        },
        FieldKind::Enum { options, .. } => {
            let option = find_option(options, text).ok_or_else(|| kind.hint())?;
            Value::String(option.value.clone())
        }
        FieldKind::MultiEnum { options, .. } => {
            let mut values = Vec::new();
            for token in text.split(',').filter(|t| !t.trim().is_empty()) {
                let option = find_option(options, token)
                    .ok_or_else(|| format!("{:?} is not an option", token.trim()))?;
                let value = Value::String(option.value.clone());
                if !values.contains(&value) {
                    values.push(value);
                }
            }
            Value::Array(values)
        }
    };
    if kind.accepts(&value) {
        Ok(value)
    } else {
        Err(kind.hint())
    }
}

/// The default of `kind` as a value, if it has one.
fn default_value(kind: &FieldKind) -> Option<Value> {
    match kind {
        FieldKind::String { default, .. } | FieldKind::Multiline { default, .. } => {
            default.clone().map(Value::String)
        }
        FieldKind::Number { default, .. } => default
            .and_then(serde_json::Number::from_f64)
            .map(Value::Number),
        FieldKind::Integer { default, .. } => default.map(Value::from),
        FieldKind::Boolean { default } => default.map(Value::Bool),
        FieldKind::Enum { default, .. } => default.clone().map(Value::String),
        FieldKind::MultiEnum { default, .. } => (!default.is_empty())
            .then(|| Value::Array(default.iter().cloned().map(Value::String).collect())),
    }
}

/// The default as the prompt shows it.
fn default_text(kind: &FieldKind) -> Option<String> {
    match (kind, default_value(kind)?) {
        (FieldKind::Multiline { .. }, _) => Some("keep the text given".to_owned()),
        (_, Value::String(s)) => Some(s),
        (_, Value::Bool(b)) => Some(if b { "y" } else { "n" }.to_owned()),
        (_, Value::Array(items)) => Some(
            items
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(","),
        ),
        (_, other) => Some(other.to_string()),
    }
}

impl StdinForm {
    /// Asks on `terminal`, reading through `input`.
    #[must_use]
    pub fn new(terminal: Terminal, input: StdinApproval) -> Self {
        Self {
            terminal,
            input,
            unavailable: None,
        }
    }

    /// Asks on stdout and stdin when stdin is a terminal; otherwise every
    /// form is cancelled.
    #[must_use]
    pub fn stdin(terminal: Terminal, input: StdinApproval) -> Self {
        let unavailable = (!std::io::stdin().is_terminal())
            .then(|| "stdin is not a terminal, so nobody can fill the form".to_owned());
        Self {
            unavailable,
            ..Self::new(terminal, input)
        }
    }

    fn header(&self, req: &FormRequest) {
        let mut w = self.terminal.lock();
        w.break_line();
        w.line(Some(Style::Bold), &format!("Form: {}", req.spec.title));
        if let Some(description) = &req.spec.description {
            w.line(Some(Style::Dim), &format!("  {description}"));
        }
        if let Some(body) = &req.spec.body {
            for line in application::interop::plain_text(body).lines().take(40) {
                w.line(None, &format!("  {line}"));
            }
        }
        if let Some(error) = &req.error {
            w.line(
                Some(Style::Yellow),
                &format!("  the previous answer was rejected: {error}"),
            );
        }
        w.line(
            Some(Style::Dim),
            &format!("  Enter keeps the default or skips; {CANCEL} cancels the form"),
        );
    }

    fn show_field(&self, field: &FormField) {
        let mut w = self.terminal.lock();
        let label = field.label.as_deref().unwrap_or(&field.name);
        let required = if field.required { " *" } else { "" };
        w.line(Some(Style::Bold), &format!("{label}{required}"));
        if let Some(description) = &field.description {
            w.line(Some(Style::Dim), &format!("  {description}"));
        }
        if let FieldKind::Enum { options, .. } | FieldKind::MultiEnum { options, .. } = &field.kind
        {
            for (i, option) in options.iter().enumerate() {
                let label = option_label(option);
                let text = if label == option.value {
                    format!("  {}) {label}", i + 1)
                } else {
                    format!("  {}) {label} ({})", i + 1, option.value)
                };
                w.line(None, &text);
            }
        }
    }

    fn prompt(&self, field: &FormField) {
        let what = match &field.kind {
            FieldKind::Multiline { .. } => "lines, then a single . to end",
            FieldKind::Boolean { .. } => "y/n",
            FieldKind::Enum { .. } => "number or value",
            FieldKind::MultiEnum { .. } => "numbers or values, separated by commas",
            FieldKind::Number { .. } => "number",
            FieldKind::Integer { .. } => "whole number",
            FieldKind::String { .. } => "text",
        };
        let default = default_text(&field.kind)
            .map(|d| format!(", default {d}"))
            .unwrap_or_default();
        let mut w = self.terminal.lock();
        w.break_line();
        w.styled(Style::Cyan, &format!("  ({what}{default}): "));
    }

    /// One line, or the text of a text area; `None` once stdin closes.
    async fn read_text(&self, field: &FormField) -> Result<Option<String>, String> {
        if !matches!(field.kind, FieldKind::Multiline { .. }) {
            return self.input.read_line().await;
        }
        let mut lines = Vec::new();
        loop {
            match self.input.read_line().await? {
                None if lines.is_empty() => return Ok(None),
                None => break,
                Some(line) if line.trim() == "." => break,
                Some(line) if line.trim() == CANCEL => return Ok(Some(CANCEL.to_owned())),
                Some(line) => lines.push(line),
            }
        }
        Ok(Some(lines.join("\n")))
    }

    async fn read_field(&self, field: &FormField) -> Result<Read, String> {
        self.show_field(field);
        loop {
            self.prompt(field);
            let Some(line) = self.read_text(field).await? else {
                return Ok(Read::Closed);
            };
            if line.trim() == CANCEL {
                return Ok(Read::Cancel);
            }
            if line.trim().is_empty() {
                match default_value(&field.kind) {
                    Some(value) => return Ok(Read::Value(Some(value))),
                    None if field.required => {
                        self.terminal
                            .lock()
                            .line(Some(Style::Yellow), "  this field is required");
                        continue;
                    }
                    None => return Ok(Read::Value(None)),
                }
            }
            match parse(&field.kind, &line) {
                Ok(value) => return Ok(Read::Value(Some(value))),
                Err(problem) => self
                    .terminal
                    .lock()
                    .line(Some(Style::Yellow), &format!("  {problem}")),
            }
        }
    }
}

#[async_trait]
impl FormPort for StdinForm {
    async fn ask(&self, req: FormRequest) -> FormResponse {
        if let Some(reason) = &self.unavailable {
            self.terminal.lock().line(
                Some(Style::Dim),
                &format!("  form {:?} cancelled: {reason}", req.spec.title),
            );
            return FormResponse::cancelled(reason.clone());
        }
        self.header(&req);
        let mut answer = BTreeMap::new();
        for field in &req.spec.fields {
            match self.read_field(field).await {
                Ok(Read::Value(Some(value))) => {
                    answer.insert(field.name.clone(), value);
                }
                Ok(Read::Value(None)) => {}
                Ok(Read::Cancel) => return FormResponse::Cancelled { reason: None },
                Ok(Read::Closed) => return FormResponse::cancelled("stdin is closed"),
                Err(err) => return FormResponse::cancelled(err),
            }
        }
        let action = match req.spec.actions.as_slice() {
            [] => None,
            [only] => Some(only.id.clone()),
            actions => {
                let field = FormField {
                    name: "action".into(),
                    label: Some("Action".into()),
                    description: None,
                    required: true,
                    kind: FieldKind::Enum {
                        options: actions
                            .iter()
                            .map(|a| FormOption {
                                value: a.id.clone(),
                                label: Some(a.label.clone()),
                            })
                            .collect(),
                        default: req.spec.default_action().map(|a| a.id.clone()),
                    },
                };
                match self.read_field(&field).await {
                    Ok(Read::Value(Some(Value::String(id)))) => Some(id),
                    Ok(Read::Value(_)) => req.spec.default_action().map(|a| a.id.clone()),
                    Ok(Read::Cancel) => return FormResponse::Cancelled { reason: None },
                    Ok(Read::Closed) => return FormResponse::cancelled("stdin is closed"),
                    Err(err) => return FormResponse::cancelled(err),
                }
            }
        };
        FormResponse::Submitted { answer, action }
    }
}

#[cfg(test)]
mod tests {
    use application::ports::form::FormRequestId;
    use domain::block::BlockId;
    use domain::conversation::ToolUseId;
    use domain::form::FormSpec;
    use serde_json::json;

    use super::*;
    use crate::headless::terminal::tests::{Buffer, terminal};

    fn request() -> FormRequest {
        let spec: FormSpec = serde_json::from_value(json!({
            "title": "Run settings",
            "fields": [
                {"name": "label", "type": "string", "required": true, "label": "Label"},
                {"name": "n", "type": "integer", "min": 1, "max": 9, "default": 2},
                {"name": "paired", "type": "boolean"},
                {"name": "test", "type": "enum",
                 "options": [{"value": "welch", "label": "Welch t"}, {"value": "student"}]},
                {"name": "plots", "type": "multi_enum",
                 "options": [{"value": "qq"}, {"value": "hist"}]},
                {"name": "notes", "type": "multiline"}
            ]
        }))
        .unwrap();
        FormRequest {
            id: FormRequestId(1),
            block_id: BlockId::new(),
            tool_use_id: ToolUseId::new("toolu_1"),
            spec,
            error: None,
        }
    }

    fn stdin_form(answers: &str) -> (StdinForm, Buffer) {
        let (term, buf) = terminal(false);
        let input = Box::new(std::io::Cursor::new(answers.to_owned().into_bytes()));
        let reader = StdinApproval::new(term.clone(), input, false);
        (StdinForm::new(term, reader), buf)
    }

    #[tokio::test]
    async fn asks_each_field_and_explains_bad_values() {
        let (form, buf) = stdin_form("\nrun 1\n12\n\nmaybe\ny\n2\nqq, 2,hist\nfirst\nsecond\n.\n");
        let response = form.ask(request()).await;
        let expected: BTreeMap<String, Value> = serde_json::from_value(json!({
            "label": "run 1", "n": 2, "paired": true, "test": "student",
            "plots": ["qq", "hist"], "notes": "first\nsecond"
        }))
        .unwrap();
        assert_eq!(response, FormResponse::submitted(expected));
        let text = buf.text();
        assert!(text.contains("Form: Run settings"), "{text}");
        assert!(text.contains("Label *"), "{text}");
        assert!(text.contains("this field is required"), "{text}");
        assert!(
            text.contains("Enter a whole number between 1 and 9"),
            "{text}"
        );
        assert!(text.contains("Answer yes or no"), "{text}");
        assert!(text.contains("1) Welch t (welch)"), "{text}");
        assert!(text.contains("default 2"), "{text}");
    }

    #[tokio::test]
    async fn cancel_or_a_closed_input_cancels() {
        let (form, _) = stdin_form("run\n/cancel\n");
        assert_eq!(
            form.ask(request()).await,
            FormResponse::Cancelled { reason: None }
        );
        let (form, _) = stdin_form("run\n");
        assert_eq!(
            form.ask(request()).await,
            FormResponse::cancelled("stdin is closed")
        );
    }

    #[tokio::test]
    async fn without_a_terminal_every_form_is_cancelled_with_the_reason() {
        let (mut form, buf) = stdin_form("run\n");
        form.unavailable = Some("stdin is not a terminal".into());
        assert_eq!(
            form.ask(request()).await,
            FormResponse::cancelled("stdin is not a terminal")
        );
        assert!(buf.text().contains("cancelled: stdin is not a terminal"));
    }

    #[tokio::test]
    async fn card_forms_ask_which_action_to_send() {
        let spec: FormSpec = serde_json::from_value(json!({
            "title": "Deploy",
            "body": {"type": "text", "text": "Release notes"},
            "fields": [],
            "actions": [{"id": "later", "label": "Later"}, {"id": "ship", "label": "Ship", "primary": true}]
        }))
        .unwrap();
        let req = FormRequest { spec, ..request() };
        let (form, buf) = stdin_form("1\n");
        assert_eq!(
            form.ask(req.clone()).await,
            FormResponse::Submitted {
                answer: BTreeMap::new(),
                action: Some("later".into()),
            }
        );
        assert!(buf.text().contains("2) Ship (ship)"), "{}", buf.text());
        assert!(buf.text().contains("  Release notes"), "{}", buf.text());
        let (form, _) = stdin_form("\n");
        assert_eq!(
            form.ask(req).await,
            FormResponse::Submitted {
                answer: BTreeMap::new(),
                action: Some("ship".into()),
            }
        );
    }
}
