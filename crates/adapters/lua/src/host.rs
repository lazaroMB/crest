//! The Lua VM on its own thread.
//!
//! [`LuaHost::spawn`] starts a `std::thread` that owns the [`Vm`] and serves
//! requests one at a time. [`LuaHandle`] is the cheap, cloneable side the
//! rest of the app keeps: each call sends a request over a channel and awaits
//! the answer on a oneshot, so the caller's async runtime never blocks while
//! Lua runs. The VM itself never leaves its thread.

use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;

use application::ports::plugin::{
    CallbackId, ConfigSources, HookEvent, HookOutcome, LoadedConfig, PluginCapabilities,
    PluginError, PluginHostPort, PluginOutput,
};
use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::oneshot;

use crate::callbacks::CallbackIds;
use crate::config;
use crate::vm::Vm;

/// Name of the VM thread, as shown by debuggers and panics.
const THREAD_NAME: &str = "crest-lua";

/// Settings of the Lua host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostOptions {
    /// Maximum VM instructions a single call may run before it is stopped.
    pub instruction_limit: u64,
    /// Maximum memory of the whole VM, in bytes; `None` means unlimited.
    pub memory_limit: Option<usize>,
    /// Workspace root, where `app.shell.run` runs programs; `None`
    /// disables it.
    pub workspace: Option<PathBuf>,
    /// Whether `app.secret` reads the system keyring before the
    /// environment (tests turn it off).
    pub keyring: bool,
    /// Environment that `app.secret` and `app.shell.run` see instead of
    /// the process's (for tests); `None` uses the process environment.
    pub environment: Option<Vec<(String, String)>>,
}

impl Default for HostOptions {
    fn default() -> Self {
        Self {
            instruction_limit: 100_000_000,
            memory_limit: Some(256 * 1024 * 1024),
            workspace: None,
            keyring: true,
            environment: None,
        }
    }
}

/// Work sent to the VM thread.
type Request = Box<dyn FnOnce(&mut Vm) + Send>;

/// Starts Lua hosts.
#[derive(Debug)]
pub struct LuaHost;

impl LuaHost {
    /// Creates a VM on a new thread and returns the handle to talk to it.
    ///
    /// The thread ends when the last [`LuaHandle`] is dropped.
    pub fn spawn(options: HostOptions) -> Result<LuaHandle, PluginError> {
        let vm = Vm::new(options, CallbackIds::default())?;
        let (tx, rx) = mpsc::channel::<Request>();
        thread::Builder::new()
            .name(THREAD_NAME.into())
            .spawn(move || serve(vm, &rx))
            .map_err(|e| PluginError::Unavailable(format!("cannot start the Lua thread: {e}")))?;
        Ok(LuaHandle { tx })
    }
}

fn serve(mut vm: Vm, rx: &mpsc::Receiver<Request>) {
    while let Ok(request) = rx.recv() {
        request(&mut vm);
    }
    tracing::debug!("lua host stopped: every handle was dropped");
}

/// Handle to a running Lua host. Cloning it is cheap; all clones talk to the
/// same VM.
#[derive(Debug, Clone)]
pub struct LuaHandle {
    tx: mpsc::Sender<Request>,
}

impl LuaHandle {
    /// Runs `f` on the VM thread and waits, without blocking, for its result.
    pub(crate) async fn run<R, F>(&self, f: F) -> Result<R, PluginError>
    where
        F: FnOnce(&mut Vm) -> R + Send + 'static,
        R: Send + 'static,
    {
        let (reply, answer) = oneshot::channel();
        let request: Request = Box::new(move |vm| {
            // The caller may have stopped waiting; that is not an error here.
            let _ = reply.send(f(vm));
        });
        self.tx
            .send(request)
            .map_err(|_| PluginError::Unavailable("the Lua thread has stopped".into()))?;
        answer
            .await
            .map_err(|_| PluginError::Unavailable("the Lua thread dropped the request".into()))
    }
}

#[async_trait]
impl PluginHostPort for LuaHandle {
    async fn load_config(&self, sources: ConfigSources) -> Result<LoadedConfig, PluginError> {
        self.run(move |vm| config::load(vm, &sources)).await?
    }

    async fn invoke_callback(
        &self,
        id: CallbackId,
        args: Value,
    ) -> Result<PluginOutput, PluginError> {
        self.run(move |vm| vm.invoke(id, &args)).await?
    }

    async fn call_tool(&self, name: &str, input: Value) -> Result<PluginOutput, PluginError> {
        let name = name.to_owned();
        self.run(move |vm| vm.call_tool(&name, &input)).await?
    }

    async fn run_hook(&self, event: HookEvent) -> Result<HookOutcome, PluginError> {
        self.run(move |vm| vm.run_hooks(event)).await?
    }

    async fn grant_capabilities(
        &self,
        granted: Vec<PluginCapabilities>,
    ) -> Result<(), PluginError> {
        self.run(move |vm| vm.grant(&granted)).await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    use serde_json::json;

    use super::*;

    fn spawn() -> LuaHandle {
        LuaHost::spawn(HostOptions::default()).unwrap()
    }

    #[tokio::test]
    async fn runs_code_on_the_vm_thread() {
        let host = spawn();
        let (value, name) = host
            .run(|vm| {
                let n: i64 = vm.lua.load("return 40 + 2").eval().unwrap();
                (n, thread::current().name().map(str::to_owned))
            })
            .await
            .unwrap();
        assert_eq!(value, 42);
        assert_eq!(name.as_deref(), Some(THREAD_NAME));
    }

    #[tokio::test]
    async fn state_persists_between_calls() {
        let host = spawn();
        host.run(|vm| vm.lua.load("counter = 1").exec().unwrap())
            .await
            .unwrap();
        let n: i64 = host
            .run(|vm| {
                vm.lua
                    .load("counter = counter + 1; return counter")
                    .eval()
                    .unwrap()
            })
            .await
            .unwrap();
        assert_eq!(n, 2);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_callers_all_get_their_answer() {
        let host = spawn();
        let tasks: Vec<_> = (0..32_i64)
            .map(|i| {
                let host = host.clone();
                tokio::spawn(async move {
                    host.run(move |vm| {
                        let f: mlua::Function = vm
                            .lua
                            .load("return function(x) return x * 2 end")
                            .eval()
                            .unwrap();
                        f.call::<i64>(i).unwrap()
                    })
                    .await
                    .unwrap()
                })
            })
            .collect();
        for (i, task) in tasks.into_iter().enumerate() {
            assert_eq!(task.await.unwrap(), i64::try_from(i).unwrap() * 2);
        }
    }

    // On a single-threaded runtime, a blocking call would freeze the ticker.
    #[tokio::test(flavor = "current_thread")]
    async fn a_slow_lua_call_does_not_block_the_runtime() {
        let host = LuaHost::spawn(HostOptions {
            instruction_limit: u64::MAX,
            ..HostOptions::default()
        })
        .unwrap();
        let ticks = Arc::new(AtomicU64::new(0));
        let ticker = {
            let ticks = Arc::clone(&ticks);
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                    ticks.fetch_add(1, Ordering::SeqCst);
                }
            })
        };
        host.run(|vm| {
            vm.lua
                .load("local t = os.clock(); while os.clock() - t < 0.3 do end")
                .exec()
                .unwrap();
        })
        .await
        .unwrap();
        ticker.abort();
        assert!(ticks.load(Ordering::SeqCst) >= 10, "runtime was blocked");
    }

    async fn register(host: &LuaHandle, code: &'static str) -> CallbackId {
        host.run(move |vm| crate::vm::register_code(vm, code))
            .await
            .unwrap()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_port_calls_are_all_answered() {
        let host = spawn();
        let id = register(&host, "return function(n) return n * n end").await;
        let port: Arc<dyn PluginHostPort> = Arc::new(host);
        let tasks: Vec<_> = (0..16_i64)
            .map(|i| {
                let port = Arc::clone(&port);
                tokio::spawn(async move { port.invoke_callback(id, json!(i)).await.unwrap() })
            })
            .collect();
        for (i, task) in (0_i64..).zip(tasks) {
            assert_eq!(task.await.unwrap(), PluginOutput::Value(json!(i * i)));
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_slow_callback_does_not_stop_other_tasks() {
        let host = spawn();
        let slow = register(
            &host,
            "return function() local t = os.clock(); while os.clock() - t < 0.3 do end; return 'slow' end",
        )
        .await;
        let fast_done = Arc::new(AtomicU64::new(0));
        let other = {
            let fast_done = Arc::clone(&fast_done);
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(20)).await;
                fast_done.store(1, Ordering::SeqCst);
            })
        };
        let out = host.invoke_callback(slow, Value::Null).await.unwrap();
        assert_eq!(out, PluginOutput::Value(json!("slow")));
        assert_eq!(
            fast_done.load(Ordering::SeqCst),
            1,
            "the other task never ran"
        );
        other.await.unwrap();
    }

    #[tokio::test]
    async fn the_host_keeps_answering_after_a_runaway_plugin() {
        let host = LuaHost::spawn(HostOptions {
            instruction_limit: 1_000_000,
            ..HostOptions::default()
        })
        .unwrap();
        let spin = register(&host, "return function() while true do end end").await;
        let ok = register(&host, "return function() return 'alive' end").await;
        let cut = host.invoke_callback(spin, Value::Null).await;
        assert_eq!(cut, Err(PluginError::InstructionLimit));
        let alive = host.invoke_callback(ok, Value::Null).await;
        assert_eq!(alive, Ok(PluginOutput::Value(json!("alive"))));
    }

    #[tokio::test]
    async fn load_config_registers_tools_for_later_calls() {
        let dir = tempfile::tempdir().unwrap();
        let global = dir.path().join("init.lua");
        std::fs::write(
            &global,
            "app.tools.register({ name = 'echo', handler = function(i) return i.text end })",
        )
        .unwrap();
        let host = spawn();
        let config = host
            .load_config(ConfigSources {
                global: Some(global),
                workspace: None,
            })
            .await
            .unwrap();
        assert_eq!(config.tools.len(), 1);
        let out = host.call_tool("echo", json!({ "text": "hi" })).await;
        assert_eq!(out, Ok(PluginOutput::Value(json!("hi"))));
    }

    #[tokio::test]
    async fn port_errors_come_back_to_the_caller() {
        let host = spawn();
        let missing = host.invoke_callback(CallbackId(404), Value::Null).await;
        assert!(matches!(missing, Err(PluginError::NotFound(_))));
        let tool = host.call_tool("nope", Value::Null).await;
        assert!(matches!(tool, Err(PluginError::NotFound(_))));
        let hook = host
            .run_hook(HookEvent::BeforeTool {
                name: "t".into(),
                input: json!({}),
            })
            .await
            .unwrap();
        assert_eq!(hook, HookOutcome::Continue);
    }

    #[tokio::test]
    async fn a_dead_thread_reports_unavailable() {
        let host = spawn();
        let panicked = host.run(|_| -> () { panic!("boom") }).await;
        assert!(matches!(panicked, Err(PluginError::Unavailable(_))));
        let after = host.run(|_| ()).await;
        assert!(matches!(after, Err(PluginError::Unavailable(_))));
    }
}
