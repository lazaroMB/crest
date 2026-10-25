//! A Terraform plan as a tree of resource changes (`show_terraform_plan`).
//!
//! The adapter reads `terraform show -json <planfile>` (or the
//! `planned_change` lines of `terraform plan -json`, which carry no
//! attribute values) into one [`ResourceChange`] per changed resource,
//! each with its attribute differences as [`AttrChange`]s on flattened
//! paths (`tags.Name`, `ingress[0].cidr_blocks[1]`).
//!
//! Values Terraform marks sensitive never reach the spec: the adapter
//! stores [`AttrValue::Sensitive`] instead, so neither the block nor the
//! model's summary can show them. Values known only after apply are
//! [`AttrValue::Unknown`].

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::invalid;
use crate::block::BlockError;
use crate::widget::tree::{BadgeTone, ColumnKind, TreeColumn, TreeNode, TreeSpec};

/// Resource changes a block keeps.
pub const MAX_RESOURCES: usize = 10_000;
/// Attribute changes kept per resource.
pub const MAX_ATTRS: usize = 300;

const BLOCK: &str = "terraform_plan";

/// What happens to a resource or output.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum PlanAction {
    /// Destroyed.
    Delete,
    /// Destroyed and created again (in either order).
    Replace,
    /// Changed in place.
    Update,
    /// Created.
    Create,
    /// A data source read during apply.
    Read,
    /// Removed from the state without being destroyed (`removed` block).
    Forget,
    /// Nothing to do.
    NoOp,
}

impl PlanAction {
    /// The action of Terraform's `actions` list.
    #[must_use]
    pub fn from_actions(actions: &[&str]) -> Self {
        match actions {
            ["create"] => Self::Create,
            ["delete"] => Self::Delete,
            ["update"] => Self::Update,
            ["read"] => Self::Read,
            ["forget"] | ["remove"] => Self::Forget,
            ["delete", "create"] | ["create", "delete"] | ["replace"] => Self::Replace,
            _ => Self::NoOp,
        }
    }

    /// Verb as Terraform's summary says it (`add`, `change`, `destroy`).
    #[must_use]
    pub fn verb(self) -> &'static str {
        match self {
            Self::Create => "add",
            Self::Update => "change",
            Self::Delete => "destroy",
            Self::Replace => "replace",
            Self::Read => "read",
            Self::Forget => "forget",
            Self::NoOp => "no-op",
        }
    }

    /// Badge text with Terraform's symbol (`+ create`, `-/+ replace`).
    #[must_use]
    pub fn badge(self) -> &'static str {
        match self {
            Self::Create => "+ create",
            Self::Update => "~ update",
            Self::Delete => "- destroy",
            Self::Replace => "-/+ replace",
            Self::Read => "<= read",
            Self::Forget => ". forget",
            Self::NoOp => "no-op",
        }
    }

    /// Badge color: creates are good, deletes and replaces are danger.
    #[must_use]
    pub fn tone(self) -> BadgeTone {
        match self {
            Self::Create => BadgeTone::Ok,
            Self::Update => BadgeTone::Warning,
            Self::Delete | Self::Replace => BadgeTone::Error,
            Self::Read => BadgeTone::Info,
            Self::Forget | Self::NoOp => BadgeTone::Neutral,
        }
    }
}

/// One side of an attribute change.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum AttrValue {
    /// Not set on this side.
    #[default]
    Absent,
    /// A value (a scalar, or an empty list or map).
    Known(Value),
    /// Marked sensitive: the value is not kept.
    Sensitive,
    /// Known only after apply.
    Unknown,
}

impl AttrValue {
    /// As Terraform prints it: strings quoted, `(sensitive value)`,
    /// `(known after apply)`, empty when absent.
    #[must_use]
    pub fn text(&self) -> String {
        match self {
            Self::Absent => String::new(),
            Self::Known(value) => value.to_string(),
            Self::Sensitive => "(sensitive value)".to_owned(),
            Self::Unknown => "(known after apply)".to_owned(),
        }
    }
}

/// One attribute that differs (or, for a create or destroy, is set).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AttrChange {
    /// Flattened path: `tags.Name`, `ingress[0].port`, `tags["a.b"]`.
    pub path: String,
    /// The value before.
    #[serde(default)]
    pub before: AttrValue,
    /// The value after.
    #[serde(default)]
    pub after: AttrValue,
    /// Changing it forces the resource to be replaced.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub forces_replacement: bool,
}

/// One resource with a planned change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ResourceChange {
    /// Full address (`module.net.aws_subnet.private[1]`).
    pub address: String,
    /// Resource type (`aws_instance`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// `data` for a data source.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub data: bool,
    /// The provider (`registry.terraform.io/hashicorp/aws`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// What happens to it.
    pub action: PlanAction,
    /// Why (`replace_because_cannot_update`, `delete_because_no_resource_config`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The resource moves from this address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_address: Option<String>,
    /// The attributes that change (all set ones for a create or
    /// destroy), at most [`MAX_ATTRS`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attributes: Vec<AttrChange>,
    /// Attribute changes before the cap at [`MAX_ATTRS`].
    #[serde(default)]
    pub attributes_total: usize,
}

impl ResourceChange {
    /// Paths whose change forces the replacement.
    #[must_use]
    pub fn replacement_causes(&self) -> Vec<&str> {
        self.attributes
            .iter()
            .filter(|a| a.forces_replacement)
            .map(|a| a.path.as_str())
            .collect()
    }
}

/// A root module output that changes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct OutputChange {
    /// Output name.
    pub name: String,
    /// What happens to it.
    pub action: PlanAction,
    /// The value before.
    #[serde(default)]
    pub before: AttrValue,
    /// The value after.
    #[serde(default)]
    pub after: AttrValue,
}

/// Where the plan came from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PlanSource {
    /// `terraform show -json <planfile>`: attribute values included.
    #[default]
    Show,
    /// The `planned_change` lines of `terraform plan -json`: actions only.
    Stream,
}

/// A Terraform plan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TerraformPlanSpec {
    /// Caption; the file name when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Workspace path of the file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Which JSON it was.
    #[serde(default)]
    pub source: PlanSource,
    /// Terraform (or OpenTofu) version that wrote it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terraform_version: Option<String>,
    /// Resources that change, in the plan's order.
    pub resources: Vec<ResourceChange>,
    /// Resources with nothing to do.
    #[serde(default)]
    pub unchanged: usize,
    /// Addresses changed outside Terraform since the last apply.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub drift: Vec<String>,
    /// Outputs that change.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<OutputChange>,
    /// Planning failed part way (the plan is incomplete).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub errored: bool,
    /// Whether the plan can be applied, when the file says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applyable: Option<bool>,
}

/// Resource counts per action.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PlanCounts {
    /// To add.
    pub create: usize,
    /// To change in place.
    pub update: usize,
    /// To destroy.
    pub delete: usize,
    /// To replace.
    pub replace: usize,
    /// Data sources to read.
    pub read: usize,
    /// To forget.
    pub forget: usize,
}

impl TerraformPlanSpec {
    /// Checks the size and that no resource is a no-op.
    ///
    /// # Errors
    ///
    /// [`BlockError::Invalid`] naming the first problem.
    pub fn validate(&self) -> Result<(), BlockError> {
        if self.resources.len() > MAX_RESOURCES {
            return Err(invalid(
                BLOCK,
                format!(
                    "{} resources; at most {MAX_RESOURCES}",
                    self.resources.len()
                ),
            ));
        }
        if let Some(r) = self.resources.iter().find(|r| r.address.trim().is_empty()) {
            return Err(invalid(
                BLOCK,
                format!("a {:?} change has no address", r.action),
            ));
        }
        if let Some(r) = self
            .resources
            .iter()
            .find(|r| r.attributes.len() > MAX_ATTRS)
        {
            return Err(invalid(
                BLOCK,
                format!("{} has more than {MAX_ATTRS} attributes", r.address),
            ));
        }
        Ok(())
    }

    /// Resource counts per action.
    #[must_use]
    pub fn counts(&self) -> PlanCounts {
        let mut c = PlanCounts::default();
        for r in &self.resources {
            match r.action {
                PlanAction::Create => c.create += 1,
                PlanAction::Update => c.update += 1,
                PlanAction::Delete => c.delete += 1,
                PlanAction::Replace => c.replace += 1,
                PlanAction::Read => c.read += 1,
                PlanAction::Forget => c.forget += 1,
                PlanAction::NoOp => {}
            }
        }
        c
    }

    /// `3 to add, 1 to change, 2 to destroy` (replacements counted apart,
    /// unlike Terraform's line, which counts them in add and destroy).
    #[must_use]
    pub fn counts_text(&self) -> String {
        let c = self.counts();
        let mut parts = vec![
            format!("{} to add", c.create),
            format!("{} to change", c.update),
            format!("{} to replace", c.replace),
            format!("{} to destroy", c.delete),
        ];
        if c.read > 0 {
            parts.push(format!("{} to read", c.read));
        }
        if c.forget > 0 {
            parts.push(format!("{} to forget", c.forget));
        }
        parts.join(", ")
    }

    /// Indices of the resources with `action`, in plan order.
    #[must_use]
    pub fn with_action(&self, action: PlanAction) -> Vec<usize> {
        (0..self.resources.len())
            .filter(|&ix| self.resources[ix].action == action)
            .collect()
    }

    /// The plan as a tree table: a row per resource badged with its
    /// action, its attribute changes nested under it (before, after,
    /// replacement causes badged), then the outputs.
    #[must_use]
    pub fn to_tree(&self) -> TreeSpec {
        let column = |name: &str| TreeColumn {
            name: name.to_owned(),
            kind: ColumnKind::Text,
            unit: None,
        };
        let attr_node = |a: &AttrChange| {
            let mut values = Map::new();
            values.insert("before".into(), a.before.text().into());
            values.insert("after".into(), a.after.text().into());
            let (badge, tone) = if a.forces_replacement {
                (Some("forces replacement"), Some(BadgeTone::Error))
            } else if a.after == AttrValue::Unknown {
                (Some("known after apply"), Some(BadgeTone::Info))
            } else if a.before == AttrValue::Sensitive || a.after == AttrValue::Sensitive {
                (Some("sensitive"), Some(BadgeTone::Neutral))
            } else {
                (None, None)
            };
            TreeNode {
                label: a.path.clone(),
                badge: badge.map(str::to_owned),
                tone,
                values,
                children: Vec::new(),
            }
        };
        let mut nodes: Vec<TreeNode> = self
            .resources
            .iter()
            .map(|r| {
                let mut values = Map::new();
                let mut note = Vec::new();
                if let Some(reason) = &r.reason {
                    note.push(reason.replace('_', " "));
                }
                if let Some(from) = &r.previous_address {
                    note.push(format!("moved from {from}"));
                }
                if r.attributes_total > r.attributes.len() {
                    note.push(format!(
                        "{} of {} attributes shown",
                        r.attributes.len(),
                        r.attributes_total
                    ));
                }
                if !note.is_empty() {
                    values.insert("note".into(), note.join("; ").into());
                }
                TreeNode {
                    label: if r.data && !r.address.contains("data.") {
                        format!("data.{}", r.address)
                    } else {
                        r.address.clone()
                    },
                    badge: Some(r.action.badge().to_owned()),
                    tone: Some(r.action.tone()),
                    values,
                    children: r.attributes.iter().map(attr_node).collect(),
                }
            })
            .collect();
        if !self.outputs.is_empty() {
            nodes.push(TreeNode {
                label: "outputs".to_owned(),
                badge: Some(format!("{}", self.outputs.len())),
                tone: Some(BadgeTone::Neutral),
                values: Map::new(),
                children: self
                    .outputs
                    .iter()
                    .map(|o| {
                        let mut node = attr_node(&AttrChange {
                            path: o.name.clone(),
                            before: o.before.clone(),
                            after: o.after.clone(),
                            forces_replacement: false,
                        });
                        node.badge = Some(o.action.badge().to_owned());
                        node.tone = Some(o.action.tone());
                        node
                    })
                    .collect(),
            });
        }
        TreeSpec {
            title: None,
            columns: vec![column("before"), column("after"), column("note")],
            nodes,
            expand_depth: if self.resources.len() <= 12 { 2 } else { 1 },
        }
    }

    /// One CSV line per attribute change (and per resource without
    /// any): address, action, path, before, after.
    #[must_use]
    pub fn to_csv(&self) -> String {
        let field = |s: &str| {
            if s.contains([',', '"', '\n']) {
                format!("\"{}\"", s.replace('"', "\"\""))
            } else {
                s.to_owned()
            }
        };
        let mut out = String::from("address,action,path,before,after,forces_replacement\n");
        for r in &self.resources {
            if r.attributes.is_empty() {
                out.push_str(&format!("{},{},,,,\n", field(&r.address), r.action.verb()));
            }
            for a in &r.attributes {
                out.push_str(&format!(
                    "{},{},{},{},{},{}\n",
                    field(&r.address),
                    r.action.verb(),
                    field(&a.path),
                    field(&a.before.text()),
                    field(&a.after.text()),
                    a.forces_replacement
                ));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn spec() -> TerraformPlanSpec {
        let change = |path: &str, before, after, forces| AttrChange {
            path: path.into(),
            before,
            after,
            forces_replacement: forces,
        };
        TerraformPlanSpec {
            title: None,
            path: Some("plan.json".into()),
            source: PlanSource::Show,
            terraform_version: Some("1.9.0".into()),
            resources: vec![
                ResourceChange {
                    address: "aws_instance.web".into(),
                    kind: Some("aws_instance".into()),
                    data: false,
                    provider: None,
                    action: PlanAction::Replace,
                    reason: Some("replace_because_cannot_update".into()),
                    previous_address: None,
                    attributes: vec![
                        change(
                            "ami",
                            AttrValue::Known(json!("ami-1")),
                            AttrValue::Known(json!("ami-2")),
                            true,
                        ),
                        change(
                            "id",
                            AttrValue::Known(json!("i-1")),
                            AttrValue::Unknown,
                            false,
                        ),
                    ],
                    attributes_total: 2,
                },
                ResourceChange {
                    address: "aws_db_instance.main".into(),
                    kind: None,
                    data: false,
                    provider: None,
                    action: PlanAction::Update,
                    reason: None,
                    previous_address: None,
                    attributes: vec![change(
                        "password",
                        AttrValue::Sensitive,
                        AttrValue::Sensitive,
                        false,
                    )],
                    attributes_total: 1,
                },
            ],
            unchanged: 4,
            drift: Vec::new(),
            outputs: vec![OutputChange {
                name: "ip".into(),
                action: PlanAction::Create,
                before: AttrValue::Absent,
                after: AttrValue::Unknown,
            }],
            errored: false,
            applyable: Some(true),
        }
    }

    #[test]
    fn actions_counts_and_tree() {
        assert_eq!(
            PlanAction::from_actions(&["create", "delete"]),
            PlanAction::Replace
        );
        assert_eq!(PlanAction::from_actions(&["no-op"]), PlanAction::NoOp);
        let spec = spec();
        spec.validate().unwrap();
        assert_eq!(
            spec.counts_text(),
            "0 to add, 1 to change, 1 to replace, 0 to destroy"
        );
        assert_eq!(spec.resources[0].replacement_causes(), ["ami"]);
        let tree = spec.to_tree();
        tree.validate().unwrap();
        assert_eq!(tree.nodes.len(), 3);
        assert_eq!(tree.nodes[0].badge.as_deref(), Some("-/+ replace"));
        assert_eq!(tree.nodes[0].tone, Some(BadgeTone::Error));
        let ami = &tree.nodes[0].children[0];
        assert_eq!(ami.badge.as_deref(), Some("forces replacement"));
        assert_eq!(
            ami.values.get("after").and_then(Value::as_str),
            Some("\"ami-2\"")
        );
        assert_eq!(
            tree.nodes[1].children[0]
                .values
                .get("after")
                .and_then(Value::as_str),
            Some("(sensitive value)")
        );
        assert_eq!(tree.nodes[2].label, "outputs");
        let csv = spec.to_csv();
        assert!(
            csv.contains("aws_instance.web,replace,ami,\"\"\"ami-1\"\"\",\"\"\"ami-2\"\"\",true"),
            "{csv}"
        );
    }
}
