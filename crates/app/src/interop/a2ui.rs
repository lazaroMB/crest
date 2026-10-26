//! A2UI inbound: `application/a2ui+json` message streams drawn with
//! crest's nodes.
//!
//! crest reads **A2UI v0.9** ([`VERSION`]): messages `createSurface`,
//! `updateComponents`, `updateDataModel` and `deleteSurface`, components as
//! flat objects (`{"id", "component": "Text", ...}`) whose tree starts at
//! the component `root`, children as id lists or templates (`{"path",
//! "componentId"}`), and values that are literals or data bindings
//! (`{"path": "/user/name"}`, relative inside templates). v0.8 streams
//! (`beginRendering`, `surfaceUpdate`, `dataModelUpdate`, wrapped
//! components, `literalString` values, `explicitList` children,
//! `MultipleChoice`) are normalised to v0.9 first; other versions are read
//! as v0.9 with a note. Function calls (`{"call": ...}`) are not
//! evaluated: they read as empty, with a note.
//!
//! The basic catalog maps onto existing nodes: `Text` → markdown (`h1`–`h5`
//! as headings), `Image` → an inline image for `data:` URIs or a link
//! otherwise (never fetched), `Row`/`Column`/`List` → layouts, `Card` →
//! card, `Tabs` → one card per tab, `Modal` → its trigger and a card with
//! its content, `Divider` → a rule, `Video`/`AudioPlayer` → links, `Icon`
//! → its name. Inputs (`TextField`, `CheckBox`, `ChoicePicker`, `Slider`,
//! `DateTimeInput`) become form fields bound to their data paths and
//! `Button`s with an event become form actions, when the card can be
//! answered; otherwise they are drawn read-only with a note. Anything else
//! becomes a placeholder with its JSON.
//!
//! Bindings are resolved against the data model when the card is drawn;
//! on submit, [`Rendered::response`] writes the answers into a copy of the
//! data model at their paths and resolves the pressed button's event
//! context against it, giving the v0.9 `action` message.

use std::collections::{BTreeMap, BTreeSet};

use domain::block::CustomNode;
use domain::form::{FieldKind, FormAction, FormField, FormOption};
use domain::interop::json_size;
use serde_json::{Map, Value, json};

use super::card::{
    Budget, Card, CardError, CardFormat, MAX_DEPTH, MAX_FIELDS, MAX_ITEMS, MAX_NODES,
    MAX_PAYLOAD_BYTES, clip, image, is_date, is_time, link, placeholder, plain_text, unique_name,
};

/// The A2UI version crest implements.
pub const VERSION: &str = "v0.9";
/// Most messages read from one payload.
pub const MAX_MESSAGES: usize = 1000;
/// Most surfaces drawn.
pub const MAX_SURFACES: usize = 8;
/// Most options of a choice picker.
const MAX_OPTIONS: usize = 200;

/// One surface after every message was applied.
#[derive(Debug, Clone, PartialEq)]
struct Surface {
    id: String,
    catalog: Option<String>,
    root: String,
    components: BTreeMap<String, Value>,
    data: Value,
    send_data_model: bool,
}

impl Surface {
    fn new(id: &str) -> Self {
        Self {
            id: id.to_owned(),
            catalog: None,
            root: "root".to_owned(),
            components: BTreeMap::new(),
            data: Value::Object(Map::new()),
            send_data_model: false,
        }
    }
}

/// An A2UI stream, applied: its surfaces in creation order.
#[derive(Debug, Clone, PartialEq)]
pub struct A2ui {
    surfaces: Vec<Surface>,
    notes: Vec<String>,
}

/// How an input writes its answer back into the data model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Bind {
    /// The value as answered (text, number, boolean).
    Plain,
    /// A single choice, written as a one-item list (A2UI choices are lists).
    OneOfList,
    /// A date `YYYY-MM-DD`.
    Date,
    /// A time `HH:MM`.
    Time,
    /// A date and time `YYYY-MM-DDTHH:MM`.
    DateTime,
}

#[derive(Debug, Clone, PartialEq)]
struct Binding {
    field: String,
    surface: usize,
    path: Option<String>,
    bind: Bind,
}

#[derive(Debug, Clone, PartialEq)]
struct Button {
    action: String,
    surface: usize,
    component: String,
    name: String,
    /// Context entries with paths made absolute.
    context: Vec<(String, Value)>,
}

/// A stream drawn as a [`Card`], with what answering it needs.
#[derive(Debug, Clone, PartialEq)]
pub struct Rendered {
    /// The card.
    pub card: Card,
    bindings: Vec<Binding>,
    buttons: Vec<Button>,
    surfaces: Vec<(String, Value, bool)>,
}

/// Reads `payload`: a list of messages, `{"messages": [...]}`, one message,
/// or text holding any of those, JSON lines or a run of JSON values.
///
/// # Errors
///
/// A [`CardError`] when the payload is too large, not JSON, or has no
/// message.
pub fn parse(payload: &Value) -> Result<A2ui, CardError> {
    let messages = messages(payload)?;
    let mut surfaces: Vec<Surface> = Vec::new();
    let mut notes = BTreeSet::new();
    for message in messages.iter().take(MAX_MESSAGES) {
        apply(message, &mut surfaces, &mut notes);
    }
    if messages.len() > MAX_MESSAGES {
        notes.insert(format!(
            "only the first {MAX_MESSAGES} of {} messages were read",
            messages.len()
        ));
    }
    if surfaces.is_empty() {
        return Err(CardError::new("the A2UI messages create no surface"));
    }
    let total: usize = surfaces.iter().map(|s| s.components.len()).sum();
    if total > MAX_NODES {
        return Err(CardError::new(format!(
            "{total} components, at most {MAX_NODES}"
        )));
    }
    Ok(A2ui {
        surfaces,
        notes: notes.into_iter().collect(),
    })
}

/// The messages of `payload`.
fn messages(payload: &Value) -> Result<Vec<Value>, CardError> {
    if json_size(payload) > MAX_PAYLOAD_BYTES {
        return Err(CardError::new(format!(
            "the A2UI payload is larger than {} KiB",
            MAX_PAYLOAD_BYTES / 1024
        )));
    }
    match payload {
        Value::Array(items) => Ok(items.clone()),
        Value::Object(map) => match map.get("messages") {
            Some(Value::Array(items)) if map.len() == 1 => Ok(items.clone()),
            _ => Ok(vec![payload.clone()]),
        },
        Value::String(text) => {
            if let Ok(value) = serde_json::from_str::<Value>(text)
                && !value.is_string()
            {
                return messages(&value);
            }
            // JSON lines, or any run of JSON values.
            serde_json::Deserializer::from_str(text)
                .into_iter::<Value>()
                .map(|value| {
                    value.map_err(|err| CardError::new(format!("the A2UI text is not JSON: {err}")))
                })
                .collect()
        }
        _ => Err(CardError::new(
            "A2UI messages must be a JSON object, a list of them or JSON lines",
        )),
    }
}

fn surface<'a>(surfaces: &'a mut Vec<Surface>, id: &str) -> Option<&'a mut Surface> {
    if let Some(ix) = surfaces.iter().position(|s| s.id == id) {
        return surfaces.get_mut(ix);
    }
    if surfaces.len() >= MAX_SURFACES {
        return None;
    }
    surfaces.push(Surface::new(id));
    surfaces.last_mut()
}

/// Applies one message.
fn apply(message: &Value, surfaces: &mut Vec<Surface>, notes: &mut BTreeSet<String>) {
    let Some(map) = message.as_object() else {
        notes.insert("a message that is not an object was skipped".into());
        return;
    };
    if let Some(version) = map.get("version").and_then(Value::as_str) {
        let v = version.trim_start_matches('v');
        if !v.starts_with("0.9") && !v.starts_with("0.8") {
            notes.insert(format!("A2UI version {version:?} was read as {VERSION}"));
        }
    }
    let body = |key: &str| map.get(key).and_then(Value::as_object);
    let id_of = |body: &Map<String, Value>| {
        body.get("surfaceId")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty() && s.len() <= 256)
            .map(str::to_owned)
    };
    let mut known = false;
    for key in [
        "createSurface",
        "beginRendering",
        "updateComponents",
        "surfaceUpdate",
        "updateDataModel",
        "dataModelUpdate",
        "deleteSurface",
    ] {
        let Some(body) = body(key) else {
            continue;
        };
        known = true;
        let Some(id) = id_of(body) else {
            notes.insert(format!("a {key} message without a surfaceId was skipped"));
            continue;
        };
        if key == "deleteSurface" {
            surfaces.retain(|s| s.id != id);
            continue;
        }
        let Some(target) = surface(surfaces, &id) else {
            notes.insert(format!("only {MAX_SURFACES} surfaces are drawn"));
            continue;
        };
        match key {
            "createSurface" => {
                target.catalog = body
                    .get("catalogId")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                target.send_data_model = body
                    .get("sendDataModel")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
            }
            "beginRendering" => {
                if let Some(root) = body.get("root").and_then(Value::as_str) {
                    target.root = root.to_owned();
                }
            }
            "updateComponents" | "surfaceUpdate" => {
                let list = body
                    .get("components")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                for component in list {
                    let wrapped = component.get("component").is_some_and(Value::is_object);
                    let (component, inits) = if wrapped {
                        normalize_v08(&component)
                    } else {
                        (Some(component), Vec::new())
                    };
                    for (path, value) in inits {
                        if pointer(&target.data, &path).is_none() {
                            set_pointer(&mut target.data, &path, Some(value));
                        }
                    }
                    match component.as_ref().and_then(|c| {
                        c.get("id")
                            .and_then(Value::as_str)
                            .filter(|id| !id.is_empty() && id.len() <= 256)
                            .map(str::to_owned)
                    }) {
                        Some(cid)
                            if target.components.len() < MAX_NODES
                                || target.components.contains_key(&cid) =>
                        {
                            if let Some(component) = component {
                                target.components.insert(cid, component);
                            }
                        }
                        Some(_) => {
                            notes.insert(format!(
                                "only {MAX_NODES} components of a surface are read"
                            ));
                        }
                        None => {
                            notes.insert("a component without an id was skipped".into());
                        }
                    }
                }
            }
            "updateDataModel" => {
                let path = body.get("path").and_then(Value::as_str).unwrap_or("/");
                set_pointer(&mut target.data, path, body.get("value").cloned());
            }
            "dataModelUpdate" => {
                let path = body.get("path").and_then(Value::as_str).unwrap_or("/");
                let contents = body
                    .get("contents")
                    .and_then(Value::as_array)
                    .map(|c| contents_value(c, 0))
                    .unwrap_or_else(|| Value::Object(Map::new()));
                let mut current = pointer(&target.data, path).cloned().unwrap_or(Value::Null);
                merge(&mut current, contents, 0);
                set_pointer(&mut target.data, path, Some(current));
            }
            _ => {}
        }
    }
    if !known {
        let keys: Vec<&str> = map
            .keys()
            .map(String::as_str)
            .filter(|k| *k != "version")
            .collect();
        notes.insert(format!("a message with {keys:?} was skipped"));
    }
}

/// A v0.8 `dataModelUpdate`'s `contents` as a JSON object.
fn contents_value(entries: &[Value], depth: usize) -> Value {
    let mut map = Map::new();
    if depth > MAX_DEPTH {
        return Value::Object(map);
    }
    for entry in entries {
        let Some(key) = entry.get("key").and_then(Value::as_str) else {
            continue;
        };
        let value = if let Some(v) = entry.get("valueString") {
            v.clone()
        } else if let Some(v) = entry.get("valueNumber") {
            v.clone()
        } else if let Some(v) = entry.get("valueBoolean") {
            v.clone()
        } else if let Some(Value::Array(inner)) = entry.get("valueMap") {
            contents_value(inner, depth + 1)
        } else {
            Value::Null
        };
        map.insert(key.to_owned(), value);
    }
    Value::Object(map)
}

/// Merges `value` into `target`: objects key by key (v0.8 updates add to
/// the data model), anything else replaced.
fn merge(target: &mut Value, value: Value, depth: usize) {
    match (target, value) {
        (Value::Object(target), Value::Object(entries)) if depth < MAX_DEPTH => {
            for (k, v) in entries {
                merge(target.entry(k).or_insert(Value::Null), v, depth + 1);
            }
        }
        (target, value) => *target = value,
    }
}

/// Keys of a v0.8 bound value.
const BOUND_KEYS: [&str; 5] = [
    "path",
    "literalString",
    "literalNumber",
    "literalBoolean",
    "literalArray",
];

/// A v0.8 bound value as v0.9: `{"path"}` or the literal; a path with a
/// literal also initialises the data there (pushed to `inits`).
fn bound(value: &Value, inits: &mut Vec<(String, Value)>, depth: usize) -> Value {
    if depth > MAX_DEPTH {
        return Value::Null;
    }
    match value {
        Value::Object(map)
            if !map.is_empty() && map.keys().all(|k| BOUND_KEYS.contains(&k.as_str())) =>
        {
            let literal = BOUND_KEYS[1..].iter().find_map(|k| map.get(*k)).cloned();
            match map.get("path").and_then(Value::as_str) {
                Some(path) => {
                    if let Some(literal) = literal {
                        inits.push((path.to_owned(), literal));
                    }
                    json!({ "path": path })
                }
                None => literal.unwrap_or(Value::Null),
            }
        }
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), bound(v, inits, depth + 1)))
                .collect(),
        ),
        Value::Array(items) => {
            Value::Array(items.iter().map(|v| bound(v, inits, depth + 1)).collect())
        }
        other => other.clone(),
    }
}

/// A v0.8 component (`{"id", "component": {"Type": {...}}}`) as v0.9, and
/// the data its bound literals initialise.
fn normalize_v08(component: &Value) -> (Option<Value>, Vec<(String, Value)>) {
    let mut inits = Vec::new();
    let Some(id) = component.get("id").cloned() else {
        return (None, inits);
    };
    let Some(wrapper) = component.get("component").and_then(Value::as_object) else {
        return (None, inits);
    };
    let Some((kind, props)) = wrapper.iter().next().filter(|_| wrapper.len() == 1) else {
        return (None, inits);
    };
    let mut out = Map::new();
    let mut kind = kind.clone();
    for (key, value) in props.as_object().into_iter().flatten() {
        let value = match key.as_str() {
            "children" => match value.as_object() {
                Some(c) if c.contains_key("explicitList") => {
                    c.get("explicitList").cloned().unwrap_or(json!([]))
                }
                Some(c) => match c.get("template") {
                    Some(t) => json!({
                        "componentId": t.get("componentId").cloned().unwrap_or(Value::Null),
                        "path": t.get("dataBinding").cloned().unwrap_or(Value::Null),
                    }),
                    None => value.clone(),
                },
                None => value.clone(),
            },
            "child" | "entryPointChild" | "contentChild" => value.clone(),
            "action" => {
                let name = value.get("name").cloned().unwrap_or(Value::Null);
                let mut context = Map::new();
                for entry in value
                    .get("context")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    if let Some(k) = entry.get("key").and_then(Value::as_str) {
                        let v = entry
                            .get("value")
                            .map_or(Value::Null, |v| bound(v, &mut inits, 0));
                        context.insert(k.to_owned(), v);
                    }
                }
                json!({ "event": { "name": name, "context": context } })
            }
            _ => bound(value, &mut inits, 0),
        };
        let key = match (kind.as_str(), key.as_str()) {
            (_, "usageHint") => "variant",
            ("TextField", "text") => "value",
            ("TextField", "textFieldType") => "variant",
            ("MultipleChoice", "selections") => "value",
            ("Slider", "minValue") => "min",
            ("Slider", "maxValue") => "max",
            ("Tabs", "tabItems") => "tabs",
            ("Modal", "entryPointChild") => "trigger",
            ("Modal", "contentChild") => "content",
            (_, other) => other,
        };
        out.insert(key.to_owned(), value);
    }
    if kind == "MultipleChoice" {
        let exclusive = out
            .get("maxAllowedSelections")
            .and_then(Value::as_u64)
            .is_some_and(|n| n == 1);
        out.insert(
            "variant".into(),
            json!(if exclusive {
                "mutuallyExclusive"
            } else {
                "multipleSelection"
            }),
        );
        kind = "ChoicePicker".into();
    }
    if kind == "Button" && props.get("primary").and_then(Value::as_bool) == Some(true) {
        out.insert("variant".into(), json!("primary"));
    }
    out.insert("id".into(), id);
    out.insert("component".into(), Value::String(kind));
    (Some(Value::Object(out)), inits)
}

/// `token` escaped for a JSON pointer.
fn escape(token: &str) -> String {
    token.replace('~', "~0").replace('/', "~1")
}

/// The value at JSON pointer `path` (`/` or empty is the root).
fn pointer<'a>(root: &'a Value, path: &str) -> Option<&'a Value> {
    if path.is_empty() || path == "/" {
        return Some(root);
    }
    root.pointer(path)
}

/// Sets (or, with `None`, removes) the value at `path`, creating objects
/// on the way; an index one past an array's end appends.
fn set_pointer(root: &mut Value, path: &str, value: Option<Value>) {
    if path.is_empty() || path == "/" {
        *root = match value {
            Some(v @ Value::Object(_)) => v,
            _ => Value::Object(Map::new()),
        };
        return;
    }
    let tokens: Vec<String> = path
        .trim_start_matches('/')
        .split('/')
        .map(|t| t.replace("~1", "/").replace("~0", "~"))
        .collect();
    if tokens.len() > MAX_DEPTH {
        return;
    }
    let mut node = root;
    for (i, token) in tokens.iter().enumerate() {
        let last = i + 1 == tokens.len();
        if !node.is_object() && !node.is_array() {
            *node = Value::Object(Map::new());
        }
        let next = match node {
            Value::Array(items) => match token.parse::<usize>() {
                Ok(ix) if ix < items.len() => {
                    if last {
                        match value {
                            Some(v) => items[ix] = v,
                            None => {
                                items.remove(ix);
                            }
                        }
                        return;
                    }
                    &mut items[ix]
                }
                Ok(ix) if ix == items.len() && items.len() < 100_000 => {
                    if last {
                        if let Some(v) = value {
                            items.push(v);
                        }
                        return;
                    }
                    items.push(Value::Object(Map::new()));
                    let Some(item) = items.last_mut() else {
                        return;
                    };
                    item
                }
                _ => return,
            },
            Value::Object(map) => {
                if last {
                    match value {
                        Some(v) => {
                            map.insert(token.clone(), v);
                        }
                        None => {
                            map.remove(token);
                        }
                    }
                    return;
                }
                map.entry(token.clone())
                    .or_insert_with(|| Value::Object(Map::new()))
            }
            _ => return,
        };
        node = next;
    }
}

/// `path` made absolute against the template item `scope`.
fn absolute(scope: &str, path: &str) -> String {
    if path.starts_with('/') {
        path.to_owned()
    } else if path.is_empty() {
        if scope.is_empty() {
            "/".into()
        } else {
            scope.into()
        }
    } else {
        format!("{scope}/{path}")
    }
}

/// Walks one surface, collecting fields and actions.
struct Walk<'a> {
    surface: &'a Surface,
    surface_ix: usize,
    interactive: bool,
    budget: Budget,
    notes: BTreeSet<String>,
    fields: Vec<FormField>,
    bindings: Vec<Binding>,
    actions: Vec<FormAction>,
    buttons: Vec<Button>,
    stack: Vec<String>,
    drawn: usize,
    inert_buttons: bool,
}

impl Walk<'_> {
    /// The value `v` stands for: a literal, or what its path holds.
    fn value(&mut self, v: Option<&Value>, scope: &str) -> Option<Value> {
        match v? {
            Value::Object(map) if map.contains_key("path") => {
                let path = map.get("path").and_then(Value::as_str)?;
                pointer(&self.surface.data, &absolute(scope, path)).cloned()
            }
            Value::Object(map) if map.contains_key("call") => {
                self.notes.insert(
                    "function calls (\"call\") are not evaluated; they read as empty".into(),
                );
                None
            }
            other => Some(other.clone()),
        }
    }

    /// The value as text (numbers and booleans written out).
    fn text(&mut self, v: Option<&Value>, scope: &str) -> String {
        match self.value(v, scope) {
            Some(Value::String(s)) => clip(&s),
            Some(Value::Null) | None => String::new(),
            Some(other) => clip(&other.to_string()),
        }
    }

    /// The absolute path `v` binds to, if it is a binding.
    fn path_of(v: Option<&Value>, scope: &str) -> Option<String> {
        v?.as_object()?
            .get("path")
            .and_then(Value::as_str)
            .map(|p| absolute(scope, p))
    }

    fn children(&mut self, v: Option<&Value>, scope: &str, depth: usize) -> Vec<CustomNode> {
        match v {
            Some(Value::Array(ids)) => ids
                .iter()
                .filter_map(Value::as_str)
                .filter_map(|id| self.node(id, scope, depth + 1))
                .collect(),
            Some(Value::String(id)) => self.node(id, scope, depth + 1).into_iter().collect(),
            Some(Value::Object(template)) => {
                let (Some(component), Some(path)) = (
                    template.get("componentId").and_then(Value::as_str),
                    template.get("path").and_then(Value::as_str),
                ) else {
                    self.notes
                        .insert("a template without componentId or path was skipped".into());
                    return Vec::new();
                };
                let base = absolute(scope, path);
                let keys: Vec<String> = match pointer(&self.surface.data, &base) {
                    Some(Value::Array(items)) => (0..items.len()).map(|i| i.to_string()).collect(),
                    Some(Value::Object(map)) => map.keys().map(|k| escape(k)).collect(),
                    _ => Vec::new(),
                };
                if keys.len() > MAX_ITEMS {
                    self.notes.insert(format!(
                        "a list bound to {base} shows its first {MAX_ITEMS} of {} items",
                        keys.len()
                    ));
                }
                let base = base.trim_end_matches('/').to_owned();
                keys.iter()
                    .take(MAX_ITEMS)
                    .filter_map(|key| self.node(component, &format!("{base}/{key}"), depth + 1))
                    .collect()
            }
            _ => Vec::new(),
        }
    }

    fn one(&mut self, id: Option<&Value>, scope: &str, depth: usize) -> Option<CustomNode> {
        id.and_then(Value::as_str)
            .and_then(|id| self.node(id, scope, depth + 1))
    }

    /// The node of component `id`; `None` when it became a field or an
    /// action, or was cut.
    fn node(&mut self, id: &str, scope: &str, depth: usize) -> Option<CustomNode> {
        if depth > MAX_DEPTH {
            self.notes.insert(format!(
                "components nested deeper than {MAX_DEPTH} were cut"
            ));
            return Some(CustomNode::Text {
                text: "… (nested too deep)".into(),
            });
        }
        if !self.budget.take() {
            return None;
        }
        let surface = self.surface;
        let Some(c) = surface.components.get(id) else {
            self.notes
                .insert(format!("component {id:?} is referenced but never defined"));
            return Some(CustomNode::Text {
                text: format!("[missing component {id:?}]"),
            });
        };
        if self.stack.iter().any(|s| s == id) {
            self.notes.insert(format!(
                "component {id:?} contains itself; the loop was cut"
            ));
            return Some(CustomNode::Text {
                text: format!("[loop at {id:?}]"),
            });
        }
        self.stack.push(id.to_owned());
        self.drawn += 1;
        let node = self.component(id, c, scope, depth);
        self.stack.pop();
        node
    }

    fn component(&mut self, id: &str, c: &Value, scope: &str, depth: usize) -> Option<CustomNode> {
        let kind = c
            .get("component")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let prop = |key: &str| c.get(key);
        Some(match kind {
            "Text" => {
                let text = self.text(prop("text"), scope);
                let variant = prop("variant").and_then(Value::as_str).unwrap_or_default();
                let level = match variant {
                    "h1" => 1,
                    "h2" => 2,
                    "h3" => 3,
                    "h4" => 4,
                    "h5" => 5,
                    _ => 0,
                };
                if level > 0 {
                    let line = text.lines().next().unwrap_or_default();
                    CustomNode::Markdown {
                        source: format!("{} {line}", "#".repeat(level)),
                    }
                } else if variant == "caption" {
                    CustomNode::Text { text }
                } else {
                    CustomNode::Markdown { source: text }
                }
            }
            "Image" => {
                let url = self.text(prop("url"), scope);
                let alt = self.text(prop("altText").or(prop("description")), scope);
                image(&url, Some(alt.as_str()).filter(|a| !a.is_empty()))
            }
            "Icon" => CustomNode::Text {
                text: format!("[{}]", self.text(prop("name"), scope)),
            },
            "Video" | "AudioPlayer" => {
                let url = self.text(prop("url"), scope);
                let what = if kind == "Video" { "Video" } else { "Audio" };
                let about = self.text(prop("description"), scope);
                link(
                    &format!("{what}: {}", if about.is_empty() { &url } else { &about }),
                    &url,
                )
            }
            "Row" => CustomNode::Row {
                children: self.children(prop("children"), scope, depth),
            },
            "Column" => CustomNode::Col {
                children: self.children(prop("children"), scope, depth),
            },
            "List" => {
                let children = self.children(prop("children"), scope, depth);
                if prop("direction").and_then(Value::as_str) == Some("horizontal") {
                    CustomNode::Row { children }
                } else {
                    CustomNode::Col { children }
                }
            }
            "Card" => CustomNode::Card {
                title: None,
                children: self.one(prop("child"), scope, depth).into_iter().collect(),
            },
            "Tabs" => {
                let tabs = prop("tabs")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let mut children = Vec::new();
                for tab in &tabs {
                    let title = self.text(tab.get("title"), scope);
                    let child = self.one(tab.get("child"), scope, depth);
                    children.push(CustomNode::Card {
                        title: Some(if title.is_empty() {
                            "Tab".into()
                        } else {
                            title
                        }),
                        children: child.into_iter().collect(),
                    });
                }
                CustomNode::Col { children }
            }
            "Modal" => {
                let mut children: Vec<CustomNode> = self
                    .one(prop("trigger"), scope, depth)
                    .into_iter()
                    .collect();
                children.push(CustomNode::Card {
                    title: Some("Shown in a dialog".into()),
                    children: self
                        .one(prop("content"), scope, depth)
                        .into_iter()
                        .collect(),
                });
                CustomNode::Col { children }
            }
            "Divider" => CustomNode::Markdown {
                source: "---".into(),
            },
            "Button" => return self.button(id, c, scope, depth),
            "TextField" | "CheckBox" | "ChoicePicker" | "Slider" | "DateTimeInput" => {
                return self.input(id, kind, c, scope);
            }
            "" => placeholder(
                format!("A2UI component {id:?} without a type"),
                Some("`component` must name a catalog component".into()),
                c,
            ),
            other => placeholder(
                format!("A2UI component {other:?}"),
                Some("not in crest's basic catalog".into()),
                c,
            ),
        })
    }

    fn button(&mut self, id: &str, c: &Value, scope: &str, depth: usize) -> Option<CustomNode> {
        let label = self
            .one(c.get("child"), scope, depth)
            .map(|n| plain_text(&n).trim().to_owned())
            .filter(|l| !l.is_empty())
            .or_else(|| c.get("label").and_then(Value::as_str).map(str::to_owned))
            .unwrap_or_else(|| "Button".into());
        let label = clip(
            label
                .lines()
                .next()
                .unwrap_or("Button")
                .trim_start_matches('#')
                .trim(),
        );
        let primary = c.get("variant").and_then(Value::as_str) == Some("primary");
        let action = c.get("action");
        if let Some(event) = action.and_then(|a| a.get("event")) {
            let name = event
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if self.interactive && self.actions.len() < domain::form::MAX_ACTIONS {
                let action_id = format!("{}:{id}", self.actions.len() + 1);
                let context = event
                    .get("context")
                    .and_then(Value::as_object)
                    .map(|m| {
                        m.iter()
                            .map(|(k, v)| {
                                let v = match Self::path_of(Some(v), scope) {
                                    Some(path) => json!({ "path": path }),
                                    None => v.clone(),
                                };
                                (k.clone(), v)
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                self.buttons.push(Button {
                    action: action_id.clone(),
                    surface: self.surface_ix,
                    component: id.to_owned(),
                    name: name.to_owned(),
                    context,
                });
                self.actions.push(FormAction {
                    id: action_id,
                    label,
                    primary,
                });
                return None;
            }
            self.inert_buttons = true;
            return Some(CustomNode::Text {
                text: format!("[button: {label}]"),
            });
        }
        if let Some(call) = action.and_then(|a| a.get("functionCall")) {
            let function = call.get("call").and_then(Value::as_str).unwrap_or_default();
            if function == "openUrl" {
                let url = self.text(call.get("args").and_then(|a| a.get("url")), scope);
                return Some(link(&label, &url));
            }
            self.notes.insert(format!(
                "client function {function:?} of a button is not run"
            ));
        }
        Some(CustomNode::Text {
            text: format!("[button: {label}]"),
        })
    }

    fn input(&mut self, id: &str, kind: &str, c: &Value, scope: &str) -> Option<CustomNode> {
        let mut label = self.text(c.get("label"), scope);
        if label.is_empty() {
            label = id.to_owned();
        }
        let path = Self::path_of(c.get("value"), scope);
        let current = self.value(c.get("value"), scope);
        let required = c
            .get("checks")
            .and_then(Value::as_array)
            .is_some_and(|checks| {
                checks
                    .iter()
                    .any(|check| check.get("call").and_then(Value::as_str) == Some("required"))
            });
        let variant = c.get("variant").and_then(Value::as_str).unwrap_or_default();
        let text_of = |v: &Option<Value>| match v {
            Some(Value::String(s)) => Some(s.clone()),
            Some(Value::Number(n)) => Some(n.to_string()),
            _ => None,
        };
        let mut description = None;
        let (field_kind, bind) = match kind {
            "TextField" => match variant {
                "longText" => (
                    FieldKind::Multiline {
                        default: text_of(&current),
                        max_length: None,
                    },
                    Bind::Plain,
                ),
                "number" => (
                    FieldKind::Number {
                        default: current.as_ref().and_then(Value::as_f64),
                        min: None,
                        max: None,
                    },
                    Bind::Plain,
                ),
                "date" => {
                    description = Some("Date as YYYY-MM-DD".to_owned());
                    (
                        FieldKind::String {
                            default: text_of(&current),
                            min_length: None,
                            max_length: None,
                        },
                        Bind::Date,
                    )
                }
                other => {
                    if other == "obscured" {
                        description = Some("crest does not hide what you type here".to_owned());
                    }
                    (
                        FieldKind::String {
                            default: text_of(&current),
                            min_length: None,
                            max_length: None,
                        },
                        Bind::Plain,
                    )
                }
            },
            "CheckBox" => (
                FieldKind::Boolean {
                    default: current.as_ref().and_then(Value::as_bool),
                },
                Bind::Plain,
            ),
            "Slider" => {
                let number =
                    |key: &str| c.get(key).and_then(Value::as_f64).filter(|v| v.is_finite());
                let (min, max) = match (number("min"), number("max")) {
                    (Some(lo), Some(hi)) if lo > hi => (None, None),
                    bounds => bounds,
                };
                let default = current
                    .as_ref()
                    .and_then(Value::as_f64)
                    .filter(|v| min.is_none_or(|lo| *v >= lo) && max.is_none_or(|hi| *v <= hi));
                (FieldKind::Number { default, min, max }, Bind::Plain)
            }
            "DateTimeInput" => {
                let date = c.get("enableDate").and_then(Value::as_bool);
                let time = c
                    .get("enableTime")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let date = date.unwrap_or(!time);
                let (hint, bind) = match (date, time) {
                    (true, true) => ("Date and time as YYYY-MM-DDTHH:MM", Bind::DateTime),
                    (false, true) => ("Time as HH:MM", Bind::Time),
                    _ => ("Date as YYYY-MM-DD", Bind::Date),
                };
                description = Some(hint.to_owned());
                (
                    FieldKind::String {
                        default: text_of(&current),
                        min_length: None,
                        max_length: None,
                    },
                    bind,
                )
            }
            _ => {
                let options = self
                    .value(c.get("options"), scope)
                    .and_then(|v| v.as_array().cloned())
                    .unwrap_or_default();
                let mut seen = BTreeSet::new();
                let options: Vec<FormOption> = options
                    .iter()
                    .take(MAX_OPTIONS)
                    .filter_map(|o| {
                        let value = match o.get("value")? {
                            Value::String(s) => s.clone(),
                            Value::Number(n) => n.to_string(),
                            _ => return None,
                        };
                        let label = self.text(o.get("label"), scope);
                        seen.insert(value.clone()).then_some(FormOption {
                            value,
                            label: Some(label).filter(|l| !l.is_empty()),
                        })
                    })
                    .collect();
                if options.is_empty() {
                    self.notes
                        .insert(format!("choice {label:?} has no options"));
                    return Some(CustomNode::Text {
                        text: format!("{label}: (no options)"),
                    });
                }
                let picked: Vec<String> = match &current {
                    Some(Value::Array(items)) => items
                        .iter()
                        .filter_map(|v| v.as_str().map(str::to_owned))
                        .filter(|v| options.iter().any(|o| &o.value == v))
                        .collect(),
                    Some(Value::String(s)) if options.iter().any(|o| &o.value == s) => {
                        vec![s.clone()]
                    }
                    _ => Vec::new(),
                };
                if variant == "mutuallyExclusive" {
                    (
                        FieldKind::Enum {
                            options,
                            default: picked.into_iter().next(),
                        },
                        Bind::OneOfList,
                    )
                } else {
                    (
                        FieldKind::MultiEnum {
                            options,
                            default: picked,
                            min_items: None,
                            max_items: None,
                        },
                        Bind::Plain,
                    )
                }
            }
        };
        if !self.interactive || self.fields.len() >= MAX_FIELDS {
            if self.interactive {
                self.notes
                    .insert(format!("only {MAX_FIELDS} inputs become form fields"));
            }
            let shown = match &current {
                Some(Value::String(s)) if !s.is_empty() => clip(s),
                Some(Value::Array(items)) if !items.is_empty() => items
                    .iter()
                    .map(|v| v.as_str().map_or_else(|| v.to_string(), str::to_owned))
                    .collect::<Vec<_>>()
                    .join(", "),
                Some(Value::Bool(b)) => if *b { "yes" } else { "no" }.to_owned(),
                Some(Value::Number(n)) => n.to_string(),
                _ => "—".to_owned(),
            };
            return Some(CustomNode::Text {
                text: format!("{label}: {shown}"),
            });
        }
        let name = unique_name(
            &path.as_deref().map_or_else(
                || id.to_owned(),
                |p| p.trim_start_matches('/').replace('/', "."),
            ),
            &self.fields,
        );
        self.bindings.push(Binding {
            field: name.clone(),
            surface: self.surface_ix,
            path,
            bind,
        });
        self.fields.push(FormField {
            name,
            label: Some(label),
            description,
            required,
            kind: field_kind,
        });
        None
    }
}

/// Draws `a2ui`. Interactive, inputs become fields and buttons with an
/// event become actions; otherwise every component is drawn read-only.
///
/// # Errors
///
/// A [`CardError`] when no surface has its root component.
pub fn render(a2ui: &A2ui, interactive: bool) -> Result<Rendered, CardError> {
    let mut notes: BTreeSet<String> = a2ui.notes.iter().cloned().collect();
    let mut fields = Vec::new();
    let mut bindings = Vec::new();
    let mut actions = Vec::new();
    let mut buttons = Vec::new();
    let mut trees = Vec::new();
    let mut drawn = 0;
    let mut budget = Budget::default();
    let mut inert = false;
    for (ix, surface) in a2ui.surfaces.iter().enumerate() {
        if let Some(catalog) = &surface.catalog
            && !catalog.contains("basic")
            && !catalog.contains("standard")
        {
            notes.insert(format!(
                "catalog {catalog:?} is not the basic catalog; components outside it are \
                 shown as placeholders"
            ));
        }
        if !surface.components.contains_key(&surface.root) {
            if !surface.components.is_empty() {
                notes.insert(format!(
                    "surface {:?} has no root component {:?}",
                    surface.id, surface.root
                ));
            }
            continue;
        }
        let mut walk = Walk {
            surface,
            surface_ix: ix,
            interactive,
            budget,
            notes: BTreeSet::new(),
            fields,
            bindings,
            actions,
            buttons,
            stack: Vec::new(),
            drawn: 0,
            inert_buttons: false,
        };
        let root = surface.root.clone();
        let tree = walk.node(&root, "", 0);
        if walk.budget.exhausted {
            walk.notes
                .insert(format!("only the first {MAX_NODES} components were drawn"));
        }
        let unused = surface.components.len().saturating_sub(walk.drawn);
        if unused > 0 && !walk.budget.exhausted {
            walk.notes.insert(format!(
                "{unused} component(s) of surface {:?} are not under its root",
                surface.id
            ));
        }
        notes.extend(walk.notes);
        drawn += walk.drawn;
        inert |= walk.inert_buttons;
        budget = walk.budget;
        fields = walk.fields;
        bindings = walk.bindings;
        actions = walk.actions;
        buttons = walk.buttons;
        trees.push((surface.id.clone(), tree));
    }
    if trees.is_empty() {
        return Err(CardError::new(
            "no A2UI surface has its root component (the component with id \"root\")",
        ));
    }
    if inert {
        notes.insert("the buttons do nothing here: this card cannot be answered".into());
    }
    let body = if trees.len() == 1 {
        trees
            .pop()
            .and_then(|(_, tree)| tree)
            .unwrap_or(CustomNode::Col {
                children: Vec::new(),
            })
    } else {
        CustomNode::Col {
            children: trees
                .into_iter()
                .map(|(id, tree)| CustomNode::Card {
                    title: Some(id),
                    children: tree.into_iter().collect(),
                })
                .collect(),
        }
    };
    let card = Card {
        format: CardFormat::A2ui,
        title: None,
        body,
        fields,
        actions,
        notes: notes.into_iter().collect(),
        components: drawn,
    };
    Ok(Rendered {
        card,
        bindings,
        buttons,
        surfaces: a2ui
            .surfaces
            .iter()
            .map(|s| (s.id.clone(), s.data.clone(), s.send_data_model))
            .collect(),
    })
}

impl Rendered {
    /// Checks the date and time inputs of `answer`.
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
            let Some(Value::String(text)) = answer.get(&binding.field) else {
                continue;
            };
            let ok = match binding.bind {
                Bind::Date => is_date(text),
                Bind::Time => is_time(text),
                Bind::DateTime => text
                    .split_once('T')
                    .is_some_and(|(d, t)| is_date(d) && is_time(t)),
                Bind::Plain | Bind::OneOfList => true,
            };
            if !ok {
                return Err(format!("{:?} is not a valid date or time", binding.field));
            }
        }
        Ok(())
    }

    /// The v0.9 `action` message for `answer` submitted with `action`:
    /// the answers written into the data model at their paths, the
    /// event's context resolved against it, and the data model itself when
    /// the surface asked for it (`sendDataModel`).
    #[must_use]
    pub fn response(&self, answer: &BTreeMap<String, Value>, action: Option<&str>) -> Value {
        let mut data: Vec<Value> = self.surfaces.iter().map(|(_, d, _)| d.clone()).collect();
        for binding in &self.bindings {
            let (Some(path), Some(value)) = (&binding.path, answer.get(&binding.field)) else {
                continue;
            };
            let value = match (binding.bind, value) {
                (Bind::OneOfList, Value::String(s)) => json!([s]),
                (_, v) => v.clone(),
            };
            if let Some(model) = data.get_mut(binding.surface) {
                set_pointer(model, path, Some(value));
            }
        }
        let button = action.and_then(|id| self.buttons.iter().find(|b| b.action == id));
        let Some(button) = button else {
            return json!({ "version": VERSION, "values": answer });
        };
        let model = data.get(button.surface).cloned().unwrap_or(Value::Null);
        let context: Map<String, Value> = button
            .context
            .iter()
            .map(|(k, v)| {
                let v = match v.get("path").and_then(Value::as_str) {
                    Some(path) => pointer(&model, path).cloned().unwrap_or(Value::Null),
                    None => v.clone(),
                };
                (k.clone(), v)
            })
            .collect();
        let Some((surface_id, _, send)) = self.surfaces.get(button.surface) else {
            return json!({ "version": VERSION, "values": answer });
        };
        let mut message = json!({
            "version": VERSION,
            "action": {
                "name": button.name,
                "surfaceId": surface_id,
                "sourceComponentId": button.component,
                "context": context,
            }
        });
        if *send {
            message["dataModel"] = json!({ surface_id.clone(): model });
        }
        message
    }
}

/// Parses and draws `payload` in one go.
///
/// # Errors
///
/// The [`CardError`] of [`parse`] or [`render`].
pub fn map(payload: &Value, interactive: bool) -> Result<Rendered, CardError> {
    render(&parse(payload)?, interactive)
}

#[cfg(test)]
mod tests;
