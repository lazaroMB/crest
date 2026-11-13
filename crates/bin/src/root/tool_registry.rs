//! The tools the agent gets: files, components, forms, statistics, formats,
//! the web, sub-tasks, simulations and the plugins' tools.

use std::sync::Arc;

use adapter_compute::StatsEngine;
use adapter_export::HtmlExporter;
use adapter_formats::Formats;
use adapter_lua::LuaHandle;
use adapter_search::RipgrepSearch;
use anyhow::Context as _;
use application::ports::events::EventSink;
use application::ports::form::FormPort;
use application::ports::formats::FormatsPort;
use application::ports::fs::FileSystemPort;
use application::ports::instructions::InstructionsPort;
use application::ports::llm::LlmPort;
use application::ports::plugin::{PluginHostPort, PluginToolSpec};
use application::ports::search::FileSearchPort;
use application::ports::shell::{CommandRewritePort, ShellPort};
use application::ports::skills::SkillsPort;
use application::ports::telemetry::TelemetryPort;
use application::privacy::Redactor;
use application::tools::components::register_component_tools;
use application::tools::compute::{ComputeToolPorts, register_compute_tools};
use application::tools::files::{FileToolPorts, register_file_tools};
use application::tools::form::register_form_tool;
use application::tools::formats::{FormatToolPorts, register_format_tools};
use application::tools::ghs::ShowGhsTool;
use application::tools::plugin::PluginTool;
use application::tools::registry::ToolRegistry;
use application::tools::simulate::SimulateTool;
use application::tools::skill::{SkillDeps, SkillToolSource};
use application::tools::task::{TaskPorts, TaskTool};
use application::tools::web::WebTools;
use application::usecases::background::AgentRunner;
use application::usecases::run_turn::SharedPermissions;
use application::usecases::simulate::{SimulateDeps, Simulator};
use domain::model::ModelPolicy;
use std::sync::OnceLock;

use super::now_ms;

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

/// What [`register_tools`] needs from the composition root.
pub(super) struct ToolInputs<'a> {
    pub(super) redactor: Redactor,
    pub(super) search: Arc<RipgrepSearch>,
    pub(super) fs: Arc<dyn FileSystemPort>,
    /// The file port of the tools, locked while a discovery runs.
    pub(super) tool_fs: Arc<dyn FileSystemPort>,
    pub(super) shell: Arc<dyn ShellPort>,
    pub(super) rewriter: Option<Arc<dyn CommandRewritePort>>,
    pub(super) form: Arc<dyn FormPort>,
    pub(super) plugins: &'a Option<Arc<LuaHandle>>,
    pub(super) web: &'a Option<WebTools>,
    pub(super) llm: Arc<dyn LlmPort>,
    pub(super) telemetry: Arc<dyn TelemetryPort>,
    pub(super) policy: &'a ModelPolicy,
    pub(super) file_search: Arc<dyn FileSearchPort>,
    pub(super) events: Arc<dyn EventSink>,
    /// The tools that `init.lua` and the plugins declare.
    pub(super) plugin_tools: &'a [PluginToolSpec],
    /// The `AGENTS.md` and `CLAUDE.md` files, for the `task` subagent.
    pub(super) instructions: Arc<dyn InstructionsPort>,
    /// The skills, for the `skill` tool.
    pub(super) skills: Arc<dyn SkillsPort>,
    /// The chat's permissions, which skills' `allowed-tools` add to.
    pub(super) permissions: SharedPermissions,
    /// The subagents of `task` and forked skills, set once they exist.
    pub(super) runner: Arc<OnceLock<Arc<AgentRunner>>>,
}

/// The registry and the ports built on the way that the app keeps.
pub(super) struct Tools {
    pub(super) tools: ToolRegistry,
    /// The read-only subagent of background `explore` jobs.
    pub(super) explore: Arc<TaskTool>,
    pub(super) compute: ComputeToolPorts,
    pub(super) formats: Arc<dyn FormatsPort>,
    pub(super) simulator: Simulator,
}

/// Registers every tool and hands the plugin host the statistics, formats
/// and form ports. What cannot be offered (a plugin tool, an `app.*` API)
/// becomes a warning, in this order.
///
/// # Errors
///
/// A builtin tool that cannot be registered.
pub(super) async fn register_tools(
    inputs: ToolInputs<'_>,
    warnings: &mut Vec<String>,
) -> anyhow::Result<Tools> {
    let ToolInputs {
        redactor,
        search,
        fs,
        tool_fs,
        shell,
        rewriter,
        form,
        plugins,
        web,
        llm,
        telemetry,
        policy,
        file_search,
        events,
        plugin_tools,
        instructions,
        skills,
        permissions,
        runner,
    } = inputs;
    let mut tools = ToolRegistry::new().with_redactor(redactor);
    register_file_tools(
        &mut tools,
        FileToolPorts {
            file_search: search.clone(),
            content_search: search.clone(),
            fs: Arc::clone(&tool_fs),
            shell: Arc::clone(&shell),
            rewriter,
        },
    )
    .context("tool registration")?;
    register_component_tools(&mut tools).context("tool registration")?;
    register_form_tool(&mut tools, Arc::clone(&form)).context("tool registration")?;
    let compute = compute_ports(Arc::clone(&fs));
    register_compute_tools(&mut tools, &compute).context("tool registration")?;
    let formats: Arc<dyn FormatsPort> = Arc::new(Formats::new());
    register_format_tools(
        &mut tools,
        &FormatToolPorts {
            formats: Arc::clone(&formats),
            fs: Arc::clone(&compute.fs),
            offload: Arc::clone(&compute.offload),
        },
    )
    .context("tool registration")?;
    if let Some(host) = &plugins {
        if let Err(err) = host
            .provide_compute(Arc::clone(&compute.engine), Arc::clone(&compute.fs))
            .await
        {
            warnings.push(format!("app.compute unavailable: {err}"));
        }
        if let Err(err) = host
            .provide_formats(
                Arc::clone(&formats),
                Arc::clone(&compute.fs),
                web.as_ref().map(|w| Arc::clone(&w.web)),
            )
            .await
        {
            warnings.push(format!("app.formats unavailable: {err}"));
        }
        if let Err(err) = host.provide_forms(Arc::clone(&form)).await {
            warnings.push(format!("app.ask unavailable: {err}"));
        }
    }
    if let Some(web) = &web {
        web.register(&mut tools).context("tool registration")?;
        tools
            .register_typed(ShowGhsTool::new(Arc::clone(&web.web), now_ms))
            .context("tool registration")?;
    }
    let task_ports = TaskPorts {
        llm: Arc::clone(&llm),
        telemetry: Arc::clone(&telemetry),
        models: policy.clone(),
        file_search: search.clone(),
        content_search: search,
        fs: Arc::clone(&fs),
        web: web.clone(),
        instructions: Some(instructions),
    };
    let task = TaskTool::new(task_ports.clone(), tools.budget())
        .context("tool registration")?
        .with_runner(Arc::clone(&runner));
    tools.register_typed(task).context("tool registration")?;
    let explore = Arc::new(TaskTool::new(task_ports, tools.budget()).context("tool registration")?);
    tools.add_source(Arc::new(SkillToolSource::new(SkillDeps {
        skills,
        permissions,
        shell: Arc::clone(&shell),
        runner,
        budget: tools.budget(),
    })));
    let simulator = Simulator::new(SimulateDeps {
        llm: Arc::clone(&llm),
        models: policy.clone(),
        fs: Arc::clone(&fs),
        search: file_search.clone(),
        events: Some(Arc::clone(&events)),
        export: Some(Arc::new(HtmlExporter::new())),
    });
    tools
        .register_typed(SimulateTool::new(simulator.clone(), now_ms))
        .context("tool registration")?;
    if let Some(host) = &plugins {
        for spec in plugin_tools.iter().cloned() {
            let name = spec.name.clone();
            let tool = PluginTool::new(spec, host.clone() as Arc<dyn PluginHostPort>);
            if let Err(err) = tools.register(Arc::new(tool)) {
                warnings.push(format!("plugin tool `{name}` ignored: {err}"));
            }
        }
    }
    Ok(Tools {
        tools,
        explore,
        compute,
        formats,
        simulator,
    })
}
