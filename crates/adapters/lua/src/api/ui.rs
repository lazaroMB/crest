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

use std::sync::Arc;

use application::ports::plugin::PluginOutput;
use domain::block::{BlockKind, CustomNode};
use mlua::{Lua, Table, Value};
use serde_json::{Value as Json, json};

use crate::config::api::Origin;
use crate::convert;

/// Key of the table `app.ui.block` returns; the VM turns a handler result
/// that is exactly `{ [BLOCK_KEY] = block }` into [`PluginOutput::Block`].
pub(crate) const BLOCK_KEY: &str = "$crest_block";

fn error(function: &str, message: impl std::fmt::Display) -> mlua::Error {
    mlua::Error::runtime(format!("app.ui.{function}: {message}"))
}

/// Reads one node argument: a string or number is a text node, a table must
/// be a node built by `app.ui`.
fn node(lua: &Lua, function: &str, value: Value) -> mlua::Result<CustomNode> {
    match value {
        Value::String(s) => Ok(CustomNode::Text {
            text: s.to_str()?.to_owned(),
        }),
        Value::Integer(n) => Ok(CustomNode::Text {
            text: n.to_string(),
        }),
        Value::Number(n) => Ok(CustomNode::Text {
            text: n.to_string(),
        }),
        Value::Table(_) => {
            let json = convert::to_json(lua, value)?;
            serde_json::from_value(json).map_err(|e| error(function, format!("not a ui node: {e}")))
        }
        other => Err(error(
            function,
            format!("expected a ui node or a string, got {}", other.type_name()),
        )),
    }
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
        "code",
        lua.create_function(|lua, (lang, source): (Option<String>, String)| {
            to_lua(lua, &CustomNode::Code { lang, source })
        })?,
    )?;
    ui.set(
        "markdown",
        lua.create_function(|lua, source: String| to_lua(lua, &CustomNode::Markdown { source }))?,
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
