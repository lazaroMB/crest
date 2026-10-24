//! `app.theme.define{...}` and the `color` helpers of config and theme
//! files.
//!
//! ```lua
//! app.theme.define{ name = "mine", extends = "twocrest", ui = { accent = "#7aa2f7" } }
//! local soft = color.alpha("#7aa2f7", 0.2)      -- "#7aa2f733"
//! local mid  = color.mix("#000000", "#ffffff", 0.5)
//! local up   = color.lighten("#7aa2f7", 0.1)
//! local down = color.darken("#7aa2f7", 0.1)
//! ```
//!
//! The helpers take and return hex strings; the theme table itself is
//! read by `application::theme`.

use std::sync::Arc;

use domain::theme::Color;
use mlua::{Function, Lua, Table};

use crate::config::api::{Origin, Sink, lock, report};
use crate::convert;

/// The `color` table: pure functions over hex colors.
pub(crate) fn color_table(lua: &Lua) -> mlua::Result<Table> {
    fn parse(text: &str) -> mlua::Result<Color> {
        Color::parse_hex(text).map_err(|err| mlua::Error::runtime(err.to_string()))
    }
    let table = lua.create_table()?;
    table.set(
        "alpha",
        lua.create_function(|_, (hex, alpha): (String, f32)| {
            Ok(parse(&hex)?.with_alpha(alpha).to_hex())
        })?,
    )?;
    table.set(
        "mix",
        lua.create_function(|_, (from, to, t): (String, String, f32)| {
            Ok(parse(&from)?.mix(parse(&to)?, t).to_hex())
        })?,
    )?;
    table.set(
        "lighten",
        lua.create_function(|_, (hex, amount): (String, f32)| {
            Ok(parse(&hex)?.lighten(amount).to_hex())
        })?,
    )?;
    table.set(
        "darken",
        lua.create_function(|_, (hex, amount): (String, f32)| {
            Ok(parse(&hex)?.darken(amount).to_hex())
        })?,
    )?;
    Ok(table)
}

/// The `app.theme` table.
pub(crate) fn table(lua: &Lua, sink: &Sink, origin: &Arc<Origin>) -> mlua::Result<Table> {
    let theme = lua.create_table()?;
    theme.set("define", define(lua, sink, origin)?)?;
    Ok(theme)
}

fn define(lua: &Lua, sink: &Sink, origin: &Arc<Origin>) -> mlua::Result<Function> {
    let sink = Arc::clone(sink);
    let origin = Arc::clone(origin);
    lua.create_function(move |lua, spec: mlua::Value| {
        if !spec.is_table() {
            report(
                lua,
                &sink,
                &origin,
                "app.theme.define: expects a theme table".into(),
            );
            return Ok(());
        }
        let value = convert::to_json(lua, spec)?;
        let named = value
            .get("name")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|name| !name.trim().is_empty());
        if named {
            lock(&sink).themes.push(value);
        } else {
            report(
                lua,
                &sink,
                &origin,
                "app.theme.define: a theme needs a `name`".into(),
            );
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox;

    #[test]
    fn color_helpers() {
        let lua = sandbox::new_lua(None).unwrap();
        lua.globals().set("color", color_table(&lua).unwrap()).unwrap();
        let eval = |code: &str| -> String { lua.load(code).eval().unwrap() };
        assert_eq!(eval(r##"return color.alpha("#7aa2f7", 0.2)"##), "#7aa2f733");
        assert_eq!(
            eval(r##"return color.mix("#000000", "#ffffff", 0.5)"##),
            "#808080"
        );
        assert_eq!(eval(r##"return color.lighten("#000000", 1)"##), "#ffffff");
        assert_eq!(eval(r##"return color.darken("#ffffff", 1)"##), "#000000");
        let err = lua
            .load(r#"return color.alpha("blue", 0.2)"#)
            .eval::<String>()
            .unwrap_err();
        assert!(err.to_string().contains("`blue` is not a color"), "{err}");
    }
}
