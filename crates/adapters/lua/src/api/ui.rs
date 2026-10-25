//! `app.ui`: declarative blocks a plugin shows in the timeline.
//!
//! ```lua
//! handler = function(input)
//!   return app.ui.block("todo", app.ui.col({
//!     app.ui.text("Pendientes"),
//!     app.ui.list(input.items),
//!     app.ui.table({ columns = { "tarea", "hecha" }, rows = { { "parser", true } } }),
//!   }))
//! end
//! ```
//!
//! Lua never touches GPUI: each constructor returns a plain table shaped
//! like a [`CustomNode`] (`{ type = "text", text = "..." }`) and the UI
//! draws it. Strings (and numbers) where a node is expected become text
//! nodes. `app.ui.block(name, node)` validates the whole tree and wraps it
//! as a `Custom` block of the calling plugin; a tool handler that returns
//! it shows the block to the user. Bad arguments raise a Lua error, which
//! fails the tool call with that message.
//!
//! | Constructor | Node |
//! | --- | --- |
//! | `text(s)` | text |
//! | `row(children)`, `col(children)` | horizontal / vertical layout |
//! | `list(items)` | bulleted list |
//! | `table{ columns, rows }` | table; a column is a name or `{ name, align }` |
//! | `chart{ title, kind, labels, series }` | chart; `kind` defaults to `"bar"` |
//! | `code(lang, src)` | code snippet; `lang` may be `nil` |
//! | `markdown(src)` | rendered markdown |
//! | `image{ path \| data, media_type, alt, fit }` or `image(path)` | picture with zoom and pan |
//! | `html(src)` | basic HTML subset |
//! | `json(value, { expand = n })` | collapsible JSON tree |
//! | `mermaid(src)` | Mermaid diagram |
//! | `link(text, url)` or `link(url)` | link opened in the browser |
//! | `diff(unified)` | colored unified diff |
//! | `csv(text, { header, delimiter })` | the CSV as a table node |
//! | `graph{ title, directed, layout, direction, seed, nodes, edges, groups }` | nodes and edges laid out in layers, by forces or on a grid (pan, zoom, click to focus) |
//! | `filter_list{ title, columns, items, facets, follow }` | records with a text or `/regex/` filter and chips |
//! | `tree{ title, columns, nodes, expand_depth }` | nested rows with typed columns (`text`, `number`, `duration`, `bar`), badges and inline bars |
//! | `timeline{ title, unit, rows, range }` | rows of spans on a time axis (zoom, pan, tooltip); a span's end is `stop` (or `["end"]`) |

use std::sync::Arc;

use application::ports::plugin::PluginOutput;
use domain::block::{BlockKind, CustomNode};
use mlua::{Lua, Table, Value};
use serde_json::{Value as Json, json};

use super::csv::{self, CsvOptions};
use crate::config::api::Origin;
use crate::convert;

/// Key of the table `app.ui.block` returns; the VM turns a handler result
/// that is exactly `{ [BLOCK_KEY] = block }` into [`PluginOutput::Block`].
pub(crate) const BLOCK_KEY: &str = "$crest_block";

fn error(function: &str, message: impl std::fmt::Display) -> mlua::Error {
    mlua::Error::runtime(format!("app.ui.{function}: {message}"))
}

/// Reads one node: a string or number is a text node, a table must be a
/// node built by `app.ui`. Errors are the bare reason, for the caller to
/// prefix.
pub(crate) fn parse_node(lua: &Lua, value: Value) -> Result<CustomNode, String> {
    match value {
        Value::String(s) => Ok(CustomNode::Text {
            text: s.to_str().map_err(|e| e.to_string())?.to_owned(),
        }),
        Value::Integer(n) => Ok(CustomNode::Text {
            text: n.to_string(),
        }),
        Value::Number(n) => Ok(CustomNode::Text {
            text: n.to_string(),
        }),
        Value::Table(_) => {
            let json = convert::to_json(lua, value).map_err(|e| e.to_string())?;
            serde_json::from_value(json).map_err(|e| format!("not a ui node: {e}"))
        }
        other => Err(format!(
            "expected a ui node or a string, got {}",
            other.type_name()
        )),
    }
}

/// Reads one node argument of `app.ui.<function>`.
fn node(lua: &Lua, function: &str, value: Value) -> mlua::Result<CustomNode> {
    parse_node(lua, value).map_err(|e| error(function, e))
}

/// Reads a list of node arguments (a sequence table).
fn nodes(lua: &Lua, function: &str, value: Value) -> mlua::Result<Vec<CustomNode>> {
    let Value::Table(list) = value else {
        return Err(error(
            function,
            format!("expected a list, got {}", value.type_name()),
        ));
    };
    list.sequence_values::<Value>()
        .enumerate()
        .map(|(ix, item)| {
            node(lua, function, item?).map_err(|e| error(function, format!("item {}: {e}", ix + 1)))
        })
        .collect()
}

/// Converts `node` back into the Lua table plugins pass around.
fn to_lua(lua: &Lua, node: &CustomNode) -> mlua::Result<Value> {
    let json = serde_json::to_value(node).map_err(mlua::Error::external)?;
    convert::to_lua(lua, &json)
}

/// Parses a spec-shaped JSON value into a node and validates it.
fn spec_node(function: &str, json: Json) -> mlua::Result<CustomNode> {
    let node: CustomNode = serde_json::from_value(json).map_err(|e| error(function, e))?;
    node.validate().map_err(|e| error(function, e))?;
    Ok(node)
}

fn table_node(lua: &Lua, def: Table) -> mlua::Result<CustomNode> {
    let columns: Vec<Json> = match def.get::<Value>("columns")? {
        Value::Table(cols) => cols
            .sequence_values::<Value>()
            .map(|col| match col? {
                Value::String(name) => Ok(json!({ "name": name.to_str()?.to_owned() })),
                other => convert::to_json(lua, other),
            })
            .collect::<mlua::Result<_>>()?,
        _ => return Err(error("table", "`columns` must be a list")),
    };
    let rows = match def.get::<Value>("rows")? {
        Value::Nil => Vec::new(),
        Value::Table(rows) => rows
            .sequence_values::<Value>()
            .map(|row| match row? {
                Value::Table(cells) => cells
                    .sequence_values::<Value>()
                    .map(|cell| convert::to_json(lua, cell?))
                    .collect::<mlua::Result<Vec<_>>>()
                    .map(Json::Array),
                other => Err(error(
                    "table",
                    format!("each row must be a list, got {}", other.type_name()),
                )),
            })
            .collect::<mlua::Result<Vec<_>>>()?,
        _ => return Err(error("table", "`rows` must be a list")),
    };
    spec_node(
        "table",
        json!({ "type": "table", "columns": columns, "rows": rows }),
    )
}

fn chart_node(lua: &Lua, def: Table) -> mlua::Result<CustomNode> {
    let field = |name: &str| -> mlua::Result<Json> { convert::to_json(lua, def.get(name)?) };
    let or = |value: Json, default: Json| if value.is_null() { default } else { value };
    spec_node(
        "chart",
        json!({
            "type": "chart",
            "title": or(field("title")?, json!("")),
            "kind": or(field("kind")?, json!("bar")),
            "labels": or(field("labels")?, json!([])),
            "series": or(field("series")?, json!([])),
        }),
    )
}

/// `app.ui.plot{...}`: the fields of a `show_plot` call (functions,
/// params, series, ranges, labels) as a plot node with sliders.
fn plot_node(lua: &Lua, def: Table) -> mlua::Result<CustomNode> {
    let mut json = convert::to_json(lua, Value::Table(def))?;
    let Some(fields) = json.as_object_mut() else {
        return Err(error("plot", "expected a table of plot fields"));
    };
    fields.insert("type".into(), json!("plot"));
    spec_node("plot", json)
}

/// Fields of the widget specs that hold lists. An empty Lua table has no
/// way to say it is a list and converts to `{}`; under these keys it is
/// read as `[]`. Maps (`attrs`, `fields`, `values`) keep `{}`.
const LIST_FIELDS: [&str; 12] = [
    "rows", "spans", "markers", "items", "columns", "facets", "nodes", "edges", "groups", "roots",
    "children", "badges",
];

/// Turns `{}` into `[]` under [`LIST_FIELDS`], at any depth.
fn empty_lists(json: &mut Json) {
    match json {
        Json::Object(map) => {
            for (key, value) in map.iter_mut() {
                if LIST_FIELDS.contains(&key.as_str())
                    && value.as_object().is_some_and(serde_json::Map::is_empty)
                {
                    *value = Json::Array(Vec::new());
                } else {
                    empty_lists(value);
                }
            }
        }
        Json::Array(items) => items.iter_mut().for_each(empty_lists),
        _ => {}
    }
}

/// `app.ui.<function>{...}`: a table shaped like the widget spec of node
/// type `kind` (see [`CustomNode`]), validated.
fn widget_node(lua: &Lua, function: &str, kind: &str, def: Table) -> mlua::Result<CustomNode> {
    let mut json = convert::to_json(lua, Value::Table(def))?;
    empty_lists(&mut json);
    let Some(fields) = json.as_object_mut() else {
        return Err(error(function, "expected a table of fields"));
    };
    fields.insert("type".into(), json!(kind));
    spec_node(function, json)
}

fn image_node(lua: &Lua, def: Value) -> mlua::Result<CustomNode> {
    let def = match def {
        Value::String(path) => {
            let path = path.to_str()?.to_owned();
            return spec_node(
                "image",
                json!({ "type": "image", "source": { "type": "path", "path": path } }),
            );
        }
        Value::Table(def) => def,
        other => {
            return Err(error(
                "image",
                format!("expected a path or a table, got {}", other.type_name()),
            ));
        }
    };
    let field = |name: &str| -> mlua::Result<Json> { convert::to_json(lua, def.get(name)?) };
    let source = match (field("path")?, field("data")?) {
        (Json::String(path), Json::Null) => json!({ "type": "path", "path": path }),
        (Json::Null, Json::String(data)) => json!({ "type": "base64", "data": data }),
        (Json::Null, Json::Null) => {
            return Err(error("image", "give `path` (a file) or `data` (base64)"));
        }
        (Json::String(_), Json::String(_)) => {
            return Err(error("image", "give either `path` or `data`, not both"));
        }
        _ => return Err(error("image", "`path` and `data` must be strings")),
    };
    let mut node = json!({ "type": "image", "source": source });
    for key in ["media_type", "alt", "fit"] {
        let value = field(key)?;
        if !value.is_null() {
            node[key] = value;
        }
    }
    spec_node("image", node)
}

fn json_node(lua: &Lua, value: Value, opts: Option<Table>) -> mlua::Result<CustomNode> {
    let value = convert::to_json(lua, value)?;
    let expand_depth = match opts {
        Some(opts) => match opts.get::<Option<f64>>("expand")? {
            Some(n) if n >= 0.0 && n.fract() == 0.0 => n as u32,
            Some(n) => {
                return Err(error(
                    "json",
                    format!("`expand` must be a whole number >= 0, got {n}"),
                ));
            }
            None => 1,
        },
        None => 1,
    };
    Ok(CustomNode::Json {
        value,
        expand_depth,
    })
}

fn link_node(text: String, url: Option<String>) -> mlua::Result<CustomNode> {
    let (text, url) = match url {
        Some(url) => (text, url),
        None => (text.clone(), text),
    };
    let node = CustomNode::Link { text, url };
    node.validate().map_err(|e| error("link", e))?;
    Ok(node)
}

fn csv_node(text: &str, opts: Option<Table>) -> mlua::Result<CustomNode> {
    let mut options = CsvOptions::default();
    if let Some(opts) = opts {
        if let Some(header) = opts.get::<Option<bool>>("header")? {
            options.header = header;
        }
        if let Some(delimiter) = opts.get::<Option<String>>("delimiter")? {
            let mut chars = delimiter.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) if c != '"' && c != '\n' && c != '\r' => options.delimiter = c,
                _ => {
                    return Err(error(
                        "csv",
                        format!("`delimiter` must be one character, got {delimiter:?}"),
                    ));
                }
            }
        }
    }
    let spec = csv::parse(text, options).map_err(|e| error("csv", e))?;
    Ok(CustomNode::Table(spec))
}

/// Builds the `app.ui` table of one phase; blocks are attributed to the
/// phase's plugin.
pub(crate) fn table(lua: &Lua, origin: &Arc<Origin>) -> mlua::Result<Table> {
    let ui = lua.create_table()?;
    ui.set(
        "text",
        lua.create_function(|lua, value: Value| {
            let node = match value {
                Value::Table(_) => return Err(error("text", "expected a string, got table")),
                other => node(lua, "text", other)?,
            };
            to_lua(lua, &node)
        })?,
    )?;
    ui.set(
        "row",
        lua.create_function(|lua, children: Value| {
            let children = nodes(lua, "row", children)?;
            to_lua(lua, &CustomNode::Row { children })
        })?,
    )?;
    ui.set(
        "col",
        lua.create_function(|lua, children: Value| {
            let children = nodes(lua, "col", children)?;
            to_lua(lua, &CustomNode::Col { children })
        })?,
    )?;
    ui.set(
        "list",
        lua.create_function(|lua, items: Value| {
            let items = nodes(lua, "list", items)?;
            to_lua(lua, &CustomNode::List { items })
        })?,
    )?;
    ui.set(
        "table",
        lua.create_function(|lua, def: Table| to_lua(lua, &table_node(lua, def)?))?,
    )?;
    ui.set(
        "chart",
        lua.create_function(|lua, def: Table| to_lua(lua, &chart_node(lua, def)?))?,
    )?;
    ui.set(
        "plot",
        lua.create_function(|lua, def: Table| to_lua(lua, &plot_node(lua, def)?))?,
    )?;
    ui.set(
        "timeline",
        lua.create_function(|lua, def: Table| {
            to_lua(lua, &widget_node(lua, "timeline", "timeline", def)?)
        })?,
    )?;
    ui.set(
        "graph",
        lua.create_function(|lua, def: Table| {
            to_lua(lua, &widget_node(lua, "graph", "graph", def)?)
        })?,
    )?;
    ui.set(
        "tree",
        lua.create_function(|lua, def: Table| {
            to_lua(lua, &widget_node(lua, "tree", "tree", def)?)
        })?,
    )?;
    ui.set(
        "filter_list",
        lua.create_function(|lua, def: Table| {
            to_lua(lua, &widget_node(lua, "filter_list", "filter_list", def)?)
        })?,
    )?;
    ui.set(
        "code",
        lua.create_function(|lua, (lang, source): (Option<String>, String)| {
            to_lua(lua, &CustomNode::Code { lang, source })
        })?,
    )?;
    ui.set(
        "markdown",
        lua.create_function(|lua, source: String| to_lua(lua, &CustomNode::Markdown { source }))?,
    )?;
    ui.set(
        "image",
        lua.create_function(|lua, def: Value| to_lua(lua, &image_node(lua, def)?))?,
    )?;
    ui.set(
        "html",
        lua.create_function(|lua, source: String| to_lua(lua, &CustomNode::Html { source }))?,
    )?;
    ui.set(
        "json",
        lua.create_function(|lua, (value, opts): (Value, Option<Table>)| {
            to_lua(lua, &json_node(lua, value, opts)?)
        })?,
    )?;
    ui.set(
        "mermaid",
        lua.create_function(|lua, source: String| to_lua(lua, &CustomNode::Mermaid { source }))?,
    )?;
    ui.set(
        "link",
        lua.create_function(|lua, (text, url): (String, Option<String>)| {
            to_lua(lua, &link_node(text, url)?)
        })?,
    )?;
    ui.set(
        "diff",
        lua.create_function(|lua, unified: String| to_lua(lua, &CustomNode::Diff { unified }))?,
    )?;
    ui.set(
        "csv",
        lua.create_function(|lua, (text, opts): (String, Option<Table>)| {
            to_lua(lua, &csv_node(&text, opts)?)
        })?,
    )?;
    let plugin = origin.plugin.clone();
    ui.set(
        "block",
        lua.create_function(move |lua, (name, tree): (String, Value)| {
            let tree = node(lua, "block", tree)?;
            tree.validate().map_err(|e| error("block", e))?;
            let block = BlockKind::Custom {
                plugin: plugin.clone(),
                name,
                tree,
            };
            let json = serde_json::to_value(&block).map_err(mlua::Error::external)?;
            convert::to_lua(lua, &json!({ BLOCK_KEY: json }))
        })?,
    )?;
    Ok(ui)
}

/// What a Lua function returned: a block built by `app.ui.block`, or plain
/// data.
pub(crate) fn output(value: Json) -> PluginOutput {
    if let Json::Object(map) = &value
        && map.len() == 1
        && let Some(block) = map.get(BLOCK_KEY)
        && let Ok(block) = serde_json::from_value::<BlockKind>(block.clone())
    {
        return PluginOutput::Block(block);
    }
    PluginOutput::Value(value)
}

#[cfg(test)]
mod tests;
