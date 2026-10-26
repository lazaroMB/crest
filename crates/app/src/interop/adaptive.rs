//! Adaptive Cards inbound: a card (`application/vnd.microsoft.card.adaptive`)
//! drawn with crest's nodes and answered through a form.
//!
//! crest reads schema **1.0 to 1.6** ([`MAX_VERSION`]; written against
//! 1.5). Elements: `TextBlock` (headings from `style: heading` or a large
//! size, markdown kept), `RichTextBlock` (text runs as markdown),
//! `Image`/`ImageSet` (inline only from `data:` URIs, a link otherwise:
//! never fetched), `Media` (a link), `Container` (a card when styled or
//! bordered), `ColumnSet`/`Column` (a row of columns), `FactSet` and
//! `Table` (tables), `ActionSet`. Inputs `Input.Text`, `Input.Number`,
//! `Input.Date`, `Input.Time`, `Input.Toggle` and `Input.ChoiceSet` become
//! form fields named by their `id`; `Action.Submit` and `Action.Execute`
//! become form actions; `Action.OpenUrl` is a link the user must click;
//! `Action.ShowCard` shows its card inline in a framed group; content
//! hidden at first (`isVisible: false`) is shown in place, framed, when an
//! `Action.ToggleVisibility` targets it, and left out otherwise.
//!
//! An unknown element uses its `fallback` (an element, or `"drop"`);
//! without one it becomes a placeholder with its JSON and the card's
//! `fallbackText`. On submit, [`Rendered::response`] gives the payload a
//! bot would get: `{type, id, verb, data}` with the input values merged
//! into the action's `data` (toggles as `valueOn`/`valueOff`, several
//! choices comma-separated), unless `associatedInputs` is `none`.

use std::collections::{BTreeMap, BTreeSet};

use domain::block::{Cell, Column, CustomNode, TableSpec};
use domain::form::{FieldKind, FormAction, FormField, FormOption, MAX_ACTIONS};
use domain::interop::json_size;
use regex::RegexBuilder;
use serde_json::{Map, Value, json};

use super::card::{
    Budget, Card, CardError, CardFormat, MAX_DEPTH, MAX_FIELDS, MAX_PAYLOAD_BYTES, clip, image,
    is_date, is_time, link, placeholder,
};

/// Newest schema version crest reads.
pub const MAX_VERSION: (u32, u32) = (1, 6);
/// Longest `regex` of an `Input.Text` that is checked.
const MAX_PATTERN: usize = 1000;
/// Most choices of a choice set.
const MAX_CHOICES: usize = 200;

/// How an input's answer becomes the card's value.
#[derive(Debug, Clone, PartialEq)]
enum Input {
    /// Text, kept as is; checked against `regex` when given.
    Text {
        regex: Option<String>,
        message: Option<String>,
    },
    /// A number.
    Number,
    /// A date, optionally bounded (`YYYY-MM-DD`, compared as text).
    Date {
        min: Option<String>,
        max: Option<String>,
    },
    /// A time, optionally bounded.
    Time {
        min: Option<String>,
        max: Option<String>,
    },
    /// A toggle sending `on` or `off`.
    Toggle { on: String, off: String },
    /// Choices; several are sent comma-separated.
    Choice { multi: bool },
}

#[derive(Debug, Clone, PartialEq)]
struct Binding {
    id: String,
    input: Input,
}

#[derive(Debug, Clone, PartialEq)]
struct Act {
    action: String,
    kind: String,
    card_id: Option<String>,
    verb: Option<String>,
    data: Option<Value>,
    associated: bool,
}

/// A card drawn as a [`Card`], with what answering it needs.
#[derive(Debug, Clone, PartialEq)]
pub struct Rendered {
    /// The card.
    pub card: Card,
    bindings: Vec<Binding>,
    acts: Vec<Act>,
}

struct Walk {
    interactive: bool,
    budget: Budget,
    notes: BTreeSet<String>,
    fields: Vec<FormField>,
    bindings: Vec<Binding>,
    actions: Vec<FormAction>,
    acts: Vec<Act>,
    toggled: BTreeMap<String, String>,
    fallback_text: Option<String>,
    elements: usize,
    inert: bool,
}

/// The card object of `payload`: the card itself, a Bot Framework
/// attachment (`{contentType, content}`), or its JSON text.
fn card_of(payload: &Value) -> Result<Value, CardError> {
    if json_size(payload) > MAX_PAYLOAD_BYTES {
        return Err(CardError::new(format!(
            "the card is larger than {} KiB",
            MAX_PAYLOAD_BYTES / 1024
        )));
    }
    let value = match payload {
        Value::String(text) => serde_json::from_str::<Value>(text)
            .map_err(|err| CardError::new(format!("the card is not JSON: {err}")))?,
        other => other.clone(),
    };
    let value = match value.get("content") {
        Some(content) if value.get("contentType").is_some() => content.clone(),
        _ => value,
    };
    match value.get("type").and_then(Value::as_str) {
        Some("AdaptiveCard") => Ok(value),
        Some(other) => Err(CardError::new(format!(
            "not an Adaptive Card: its type is {other:?}"
        ))),
        None => Err(CardError::new(
            "not an Adaptive Card: it needs \"type\": \"AdaptiveCard\"",
        )),
    }
}

/// `major.minor` of a version string.
fn version(text: &str) -> Option<(u32, u32)> {
    let (major, minor) = text.trim().split_once('.')?;
    Some((major.parse().ok()?, minor.split('.').next()?.parse().ok()?))
}

/// Collects the targets of every `Action.ToggleVisibility`, by element id,
/// with the action's title.
fn toggles(value: &Value, out: &mut BTreeMap<String, String>, depth: usize) {
    if depth > MAX_DEPTH {
        return;
    }
    match value {
        Value::Object(map) => {
            if map.get("type").and_then(Value::as_str) == Some("Action.ToggleVisibility") {
                let title = map
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or("Show")
                    .to_owned();
                for target in map
                    .get("targetElements")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    let id = target
                        .as_str()
                        .or_else(|| target.get("elementId").and_then(Value::as_str));
                    if let Some(id) = id {
                        out.entry(id.to_owned()).or_insert_with(|| title.clone());
                    }
                }
            }
            for child in map.values() {
                toggles(child, out, depth + 1);
            }
        }
        Value::Array(items) => {
            for item in items {
                toggles(item, out, depth + 1);
            }
        }
        _ => {}
    }
}

fn str_of<'a>(e: &'a Value, key: &str) -> Option<&'a str> {
    e.get(key).and_then(Value::as_str)
}

/// `text` with markdown's special characters escaped.
fn escape_markdown(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(
            c,
            '\\' | '`' | '*' | '_' | '[' | ']' | '#' | '<' | '>' | '|' | '~'
        ) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

impl Walk {
    fn elements(&mut self, items: Option<&Value>, depth: usize) -> Vec<CustomNode> {
        items
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|e| self.element(e, depth + 1))
            .collect()
    }

    /// The node of one element; `None` when it became a field, was
    /// hidden, dropped or cut.
    fn element(&mut self, e: &Value, depth: usize) -> Option<CustomNode> {
        if depth > MAX_DEPTH {
            self.notes
                .insert(format!("elements nested deeper than {MAX_DEPTH} were cut"));
            return None;
        }
        if !self.budget.take() {
            return None;
        }
        self.elements += 1;
        if e.get("isVisible").and_then(Value::as_bool) == Some(false) {
            let toggled = str_of(e, "id").and_then(|id| self.toggled.get(id).cloned());
            let mut shown = e.clone();
            if let Some(map) = shown.as_object_mut() {
                map.remove("isVisible");
            }
            return toggled.and_then(|title| {
                let node = self.element(&shown, depth + 1)?;
                Some(CustomNode::Card {
                    title: Some(format!("Hidden at first; “{}” shows it", clip(&title))),
                    children: vec![node],
                })
            });
        }
        let kind = str_of(e, "type").unwrap_or_default();
        match kind {
            "TextBlock" => Some(self.text_block(e)),
            "RichTextBlock" => Some(self.rich_text(e)),
            "Image" => Some(image(
                str_of(e, "url").unwrap_or_default(),
                str_of(e, "altText"),
            )),
            "ImageSet" => Some(CustomNode::Row {
                children: self.elements(e.get("images"), depth),
            }),
            "Media" => {
                let url = e
                    .get("sources")
                    .and_then(Value::as_array)
                    .and_then(|s| s.first())
                    .and_then(|s| str_of(s, "url"))
                    .unwrap_or_default();
                Some(link(
                    &format!("Media: {}", str_of(e, "altText").unwrap_or(url)),
                    url,
                ))
            }
            "Container" => {
                let children = self.elements(e.get("items"), depth);
                let framed = e.get("style").is_some_and(|s| s != "default")
                    || e.get("showBorder").and_then(Value::as_bool) == Some(true);
                Some(if framed {
                    CustomNode::Card {
                        title: None,
                        children,
                    }
                } else {
                    CustomNode::Col { children }
                })
            }
            "ColumnSet" => {
                let columns = e
                    .get("columns")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let children = columns
                    .iter()
                    .filter_map(|column| self.element(column, depth + 1))
                    .collect();
                Some(CustomNode::Row { children })
            }
            "Column" => Some(CustomNode::Col {
                children: self.elements(e.get("items"), depth),
            }),
            "FactSet" => Some(self.facts(e)),
            "Table" => Some(self.table(e, depth)),
            "ActionSet" => {
                let nodes = self.actions(e.get("actions"), depth);
                (!nodes.is_empty()).then_some(CustomNode::Row { children: nodes })
            }
            "Input.Text" | "Input.Number" | "Input.Date" | "Input.Time" | "Input.Toggle"
            | "Input.ChoiceSet" => self.input(kind, e),
            _ => match e.get("fallback") {
                Some(Value::String(s)) if s == "drop" => {
                    self.notes.insert(format!(
                        "element {kind:?} was left out, as its fallback asks"
                    ));
                    None
                }
                Some(fallback @ Value::Object(_)) => self.element(fallback, depth + 1),
                _ => {
                    let note = match &self.fallback_text {
                        Some(text) => {
                            format!("not supported by crest; the card's fallback text: {text}")
                        }
                        None => "not supported by crest".to_owned(),
                    };
                    Some(placeholder(
                        format!("Adaptive Cards element {kind:?}"),
                        Some(note),
                        e,
                    ))
                }
            },
        }
    }

    fn text_block(&mut self, e: &Value) -> CustomNode {
        let text = clip(str_of(e, "text").unwrap_or_default());
        let size = str_of(e, "size").unwrap_or_default().to_ascii_lowercase();
        let heading = str_of(e, "style") == Some("heading");
        let prefix = match size.as_str() {
            "extralarge" => "## ",
            "large" => "### ",
            _ if heading => "### ",
            _ => "",
        };
        let bold = str_of(e, "weight").is_some_and(|w| w.eq_ignore_ascii_case("bolder"));
        let source = if !prefix.is_empty() {
            format!("{prefix}{}", text.lines().next().unwrap_or_default())
        } else if bold && !text.contains('\n') && !text.trim().is_empty() {
            format!("**{}**", text.trim())
        } else {
            text
        };
        CustomNode::Markdown { source }
    }

    fn rich_text(&mut self, e: &Value) -> CustomNode {
        let mut source = String::new();
        let mut links = Vec::new();
        for inline in e
            .get("inlines")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let (text, run) = match inline {
                Value::String(s) => (s.as_str(), None),
                other => (str_of(other, "text").unwrap_or_default(), Some(other)),
            };
            let mut piece = escape_markdown(text);
            if let Some(run) = run {
                let flag = |key: &str| run.get(key).and_then(Value::as_bool) == Some(true);
                if run.get("fontType").and_then(Value::as_str) == Some("monospace") {
                    piece = format!("`{}`", text.replace('`', "'"));
                }
                if str_of(run, "weight").is_some_and(|w| w.eq_ignore_ascii_case("bolder")) {
                    piece = format!("**{piece}**");
                }
                if flag("italic") {
                    piece = format!("*{piece}*");
                }
                if flag("strikethrough") {
                    piece = format!("~~{piece}~~");
                }
                if let Some(url) = run
                    .get("selectAction")
                    .filter(|a| str_of(a, "type") == Some("Action.OpenUrl"))
                    .and_then(|a| str_of(a, "url"))
                {
                    links.push(link(text, url));
                }
            }
            source.push_str(&piece);
        }
        let text = CustomNode::Markdown {
            source: clip(&source),
        };
        if links.is_empty() {
            text
        } else {
            let mut children = vec![text];
            children.extend(links);
            CustomNode::Col { children }
        }
    }

    fn facts(&mut self, e: &Value) -> CustomNode {
        let rows = e
            .get("facts")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|f| {
                vec![
                    Cell::Text(clip(str_of(f, "title").unwrap_or_default())),
                    Cell::Text(clip(str_of(f, "value").unwrap_or_default())),
                ]
            })
            .collect();
        CustomNode::Table(TableSpec {
            columns: vec![Column::new("Fact"), Column::new("Value")],
            rows,
        })
    }

    fn table(&mut self, e: &Value, depth: usize) -> CustomNode {
        let rows: Vec<Vec<Value>> = e
            .get("rows")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|row| {
                row.get("cells")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default()
            })
            .collect();
        let width = rows.iter().map(Vec::len).max().unwrap_or(0).max(
            e.get("columns")
                .and_then(Value::as_array)
                .map_or(0, Vec::len),
        );
        let plain_cell = |cell: &Value| {
            cell.get("items")
                .and_then(Value::as_array)
                .is_none_or(|items| items.iter().all(|i| str_of(i, "type") == Some("TextBlock")))
        };
        if width > 0 && rows.iter().flatten().all(plain_cell) {
            let text_of = |cell: &Value| {
                cell.get("items")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|i| str_of(i, "text"))
                    .collect::<Vec<_>>()
                    .join(" ")
            };
            let header = e
                .get("firstRowAsHeader")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            let mut rows = rows.iter();
            let columns: Vec<Column> = match (header, rows.clone().next()) {
                (true, Some(first)) => {
                    rows.next();
                    (0..width)
                        .map(|i| Column::new(first.get(i).map(text_of).unwrap_or_default()))
                        .collect()
                }
                _ => (1..=width)
                    .map(|i| Column::new(format!("Column {i}")))
                    .collect(),
            };
            let rows = rows
                .map(|row| {
                    (0..width)
                        .map(|i| Cell::Text(clip(&row.get(i).map(text_of).unwrap_or_default())))
                        .collect()
                })
                .collect();
            return CustomNode::Table(TableSpec { columns, rows });
        }
        let children = rows
            .iter()
            .map(|row| CustomNode::Row {
                children: row
                    .iter()
                    .map(|cell| CustomNode::Col {
                        children: self.elements(cell.get("items"), depth + 1),
                    })
                    .collect(),
            })
            .collect();
        CustomNode::Col { children }
    }

    /// The nodes of `actions` that stay in the body (links, shown cards,
    /// inert buttons); submit actions become form actions.
    fn actions(&mut self, actions: Option<&Value>, depth: usize) -> Vec<CustomNode> {
        let mut nodes = Vec::new();
        for a in actions.and_then(Value::as_array).into_iter().flatten() {
            let kind = str_of(a, "type").unwrap_or_default();
            let title = clip(str_of(a, "title").unwrap_or_default());
            match kind {
                "Action.Submit" | "Action.Execute" => {
                    let label = if title.trim().is_empty() {
                        if kind == "Action.Execute" {
                            "Execute"
                        } else {
                            "Submit"
                        }
                        .to_owned()
                    } else {
                        title
                    };
                    if self.interactive && self.actions.len() < MAX_ACTIONS {
                        let action = format!("{}", self.actions.len() + 1);
                        self.acts.push(Act {
                            action: action.clone(),
                            kind: kind.to_owned(),
                            card_id: str_of(a, "id").map(str::to_owned),
                            verb: str_of(a, "verb").map(str::to_owned),
                            data: a.get("data").cloned(),
                            associated: str_of(a, "associatedInputs")
                                .is_none_or(|v| !v.eq_ignore_ascii_case("none")),
                        });
                        self.actions.push(FormAction {
                            id: action,
                            label,
                            primary: str_of(a, "style") == Some("positive"),
                        });
                    } else {
                        if self.interactive {
                            self.notes
                                .insert(format!("only {MAX_ACTIONS} actions are offered"));
                        }
                        self.inert = true;
                        nodes.push(CustomNode::Text {
                            text: format!("[button: {label}]"),
                        });
                    }
                }
                "Action.OpenUrl" => {
                    let url = str_of(a, "url").unwrap_or_default();
                    nodes.push(link(if title.is_empty() { url } else { &title }, url));
                }
                "Action.ShowCard" => {
                    let card = a.get("card").cloned().unwrap_or(Value::Null);
                    let mut children = self.elements(card.get("body"), depth);
                    children.extend(self.actions(card.get("actions"), depth + 1));
                    nodes.push(CustomNode::Card {
                        title: Some(if title.is_empty() {
                            "More".into()
                        } else {
                            title
                        }),
                        children,
                    });
                }
                "Action.ToggleVisibility" => {
                    self.notes.insert(
                        "toggle buttons are not drawn; what they show is drawn in place, framed"
                            .into(),
                    );
                }
                other => {
                    self.notes
                        .insert(format!("action {other:?} is not supported"));
                    nodes.push(CustomNode::Text {
                        text: format!("[button: {title}]"),
                    });
                }
            }
        }
        nodes
    }

    fn input(&mut self, kind: &str, e: &Value) -> Option<CustomNode> {
        let id = str_of(e, "id").filter(|id| !id.trim().is_empty());
        let label = str_of(e, "label")
            .or_else(|| str_of(e, "title").filter(|_| kind != "Input.Toggle"))
            .or_else(|| str_of(e, "placeholder"))
            .or(id)
            .unwrap_or("Input");
        let label = clip(label);
        let value = e.get("value");
        let value_text = value.and_then(|v| match v {
            Value::String(s) => Some(s.clone()),
            Value::Number(n) => Some(n.to_string()),
            Value::Bool(b) => Some(b.to_string()),
            _ => None,
        });
        let required = e.get("isRequired").and_then(Value::as_bool) == Some(true);
        let mut description = str_of(e, "placeholder")
            .filter(|p| *p != label)
            .map(str::to_owned);
        let bound = |key: &str| str_of(e, key).map(str::to_owned);
        let (field_kind, input) = match kind {
            "Input.Text" => {
                let max_length = e
                    .get("maxLength")
                    .and_then(Value::as_u64)
                    .and_then(|n| u32::try_from(n).ok())
                    .filter(|n| *n > 0);
                let default = value_text
                    .clone()
                    .filter(|d| max_length.is_none_or(|max| d.chars().count() <= max as usize));
                if str_of(e, "style") == Some("password") {
                    description = Some("crest does not hide what you type here".into());
                }
                let regex = bound("regex").filter(|r| {
                    let ok = r.len() <= MAX_PATTERN
                        && RegexBuilder::new(r).size_limit(1 << 20).build().is_ok();
                    if !ok {
                        self.notes
                            .insert(format!("the pattern of input {label:?} is not checked"));
                    }
                    ok
                });
                let field = if e.get("isMultiline").and_then(Value::as_bool) == Some(true) {
                    FieldKind::Multiline {
                        default,
                        max_length,
                    }
                } else {
                    FieldKind::String {
                        default,
                        min_length: None,
                        max_length,
                    }
                };
                (
                    field,
                    Input::Text {
                        regex,
                        message: bound("errorMessage"),
                    },
                )
            }
            "Input.Number" => {
                let num = |key: &str| e.get(key).and_then(Value::as_f64).filter(|v| v.is_finite());
                let (min, max) = match (num("min"), num("max")) {
                    (Some(lo), Some(hi)) if lo > hi => (None, None),
                    bounds => bounds,
                };
                let default = num("value")
                    .filter(|v| min.is_none_or(|lo| *v >= lo) && max.is_none_or(|hi| *v <= hi));
                (FieldKind::Number { default, min, max }, Input::Number)
            }
            "Input.Date" | "Input.Time" => {
                let date = kind == "Input.Date";
                let (min, max) = (bound("min"), bound("max"));
                let range = match (&min, &max) {
                    (Some(lo), Some(hi)) => format!(", from {lo} to {hi}"),
                    (Some(lo), None) => format!(", from {lo}"),
                    (None, Some(hi)) => format!(", until {hi}"),
                    (None, None) => String::new(),
                };
                description = Some(if date {
                    format!("Date as YYYY-MM-DD{range}")
                } else {
                    format!("Time as HH:MM{range}")
                });
                (
                    FieldKind::String {
                        default: value_text.clone(),
                        min_length: None,
                        max_length: None,
                    },
                    if date {
                        Input::Date { min, max }
                    } else {
                        Input::Time { min, max }
                    },
                )
            }
            "Input.Toggle" => {
                let on = bound("valueOn").unwrap_or_else(|| "true".into());
                let off = bound("valueOff").unwrap_or_else(|| "false".into());
                let toggle_label = clip(str_of(e, "title").unwrap_or(&label));
                let field = FieldKind::Boolean {
                    default: Some(value_text.as_deref() == Some(on.as_str())),
                };
                return self.add(
                    id,
                    toggle_label,
                    None,
                    required,
                    field,
                    Input::Toggle { on, off },
                    value_text,
                );
            }
            _ => {
                let multi = e.get("isMultiSelect").and_then(Value::as_bool) == Some(true);
                let mut seen = BTreeSet::new();
                let options: Vec<FormOption> = e
                    .get("choices")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .take(MAX_CHOICES)
                    .filter_map(|c| {
                        let value = str_of(c, "value")?.to_owned();
                        let title = str_of(c, "title").map(clip);
                        seen.insert(value.clone()).then_some(FormOption {
                            value,
                            label: title,
                        })
                    })
                    .collect();
                if options.is_empty() {
                    self.notes
                        .insert(format!("choice set {label:?} has no choices"));
                    return Some(CustomNode::Text {
                        text: format!("{label}: (no choices)"),
                    });
                }
                let picked: Vec<String> = value_text
                    .as_deref()
                    .unwrap_or_default()
                    .split(',')
                    .map(str::trim)
                    .filter(|v| options.iter().any(|o| o.value == *v))
                    .map(str::to_owned)
                    .collect();
                let field = if multi {
                    FieldKind::MultiEnum {
                        options,
                        default: picked,
                        min_items: None,
                        max_items: None,
                    }
                } else {
                    FieldKind::Enum {
                        options,
                        default: picked.into_iter().next(),
                    }
                };
                (field, Input::Choice { multi })
            }
        };
        self.add(
            id,
            label,
            description,
            required,
            field_kind,
            input,
            value_text,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn add(
        &mut self,
        id: Option<&str>,
        label: String,
        description: Option<String>,
        required: bool,
        kind: FieldKind,
        input: Input,
        current: Option<String>,
    ) -> Option<CustomNode> {
        let usable = self.interactive
            && self.fields.len() < MAX_FIELDS
            && id.is_some_and(|id| !self.fields.iter().any(|f| f.name == id));
        if !usable {
            if self.interactive {
                match id {
                    None => self
                        .notes
                        .insert(format!("input {label:?} has no id and cannot be answered")),
                    Some(id) if self.fields.iter().any(|f| f.name == id) => {
                        self.notes.insert(format!("input id {id:?} appears twice"))
                    }
                    Some(_) => self
                        .notes
                        .insert(format!("only {MAX_FIELDS} inputs become form fields")),
                };
            }
            let shown = current
                .filter(|c| !c.is_empty())
                .unwrap_or_else(|| "—".into());
            return Some(CustomNode::Text {
                text: format!("{label}: {}", clip(&shown)),
            });
        }
        let id = id.unwrap_or_default().to_owned();
        self.bindings.push(Binding {
            id: id.clone(),
            input,
        });
        self.fields.push(FormField {
            name: id,
            label: Some(label),
            description,
            required,
            kind,
        });
        None
    }
}

/// Draws the Adaptive Card in `payload` (the card, an attachment holding
/// it, or its JSON text). Interactive, inputs become fields and submit
/// actions become form actions; otherwise everything is drawn read-only.
///
/// # Errors
///
/// A [`CardError`] when the payload is too large, not JSON or not an
/// Adaptive Card.
pub fn map(payload: &Value, interactive: bool) -> Result<Rendered, CardError> {
    let card = card_of(payload)?;
    let mut notes = BTreeSet::new();
    match str_of(&card, "version").map(|v| (v, version(v))) {
        None => {
            notes.insert("the card names no version; read as 1.6".to_owned());
        }
        Some((v, None)) => {
            notes.insert(format!("version {v:?} is not major.minor; read as 1.6"));
        }
        Some((v, Some(parsed))) if parsed > MAX_VERSION => {
            notes.insert(format!("version {v} is newer than 1.6; read as 1.6"));
        }
        Some(_) => {}
    }
    let mut toggled = BTreeMap::new();
    toggles(&card, &mut toggled, 0);
    let mut walk = Walk {
        interactive,
        budget: Budget::default(),
        notes: BTreeSet::new(),
        fields: Vec::new(),
        bindings: Vec::new(),
        actions: Vec::new(),
        acts: Vec::new(),
        toggled,
        fallback_text: str_of(&card, "fallbackText").map(clip),
        elements: 0,
        inert: false,
    };
    let mut children = walk.elements(card.get("body"), 0);
    let actions = walk.actions(card.get("actions"), 0);
    if !actions.is_empty() {
        children.push(CustomNode::Row { children: actions });
    }
    if walk.budget.exhausted {
        walk.notes.insert(format!(
            "only the first {} elements were drawn",
            super::card::MAX_NODES
        ));
    }
    if walk.inert {
        walk.notes
            .insert("the buttons do nothing here: this card cannot be answered".into());
    }
    notes.extend(walk.notes);
    let body = CustomNode::Col { children };
    let title = first_heading(&body);
    Ok(Rendered {
        card: Card {
            format: CardFormat::AdaptiveCard,
            title,
            body,
            fields: walk.fields,
            actions: walk.actions,
            notes: notes.into_iter().collect(),
            components: walk.elements,
        },
        bindings: walk.bindings,
        acts: walk.acts,
    })
}

/// The text of the first heading of `body`, as the form's title.
fn first_heading(body: &CustomNode) -> Option<String> {
    let mut found = None;
    body.for_each(&mut |node| {
        if found.is_none()
            && let CustomNode::Markdown { source } = node
            && source.starts_with('#')
        {
            found = Some(source.trim_start_matches('#').trim().to_owned());
        }
    });
    found.filter(|t| !t.is_empty())
}

impl Rendered {
    /// Checks dates, times and patterns of `answer`.
    ///
    /// # Errors
    ///
    /// What is wrong with the first bad value.
    pub fn check(
        &self,
        answer: &BTreeMap<String, Value>,
        _action: Option<&str>,
    ) -> Result<(), String> {
        for binding in &self.bindings {
            let Some(Value::String(text)) = answer.get(&binding.id) else {
                continue;
            };
            let within = |min: &Option<String>, max: &Option<String>| {
                min.as_deref().is_none_or(|lo| text.as_str() >= lo)
                    && max.as_deref().is_none_or(|hi| text.as_str() <= hi)
            };
            match &binding.input {
                Input::Date { min, max } => {
                    if !is_date(text) || !within(min, max) {
                        return Err(format!(
                            "{:?} needs a date in range (YYYY-MM-DD)",
                            binding.id
                        ));
                    }
                }
                Input::Time { min, max } => {
                    if !is_time(text) || !within(min, max) {
                        return Err(format!("{:?} needs a time in range (HH:MM)", binding.id));
                    }
                }
                Input::Text {
                    regex: Some(pattern),
                    message,
                } => {
                    let matches = RegexBuilder::new(pattern)
                        .size_limit(1 << 20)
                        .build()
                        .is_ok_and(|re| re.is_match(text));
                    if !matches {
                        return Err(message
                            .clone()
                            .unwrap_or_else(|| format!("{:?} has the wrong format", binding.id)));
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// The values a bot gets for `answer`, by input id.
    fn inputs(&self, answer: &BTreeMap<String, Value>) -> Map<String, Value> {
        let mut out = Map::new();
        for binding in &self.bindings {
            let Some(value) = answer.get(&binding.id) else {
                continue;
            };
            let value = match (&binding.input, value) {
                (Input::Toggle { on, off }, Value::Bool(b)) => {
                    Value::String(if *b { on.clone() } else { off.clone() })
                }
                (Input::Choice { multi: true }, Value::Array(items)) => Value::String(
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(","),
                ),
                (_, v) => v.clone(),
            };
            out.insert(binding.id.clone(), value);
        }
        out
    }

    /// The payload of the pressed action: `{type, id, verb, data}`, with
    /// the inputs merged into an object `data` (or beside a non-object one
    /// as `inputs`), unless the action's `associatedInputs` is `none`.
    #[must_use]
    pub fn response(&self, answer: &BTreeMap<String, Value>, action: Option<&str>) -> Value {
        let inputs = self.inputs(answer);
        let Some(act) = action.and_then(|id| self.acts.iter().find(|a| a.action == id)) else {
            return json!({ "data": inputs });
        };
        let mut message = Map::new();
        message.insert("type".into(), json!(act.kind));
        if let Some(id) = &act.card_id {
            message.insert("id".into(), json!(id));
        }
        if let Some(verb) = &act.verb {
            message.insert("verb".into(), json!(verb));
        }
        match (&act.data, act.associated) {
            (Some(Value::Object(data)), true) => {
                let mut merged = data.clone();
                merged.extend(inputs);
                message.insert("data".into(), Value::Object(merged));
            }
            (None, true) => {
                message.insert("data".into(), Value::Object(inputs));
            }
            (Some(data), true) => {
                message.insert("data".into(), data.clone());
                message.insert("inputs".into(), Value::Object(inputs));
            }
            (data, false) => {
                if let Some(data) = data {
                    message.insert("data".into(), data.clone());
                }
            }
        }
        Value::Object(message)
    }
}

#[cfg(test)]
mod tests;
