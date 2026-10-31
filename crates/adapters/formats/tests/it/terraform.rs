//! Terraform plans on fixture files.

use adapter_formats::Formats;
use application::ports::formats::FormatsPort;
use domain::devops::terraform::{AttrValue, PlanAction, PlanSource};
use serde_json::json;

fn fixture(name: &str) -> Vec<u8> {
    let path = format!(
        "{}/tests/fixtures/devops/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

#[test]
fn show_json_gives_actions_diffs_and_masks_sensitive_values() {
    let spec = Formats
        .parse_terraform_plan(&fixture("tfplan.json"), "infra/plan.json")
        .unwrap_or_else(|e| panic!("{e}"));
    spec.validate().unwrap();
    assert_eq!(spec.terraform_version.as_deref(), Some("1.9.5"));
    assert_eq!(
        spec.counts_text(),
        "1 to add, 1 to change, 1 to replace, 1 to destroy"
    );
    assert_eq!(spec.unchanged, 1);
    assert_eq!(spec.drift, ["aws_security_group.web"]);
    let paths = |ix: usize| -> Vec<String> {
        spec.resources[ix]
            .attributes
            .iter()
            .map(|a| a.path.clone())
            .collect()
    };
    let web = &spec.resources[0];
    assert_eq!(web.action, PlanAction::Replace);
    assert_eq!(web.reason.as_deref(), Some("replace_because_cannot_update"));
    assert_eq!(paths(0), ["ami", "id"]);
    assert!(web.attributes[0].forces_replacement);
    assert_eq!(web.attributes[1].after, AttrValue::Unknown);
    let db = &spec.resources[1];
    assert_eq!(
        paths(1),
        ["allocated_storage", "ingress[0].cidr_blocks[1]", "password"]
    );
    assert_eq!(db.attributes[0].before, AttrValue::Known(json!(20)));
    assert_eq!(db.attributes[2].before, AttrValue::Sensitive);
    assert_eq!(db.attributes[2].after, AttrValue::Sensitive);
    assert_eq!(
        paths(2),
        ["arn", "bucket", "force_destroy", "id", "lifecycle_rule"]
    );
    assert_eq!(paths(3), ["name"]);
    assert_eq!(spec.resources[3].action, PlanAction::Delete);
    let outputs: Vec<_> = spec
        .outputs
        .iter()
        .map(|o| (o.name.as_str(), o.action, o.after.clone()))
        .collect();
    assert_eq!(
        outputs,
        [
            ("web_ip", PlanAction::Create, AttrValue::Unknown),
            ("db_password", PlanAction::Update, AttrValue::Sensitive),
        ]
    );
    // No sensitive value anywhere in the spec.
    let all = serde_json::to_string(&spec).unwrap_or_default();
    assert!(
        !all.contains("hunter2") && !all.contains("correct-horse"),
        "{all}"
    );
    let tree = spec.to_tree();
    tree.validate().unwrap();
    assert_eq!(tree.nodes[2].label, "module.logs.aws_s3_bucket.this[0]");
}

#[test]
fn streamed_plan_lines_give_actions_only() {
    let spec = Formats
        .parse_terraform_plan(&fixture("tfplan-stream.jsonl"), "plan.jsonl")
        .unwrap_or_else(|e| panic!("{e}"));
    spec.validate().unwrap();
    assert_eq!(spec.source, PlanSource::Stream);
    assert_eq!(spec.terraform_version.as_deref(), Some("1.9.5"));
    let actions: Vec<_> = spec
        .resources
        .iter()
        .map(|r| (r.address.as_str(), r.action, r.data))
        .collect();
    assert_eq!(
        actions,
        [
            ("aws_instance.web", PlanAction::Replace, false),
            ("data.aws_ami.ubuntu", PlanAction::Read, true),
        ]
    );
    assert_eq!(spec.drift, ["aws_security_group.web"]);
}

#[test]
fn other_json_is_not_a_plan() {
    let err = Formats
        .parse_terraform_plan(b"{\"a\": 1}", "x.json")
        .unwrap_err()
        .to_string();
    assert!(err.contains("terraform show -json"), "{err}");
}
