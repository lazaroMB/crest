//! `show_terraform_plan`: a Terraform plan's resource changes on the
//! tree table.
//!
//! The model gets the counts per action, the destroyed, replaced
//! (with what forces it), updated (with the changed attribute paths),
//! created and read resources, drift and outputs, never attribute
//! values: they may hold secrets Terraform did not mark sensitive.

use std::fmt::Write as _;
use std::sync::Arc;

use async_trait::async_trait;
use domain::block::BlockKind;
use domain::devops::terraform::{AttrValue, PlanAction, PlanSource, TerraformPlanSpec};
use domain::permission::ToolEffect;
use futures::FutureExt;
use schemars::JsonSchema;
use serde::Deserialize;

use super::devops::MAX_TOP;
use super::{FormatToolPorts, MAX_LISTED, invalid, one_line, read};
use crate::tools::registry::{PreparedCall, ToolContext, ToolError, ToolOutput, TypedTool};
use crate::usecases::compute::offloaded;

/// Input of `show_terraform_plan`.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShowTerraformPlan {
    /// A plan in JSON in the workspace: `terraform show -json tfplan >
    /// plan.json` (after `terraform plan -out=tfplan`; OpenTofu's `tofu
    /// show -json` too), or the JSON lines of `terraform plan -json`
    /// (actions only, no attribute diffs).
    pub path: String,
    /// Title shown above the plan; defaults to the file name.
    #[serde(default)]
    pub title: Option<String>,
}

/// `show_terraform_plan`: a plan's changes on the tree table.
#[derive(Clone)]
pub struct ShowTerraformPlanTool {
    ports: FormatToolPorts,
}

impl ShowTerraformPlanTool {
    /// The tool over `ports`.
    #[must_use]
    pub fn new(ports: FormatToolPorts) -> Self {
        Self { ports }
    }
}

/// Name of `show_terraform_plan`.
pub const SHOW_TERRAFORM_PLAN: &str = "show_terraform_plan";

#[async_trait]
impl TypedTool for ShowTerraformPlanTool {
    type Input = ShowTerraformPlan;

    fn name(&self) -> &str {
        SHOW_TERRAFORM_PLAN
    }

    fn description(&self) -> &str {
        "Shows a Terraform or OpenTofu plan in JSON (`terraform show -json \
         tfplan`) as a tree table in the user's timeline: counts to add, \
         change, replace and destroy, then each resource badged with its \
         action and its attribute diff (before and after; sensitive values \
         masked, unknown ones shown as known after apply, replacement \
         causes flagged). Returns the counts and the resources per action \
         with the changed attribute paths, drift and outputs, but no \
         values: do not repeat the plan in your reply."
    }

    fn effect(&self, _input: &ShowTerraformPlan) -> ToolEffect {
        ToolEffect::Read
    }

    async fn prepare(
        &self,
        input: ShowTerraformPlan,
        _ctx: &ToolContext,
    ) -> Result<PreparedCall, ToolError> {
        if input.path.trim().is_empty() {
            return Err(invalid(SHOW_TERRAFORM_PLAN, "`path` is empty"));
        }
        let ports = self.ports.clone();
        let subject = input.path.clone();
        let run = async move {
            let bytes = match read(ports.fs.as_ref(), &input.path).await {
                Ok(bytes) => bytes,
                Err(message) => return Ok(ToolOutput::error(message)),
            };
            let formats = Arc::clone(&ports.formats);
            let path = input.path.clone();
            let parsed = offloaded(&ports.offload, move || {
                formats.parse_terraform_plan(&bytes, &path)
            })
            .await;
            let mut spec = match parsed {
                Some(Ok(spec)) => spec,
                Some(Err(err)) => return Ok(ToolOutput::error(err.to_string())),
                None => return Ok(ToolOutput::error("the plan parser did not run")),
            };
            spec.title = input.title.filter(|t| !t.trim().is_empty());
            if let Err(err) = spec.validate() {
                return Ok(ToolOutput::error(format!("`{}`: {err}", input.path)));
            }
            let summary = terraform_summary(&input.path, &spec);
            Ok(ToolOutput::text(summary)
                .with_block_update(BlockKind::TerraformPlan(Box::new(spec))))
        };
        Ok(PreparedCall::new(subject, run.boxed()))
    }
}

/// What the model gets back from `show_terraform_plan`.
#[must_use]
pub fn terraform_summary(path: &str, spec: &TerraformPlanSpec) -> String {
    let mut out = format!("Terraform plan `{path}`");
    if let Some(version) = &spec.terraform_version {
        let _ = write!(out, " (Terraform {version})");
    }
    let _ = write!(out, " shown to the user: {}", spec.counts_text());
    if spec.unchanged > 0 {
        let _ = write!(out, "; {} unchanged", spec.unchanged);
    }
    out.push('.');
    if spec.errored {
        out.push_str(" Planning errored: the plan is incomplete.");
    }
    if spec.applyable == Some(false) {
        out.push_str(" Not applyable.");
    }
    if spec.source == PlanSource::Stream {
        out.push_str(
            " From `terraform plan -json` lines: actions only; save the plan and run \
             `terraform show -json` for attribute diffs.",
        );
    }
    let sections = [
        (PlanAction::Delete, "Destroy"),
        (PlanAction::Replace, "Replace"),
        (PlanAction::Update, "Update"),
        (PlanAction::Create, "Create"),
        (PlanAction::Read, "Read"),
        (PlanAction::Forget, "Forget"),
    ];
    for (action, head) in sections {
        let ixs = spec.with_action(action);
        if ixs.is_empty() {
            continue;
        }
        let _ = write!(out, "\n{head}:");
        for &ix in ixs.iter().take(MAX_LISTED) {
            let r = &spec.resources[ix];
            let _ = write!(out, "\n- {}", one_line(&r.address, 160));
            if let Some(reason) = &r.reason {
                let _ = write!(out, " ({})", reason.replace('_', " "));
            }
            if let Some(from) = &r.previous_address {
                let _ = write!(out, " (moved from {from})");
            }
            match action {
                PlanAction::Replace => {
                    let causes = r.replacement_causes();
                    if !causes.is_empty() {
                        let _ = write!(out, ", forced by: {}", causes.join(", "));
                    }
                }
                PlanAction::Update => {
                    let paths: Vec<String> = r
                        .attributes
                        .iter()
                        .take(MAX_TOP)
                        .map(|a| {
                            let mut p = a.path.clone();
                            if a.after == AttrValue::Sensitive || a.before == AttrValue::Sensitive {
                                p.push_str(" (sensitive)");
                            } else if a.after == AttrValue::Unknown {
                                p.push_str(" (known after apply)");
                            }
                            p
                        })
                        .collect();
                    if !paths.is_empty() {
                        let _ = write!(out, ": {}", paths.join(", "));
                    }
                    if r.attributes_total > MAX_TOP {
                        let _ = write!(out, " and {} more", r.attributes_total - MAX_TOP);
                    }
                }
                _ => {}
            }
        }
        if ixs.len() > MAX_LISTED {
            let _ = write!(out, "\n… and {} more", ixs.len() - MAX_LISTED);
        }
    }
    if !spec.drift.is_empty() {
        let shown: Vec<&str> = spec
            .drift
            .iter()
            .take(MAX_LISTED)
            .map(String::as_str)
            .collect();
        let _ = write!(
            out,
            "\nChanged outside Terraform (drift): {}",
            shown.join(", ")
        );
        if spec.drift.len() > MAX_LISTED {
            let _ = write!(out, " and {} more", spec.drift.len() - MAX_LISTED);
        }
    }
    if !spec.outputs.is_empty() {
        let outputs: Vec<String> = spec
            .outputs
            .iter()
            .take(MAX_LISTED)
            .map(|o| format!("{} ({})", o.name, o.action.verb()))
            .collect();
        let _ = write!(out, "\nOutputs: {}", outputs.join(", "));
    }
    if spec.source == PlanSource::Show && !spec.resources.is_empty() {
        out.push_str(
            "\nAttribute values are not listed here; the user sees them, with sensitive values \
             masked.",
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use domain::conversation::{SessionId, ToolUseId, TurnId};
    use serde_json::{Value, json};

    use super::*;
    use crate::testing::formats::FakeFormats;
    use crate::testing::fs::{InMemoryFs, test_root};
    use crate::tools::registry::{Tool, Typed};
    use crate::usecases::compute::inline_offload;

    fn ctx() -> ToolContext {
        ToolContext::new(SessionId::new(), TurnId::new(), ToolUseId::new("toolu_1"))
    }

    fn ports(formats: Arc<FakeFormats>) -> FormatToolPorts {
        let fs = InMemoryFs::new(test_root()).with_file("infra/plan.json", "{}");
        FormatToolPorts {
            formats,
            fs: Arc::new(fs),
            offload: inline_offload(),
        }
    }

    async fn call(tool: &dyn Tool, input: Value) -> Result<ToolOutput, ToolError> {
        let prepared = tool.prepare(input, &ctx()).await?;
        prepared.run.await
    }

    #[test]
    fn definition() {
        let tool = Typed(ShowTerraformPlanTool::new(ports(Arc::new(
            FakeFormats::new(),
        ))));
        insta::assert_json_snapshot!(
            "show_terraform_plan_definition",
            json!({
                "name": tool.name(),
                "description": tool.description(),
                "input_schema": tool.input_schema(),
            })
        );
    }

    #[tokio::test]
    async fn the_model_gets_actions_and_paths_but_no_values() {
        let formats = Arc::new(FakeFormats::new());
        let tool = Typed(ShowTerraformPlanTool::new(ports(Arc::clone(&formats))));
        let out = call(&tool, json!({ "path": "infra/plan.json" }))
            .await
            .unwrap();
        assert!(!out.is_error, "{}", out.text_content());
        let text = out.text_content();
        assert_eq!(
            text,
            "Terraform plan `infra/plan.json` (Terraform 1.9.0) shown to the user: 1 to add, 1 to \
             change, 1 to replace, 0 to destroy; 4 unchanged.\n\
             Replace:\n\
             - aws_instance.web, forced by: ami\n\
             Update:\n\
             - aws_db_instance.main: password (sensitive)\n\
             Create:\n\
             - aws_s3_bucket.logs\n\
             Attribute values are not listed here; the user sees them, with sensitive values \
             masked."
        );
        assert!(!text.contains("ami-2"));
        assert!(matches!(
            out.block_update,
            Some(BlockKind::TerraformPlan(_))
        ));
        let empty = tool.prepare(json!({ "path": " " }), &ctx()).await;
        assert!(matches!(empty, Err(ToolError::InvalidInput { .. })));
    }
}
