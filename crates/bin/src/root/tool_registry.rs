//! The tools the agent gets: files, components, forms, statistics, formats,
//! the web, sub-tasks, simulations and the plugins' tools.

use std::sync::Arc;

use adapter_compute::StatsEngine;
use application::ports::fs::FileSystemPort;
use application::tools::compute::ComputeToolPorts;

/// The statistics engine, reading data through `fs` and running on the
/// blocking pool of the current runtime.
pub(super) fn compute_ports(fs: Arc<dyn FileSystemPort>) -> ComputeToolPorts {
    let handle = tokio::runtime::Handle::current();
    ComputeToolPorts {
        engine: Arc::new(StatsEngine::new()),
        fs,
        offload: Arc::new(move |job| {
            // The caller awaits the job's own channel, not the join handle.
            drop(handle.spawn_blocking(job));
        }),
    }
}
