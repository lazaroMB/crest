//! `simulate_solution`: the agent asks for a simulation of a Spec.

use async_trait::async_trait;
use domain::permission::ToolEffect;
use futures::FutureExt as _;
use schemars::JsonSchema;
use serde::Deserialize;

use super::registry::{PreparedCall, ToolContext, ToolError, ToolOutput, TypedTool};
use crate::flows::research::nodes::spec::sink::SPECS_DIR;
use crate::usecases::simulate::{SimulateRequest, Simulator};

const DESCRIPTION: &str = "Simulates the solution of a Spec in .crest/specs/: derives \
assertions from its test protocol, writes an executable model (crest.sysmodel/v1) from its \
diagrams, runs every scenario deterministically and records the verdicts as evidence in the \
Spec folder (simulation/, sources.json, journal.md). The window opens the board. Use it when \
the user asks to validate or simulate a design before implementing it.";

/// Input of `simulate_solution`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SimulateInput {
    /// Spec folder name (`0007-session-cache`), its number (`7`) or its path.
    pub spec: String,
}

/// The `simulate_solution` tool.
#[derive(Debug, Clone)]
pub struct SimulateTool {
    simulator: Simulator,
    now: fn() -> u64,
}

impl SimulateTool {
    /// A tool running `simulator`; `now` gives Unix milliseconds.
    #[must_use]
    pub fn new(simulator: Simulator, now: fn() -> u64) -> Self {
        Self { simulator, now }
    }
}

#[async_trait]
impl TypedTool for SimulateTool {
    type Input = SimulateInput;

    fn name(&self) -> &str {
        "simulate_solution"
    }

    fn description(&self) -> &str {
        DESCRIPTION
    }

    /// It writes into the Spec folder.
    fn effect(&self, _input: &SimulateInput) -> ToolEffect {
        ToolEffect::Write
    }

    async fn prepare(
        &self,
        input: SimulateInput,
        _ctx: &ToolContext,
    ) -> Result<PreparedCall, ToolError> {
        let spec = input.spec.trim().to_owned();
        if spec.is_empty() {
            return Err(ToolError::InvalidInput {
                tool: "simulate_solution".to_owned(),
                message: "`spec` is empty; name a folder of .crest/specs/".to_owned(),
            });
        }
        let simulator = self.simulator.clone();
        let now_ms = (self.now)();
        let subject = format!("{SPECS_DIR}/{spec}");
        let run = async move {
            let report = simulator
                .run(SimulateRequest {
                    spec,
                    now_ms,
                    open_board: true,
                })
                .await
                .map_err(|e| ToolError::Execution(format!("simulation failed: {e}")))?;
            Ok(ToolOutput::text(report.summary()))
        };
        Ok(PreparedCall::new(subject, run.boxed()))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use domain::conversation::{SessionId, ToolUseId, TurnId};
    use domain::model::ModelPolicy;
    use domain::workspace::WorkspaceRoot;

    use super::*;
    use crate::testing::fs::InMemoryFs;
    use crate::testing::llm::ScriptedLlm;
    use crate::testing::sim::{self, FOLDER, SOURCES_JSON, SPEC_MD};
    use crate::tools::registry::{Tool, Typed};
    use crate::usecases::simulate::SimulateDeps;

    #[tokio::test]
    async fn the_tool_simulates_and_summarizes() {
        let fs = Arc::new(
            InMemoryFs::new(WorkspaceRoot::new(std::env::temp_dir()).unwrap())
                .with_file(&format!(".crest/specs/{FOLDER}/spec.md"), SPEC_MD)
                .with_file(&format!(".crest/specs/{FOLDER}/sources.json"), SOURCES_JSON),
        );
        let simulator = Simulator::new(SimulateDeps {
            llm: Arc::new(ScriptedLlm::new(sim::script())),
            models: ModelPolicy::default(),
            fs: fs.clone(),
            search: fs.clone(),
            events: None,
            export: None,
        });
        let tool = Typed(SimulateTool::new(simulator, || 0));
        assert_eq!(
            tool.effect(&serde_json::json!({ "spec": "1" })),
            ToolEffect::Write
        );
        let ctx = ToolContext::new(SessionId::new(), TurnId::new(), ToolUseId::new("t1"));
        assert!(
            tool.prepare(serde_json::json!({ "spec": " " }), &ctx)
                .await
                .is_err()
        );
        let call = tool
            .prepare(serde_json::json!({ "spec": "1" }), &ctx)
            .await
            .unwrap();
        assert_eq!(call.subject, ".crest/specs/1");
        let output = call.run.await.unwrap();
        let text = output.text_content();
        assert!(text.contains("PASS one_request"), "{text}");
        assert!(text.contains("FAIL burst"), "{text}");
        assert!(
            fs.file(&format!(".crest/specs/{FOLDER}/simulation/sysmodel.json"))
                .is_some()
        );
    }
}
