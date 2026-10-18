//! The Lua VM on its own thread.
//!
//! [`LuaHost::spawn`] starts a `std::thread` that owns the [`Vm`] and serves
//! requests one at a time. [`LuaHandle`] is the cheap, cloneable side the
//! rest of the app keeps: each call sends a request over a channel and awaits
//! the answer on a oneshot, so the caller's async runtime never blocks while
//! Lua runs. The VM itself never leaves its thread.

use std::sync::mpsc;
use std::thread;

use application::ports::plugin::PluginError;
use mlua::Lua;
use tokio::sync::oneshot;

/// Name of the VM thread, as shown by debuggers and panics.
const THREAD_NAME: &str = "crest-lua";

/// Settings of the Lua host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostOptions {
    /// Maximum VM instructions a single call may run before it is stopped.
    pub instruction_limit: u64,
    /// Maximum memory of the whole VM, in bytes; `None` means unlimited.
    pub memory_limit: Option<usize>,
}

impl Default for HostOptions {
    fn default() -> Self {
        Self {
            instruction_limit: 100_000_000,
            memory_limit: Some(256 * 1024 * 1024),
        }
    }
}

/// State owned by the VM thread.
pub(crate) struct Vm {
    pub(crate) lua: Lua,
    pub(crate) options: HostOptions,
}

impl Vm {
    pub(crate) fn new(options: HostOptions) -> Result<Self, PluginError> {
        Ok(Self {
            lua: Lua::new(),
            options,
        })
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
        let vm = Vm::new(options)?;
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

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
        let host = spawn();
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

    #[tokio::test]
    async fn a_dead_thread_reports_unavailable() {
        let host = spawn();
        let panicked = host.run(|_| -> () { panic!("boom") }).await;
        assert!(matches!(panicked, Err(PluginError::Unavailable(_))));
        let after = host.run(|_| ()).await;
        assert!(matches!(after, Err(PluginError::Unavailable(_))));
    }
}
