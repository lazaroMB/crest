//! Lua functions kept alive by the host and referred to by [`CallbackId`].
//!
//! Functions live in a table of the Lua registry, keyed by id, so they stay
//! inside the VM. Ids come from a counter shared by every VM a host creates:
//! after a config reload, ids handed out by the previous VM never point to a
//! different function, they just stop resolving.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use application::ports::plugin::CallbackId;
use mlua::{Function, Lua, Table};

/// Name of the registry table holding the callbacks.
const REGISTRY_TABLE: &str = "crest.callbacks";

/// Source of fresh callback ids, shared by all VMs of one host.
#[derive(Debug, Clone, Default)]
pub(crate) struct CallbackIds(Arc<AtomicU64>);

impl CallbackIds {
    fn next(&self) -> CallbackId {
        CallbackId(self.0.fetch_add(1, Ordering::Relaxed) + 1)
    }
}

/// Prepares `lua` to store callbacks with ids taken from `ids`.
pub(crate) fn install(lua: &Lua, ids: CallbackIds) -> mlua::Result<()> {
    lua.set_named_registry_value(REGISTRY_TABLE, lua.create_table()?)?;
    lua.set_app_data(ids);
    Ok(())
}

fn table(lua: &Lua) -> mlua::Result<Table> {
    lua.named_registry_value(REGISTRY_TABLE)
}

/// Keeps `f` alive and returns its id.
pub(crate) fn register(lua: &Lua, f: Function) -> mlua::Result<CallbackId> {
    let id = lua
        .app_data_ref::<CallbackIds>()
        .ok_or_else(|| mlua::Error::runtime("callback registry not installed"))?
        .next();
    table(lua)?.raw_set(id.0, f)?;
    Ok(id)
}

/// The function registered as `id`, if any.
pub(crate) fn get(lua: &Lua, id: CallbackId) -> mlua::Result<Option<Function>> {
    table(lua)?.raw_get(id.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lua(ids: &CallbackIds) -> Lua {
        let lua = Lua::new();
        install(&lua, ids.clone()).unwrap();
        lua
    }

    #[test]
    fn registered_functions_can_be_called_by_id() {
        let ids = CallbackIds::default();
        let lua = lua(&ids);
        let f: Function = lua
            .load("return function(x) return x + 1 end")
            .eval()
            .unwrap();
        let id = register(&lua, f).unwrap();
        let back = get(&lua, id).unwrap().unwrap();
        assert_eq!(back.call::<i64>(1).unwrap(), 2);
        assert!(get(&lua, CallbackId(id.0 + 1)).unwrap().is_none());
    }

    #[test]
    fn ids_are_not_reused_across_vms() {
        let ids = CallbackIds::default();
        let first = lua(&ids);
        let f: Function = first.load("return function() end").eval().unwrap();
        let old = register(&first, f).unwrap();

        let second = lua(&ids);
        let g: Function = second.load("return function() end").eval().unwrap();
        let new = register(&second, g).unwrap();
        assert_ne!(old, new);
        assert!(get(&second, old).unwrap().is_none());
    }
}
