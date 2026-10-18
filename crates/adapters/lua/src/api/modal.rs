//! `app.modal`: windows a plugin opens over crest's window.
//!
//! ```lua
//! local id = app.modal.open{
//!   id = "coverage",              -- optional; reopening an id replaces it
//!   title = "Coverage report",
//!   width = "60%", height = 80,   -- percent of the window; default 70 × 70
//!   position = "top-right",       -- center (default), top, bottom, left,
//!                                 -- right, top-left, top-right, ...
//!   content = app.ui.col{ app.ui.image{ path = "target/cov.png" }, "Done" },
//! }
//! app.modal.update(id, app.ui.markdown("# Updated"))
//! app.modal.close(id)
//! ```
//!
//! The calls push [`DomainEvent`]s to the front end at once (the sink never
//! blocks), from any plugin code: tool handlers, commands, keymaps, hooks.
//! Ids are scoped by plugin: the event carries `<plugin>/<id>`, so a plugin
//! cannot touch another's modals. Bad arguments raise a Lua error.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use application::ports::events::DomainEvent;
use domain::block::CustomNode;
use domain::modal::{ModalId, ModalPosition, ModalSpec, Percent};
use mlua::{Lua, Table, Value};

use super::ui::parse_node;
use crate::caps::context;
use crate::config::api::Origin;

/// Longest id a plugin may give.
const MAX_ID_LEN: usize = 128;

fn error(function: &str, message: impl std::fmt::Display) -> mlua::Error {
    mlua::Error::runtime(format!("app.modal.{function}: {message}"))
}

/// Sends `event` to the front end, or fails when there is none.
fn emit(lua: &Lua, function: &str, event: DomainEvent) -> mlua::Result<()> {
    let ctx = context(lua)?;
    let events = ctx
        .events
        .as_ref()
        .ok_or_else(|| error(function, "no window to show modals in"))?;
    events.0.emit(event);
    Ok(())
}

/// The plugin's id, checked.
fn plugin_id(function: &str, id: &str) -> mlua::Result<String> {
    let id = id.trim();
    if id.is_empty() || id.len() > MAX_ID_LEN || id.chars().any(char::is_control) {
        return Err(error(
            function,
            format!("`id` must be 1 to {MAX_ID_LEN} printable characters"),
        ));
    }
    Ok(id.to_owned())
}

/// The id of a modal as the front end knows it.
fn scoped(plugin: &str, id: &str) -> ModalId {
    ModalId::new(format!("{plugin}/{id}"))
}

/// Content of a modal, validated.
fn content(lua: &Lua, function: &str, value: Value) -> mlua::Result<CustomNode> {
    if value.is_nil() {
        return Err(error(function, "`content` is missing"));
    }
    let node = parse_node(lua, value).map_err(|e| error(function, format!("content: {e}")))?;
    node.validate()
        .map_err(|e| error(function, format!("content: {e}")))?;
    Ok(node)
}

/// A width or height: a number of percent or text such as `"60%"`.
fn percent(function: &str, field: &str, value: Value) -> mlua::Result<Percent> {
    let parsed = match value {
        Value::Nil => return Ok(Percent::default()),
        Value::Integer(n) => Percent::new(n as f32),
        Value::Number(n) => Percent::new(n as f32),
        Value::String(s) => s.to_str()?.parse(),
        other => {
            return Err(error(
                function,
                format!(
                    "`{field}` must be a number or \"NN%\", got {}",
                    other.type_name()
                ),
            ));
        }
    };
    parsed.map_err(|e| error(function, format!("`{field}`: {e}")))
}

fn open(lua: &Lua, origin: &Origin, def: Table) -> mlua::Result<String> {
    const F: &str = "open";
    let id = match def.get::<Value>("id")? {
        Value::Nil => {
            let n = context(lua)?.next_modal.fetch_add(1, Ordering::Relaxed);
            format!("modal-{n}")
        }
        Value::String(s) => plugin_id(F, &s.to_str()?)?,
        Value::Integer(n) => plugin_id(F, &n.to_string())?,
        other => {
            return Err(error(
                F,
                format!("`id` must be a string, got {}", other.type_name()),
            ));
        }
    };
    let title = match def.get::<Value>("title")? {
        Value::Nil => origin.plugin.clone(),
        Value::String(s) => s.to_str()?.to_owned(),
        other => {
            return Err(error(
                F,
                format!("`title` must be a string, got {}", other.type_name()),
            ));
        }
    };
    let position = match def.get::<Value>("position")? {
        Value::Nil => ModalPosition::default(),
        Value::String(s) => s
            .to_str()?
            .parse()
            .map_err(|e| error(F, format!("`position`: {e}")))?,
        other => {
            return Err(error(
                F,
                format!("`position` must be a string, got {}", other.type_name()),
            ));
        }
    };
    let spec = ModalSpec {
        id: scoped(&origin.plugin, &id),
        plugin: origin.plugin.clone(),
        title,
        width: percent(F, "width", def.get("width")?)?,
        height: percent(F, "height", def.get("height")?)?,
        position,
        content: content(lua, F, def.get("content")?)?,
    };
    emit(lua, F, DomainEvent::ModalOpened { spec })?;
    Ok(id)
}

/// Builds the `app.modal` table of one phase; its modals belong to the
/// phase's plugin.
pub(crate) fn table(lua: &Lua, origin: &Arc<Origin>) -> mlua::Result<Table> {
    let modal = lua.create_table()?;
    let o = Arc::clone(origin);
    modal.set(
        "open",
        lua.create_function(move |lua, def: Value| match def {
            Value::Table(def) => open(lua, &o, def),
            other => Err(error(
                "open",
                format!("expected a table, got {}", other.type_name()),
            )),
        })?,
    )?;
    let o = Arc::clone(origin);
    modal.set(
        "update",
        lua.create_function(move |lua, (id, node): (String, Value)| {
            let id = scoped(&o.plugin, &plugin_id("update", &id)?);
            let content = content(lua, "update", node)?;
            emit(lua, "update", DomainEvent::ModalUpdated { id, content })
        })?,
    )?;
    let o = Arc::clone(origin);
    modal.set(
        "close",
        lua.create_function(move |lua, id: String| {
            let id = scoped(&o.plugin, &plugin_id("close", &id)?);
            emit(lua, "close", DomainEvent::ModalClosed { id })
        })?,
    )?;
    Ok(modal)
}

#[cfg(test)]
mod tests {
    use application::testing::system::RecordingSink;
    use domain::modal::{HorizontalAnchor, VerticalAnchor};

    use super::*;
    use crate::callbacks::{self, CallbackIds};
    use crate::caps::Context;
    use crate::config::api::Phase;
    use crate::sandbox;

    fn run(code: &str, sink: Option<Arc<RecordingSink>>) -> mlua::Result<Option<String>> {
        let lua = sandbox::new_lua(None)?;
        callbacks::install(&lua, CallbackIds::default())?;
        let events = sink.map(|s| s as Arc<dyn application::ports::events::EventSink>);
        lua.set_app_data(Arc::new(
            Context::new(None, false, Some(Vec::new())).with_events(events),
        ));
        let phase = Phase::new(&lua, "init.lua", "cov")?;
        lua.load(code)
            .set_name("@init.lua")
            .set_environment(phase.env.clone())
            .eval()
    }

    fn events(code: &str) -> Vec<DomainEvent> {
        let sink = Arc::new(RecordingSink::new());
        run(code, Some(Arc::clone(&sink))).unwrap();
        sink.events()
    }

    fn error_of(code: &str) -> String {
        let sink = Arc::new(RecordingSink::new());
        let err = run(code, Some(Arc::clone(&sink))).unwrap_err().to_string();
        assert!(sink.events().is_empty(), "nothing is emitted on error");
        err
    }

    #[test]
    fn open_update_and_close_emit_scoped_events() {
        let got = events(
            r##"local id = app.modal.open{
                 id = "report", title = "Coverage", width = "60%", height = 80,
                 position = "top-right", content = app.ui.markdown("# hi"),
               }
               app.modal.update(id, "done")
               app.modal.close(id)
               return id"##,
        );
        assert_eq!(got.len(), 3);
        let DomainEvent::ModalOpened { spec } = &got[0] else {
            panic!("{got:?}")
        };
        assert_eq!(spec.id, ModalId::new("cov/report"));
        assert_eq!(spec.plugin, "cov");
        assert_eq!(spec.title, "Coverage");
        assert_eq!(spec.width.get(), 60.0);
        assert_eq!(spec.height.get(), 80.0);
        assert_eq!(spec.position.vertical, VerticalAnchor::Top);
        assert_eq!(spec.position.horizontal, HorizontalAnchor::Right);
        assert_eq!(
            spec.content,
            CustomNode::Markdown {
                source: "# hi".into()
            }
        );
        assert_eq!(
            got[1],
            DomainEvent::ModalUpdated {
                id: ModalId::new("cov/report"),
                content: CustomNode::Text {
                    text: "done".into()
                },
            }
        );
        assert_eq!(
            got[2],
            DomainEvent::ModalClosed {
                id: ModalId::new("cov/report")
            }
        );
    }

    #[test]
    fn defaults_give_an_id_a_title_and_a_centered_70_percent() {
        let sink = Arc::new(RecordingSink::new());
        let id = run(
            "local a = app.modal.open{ content = 'x' }; local b = app.modal.open{ content = 'y' }; return a .. ',' .. b",
            Some(Arc::clone(&sink)),
        )
        .unwrap();
        assert_eq!(id.as_deref(), Some("modal-1,modal-2"));
        let DomainEvent::ModalOpened { spec } = &sink.events()[0] else {
            panic!()
        };
        assert_eq!(spec.id, ModalId::new("cov/modal-1"));
        assert_eq!(spec.title, "cov");
        assert_eq!(spec.width, Percent::default());
        assert_eq!(spec.height, Percent::default());
        assert_eq!(spec.position, ModalPosition::CENTER);
    }

    #[test]
    fn bad_arguments_raise_clear_errors() {
        let cases = [
            ("app.modal.open('x')", "app.modal.open: expected a table"),
            ("app.modal.open{}", "`content` is missing"),
            (
                "app.modal.open{ content = 'x', width = '0%' }",
                "`width`: invalid size",
            ),
            (
                "app.modal.open{ content = 'x', height = 150 }",
                "`height`: invalid size",
            ),
            (
                "app.modal.open{ content = 'x', width = true }",
                "`width` must be a number",
            ),
            (
                "app.modal.open{ content = 'x', position = 'left-right' }",
                "contradictory position",
            ),
            (
                "app.modal.open{ content = 'x', position = 'middle' }",
                "unknown position",
            ),
            ("app.modal.open{ content = 'x', id = '' }", "`id` must be"),
            (
                "app.modal.open{ content = { type = 'nope' } }",
                "content: not a ui node",
            ),
            (
                "app.modal.open{ content = app.ui.col{ { type = 'link', text = 'x', url = 'ftp://x' } } }",
                "content: link \"ftp://x\" must be",
            ),
            (
                "app.modal.update('a', nil)",
                "app.modal.update: `content` is missing",
            ),
        ];
        for (code, expected) in cases {
            let err = error_of(code);
            assert!(err.contains(expected), "{code}: {err}");
        }
    }

    #[test]
    fn without_a_front_end_the_calls_fail() {
        let err = run("app.modal.open{ content = 'x' }", None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("no window to show modals in"), "{err}");
    }
}
