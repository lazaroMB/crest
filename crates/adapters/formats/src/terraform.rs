//! Terraform (and OpenTofu) plans into a [`TerraformPlanSpec`].
//!
//! Two inputs: the JSON of `terraform show -json <planfile>` (one object
//! with `resource_changes`), and the machine-readable UI of `terraform
//! plan -json` (JSON lines; only `planned_change` and `resource_drift`
//! messages are used, which name the action but carry no values).
//!
//! Attribute differences come from walking `before` and `after` side by
//! side down to scalars (and empty lists and maps), guided by
//! `before_sensitive` / `after_sensitive` (a `true` there means the value
//! is sensitive and is not copied) and `after_unknown` (`true` means
//! known after apply). `replace_paths` marks the paths that force a
//! replacement.

use domain::devops::terraform::{
    AttrChange, AttrValue, MAX_ATTRS, MAX_RESOURCES, OutputChange, PlanAction, PlanSource,
    ResourceChange, TerraformPlanSpec,
};
use serde_json::{Map, Value};

/// Parses a plan.
///
/// # Errors
///
/// A message for malformed JSON or JSON that is not a plan.
pub fn parse(bytes: &[u8], path: &str) -> Result<TerraformPlanSpec, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| format!("`{path}` is not UTF-8 text"))?;
    let text = text.trim_start_matches('\u{feff}').trim();
    match serde_json::from_str::<Value>(text) {
        Ok(Value::Object(plan))
            if plan.contains_key("resource_changes")
                || plan.contains_key("planned_values")
                || plan.contains_key("format_version") =>
        {
            Ok(show(&plan, path))
        }
        Ok(Value::Object(line)) if line.contains_key("@level") => stream(text, path),
        Ok(_) => Err(not_a_plan(path)),
        // Several JSON values: the streamed UI.
        Err(_) if text.starts_with('{') && text.contains("\"@level\"") => stream(text, path),
        Err(e) => Err(format!("`{path}` is not valid JSON: {e}")),
    }
}

fn not_a_plan(path: &str) -> String {
    format!(
        "`{path}` is not a Terraform plan in JSON; save one with `terraform plan -out=tfplan` \
         and `terraform show -json tfplan > plan.json`"
    )
}

fn actions(change: &Value) -> PlanAction {
    let actions: Vec<&str> = change
        .get("actions")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    PlanAction::from_actions(&actions)
}

fn string(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// `terraform show -json`.
fn show(plan: &Map<String, Value>, path: &str) -> TerraformPlanSpec {
    let empty = Vec::new();
    let changes = plan
        .get("resource_changes")
        .and_then(Value::as_array)
        .unwrap_or(&empty);
    let mut resources = Vec::new();
    let mut unchanged = 0;
    for rc in changes {
        let Some(change) = rc.get("change") else {
            continue;
        };
        let action = actions(change);
        if action == PlanAction::NoOp {
            unchanged += 1;
            continue;
        }
        if resources.len() >= MAX_RESOURCES {
            continue;
        }
        let mut attributes = Vec::new();
        let replace_paths: Vec<String> = change
            .get("replace_paths")
            .and_then(Value::as_array)
            .map(|paths| paths.iter().filter_map(replace_path).collect())
            .unwrap_or_default();
        let null = Value::Null;
        let side = |key: &str| change.get(key).unwrap_or(&null);
        diff(
            &mut String::new(),
            Side {
                value: Some(side("before")).filter(|v| !v.is_null()),
                sensitive: side("before_sensitive"),
            },
            Side {
                value: Some(side("after")).filter(|v| !v.is_null()),
                sensitive: side("after_sensitive"),
            },
            side("after_unknown"),
            action,
            &mut attributes,
        );
        for a in &mut attributes {
            a.forces_replacement = replace_paths.iter().any(|p| {
                a.path == *p
                    || a.path.starts_with(&format!("{p}."))
                    || a.path.starts_with(&format!("{p}["))
            });
        }
        let total = attributes.len();
        // Replacement causes first, then the rest in path order.
        attributes.sort_by_key(|a| std::cmp::Reverse(a.forces_replacement));
        attributes.truncate(MAX_ATTRS);
        resources.push(ResourceChange {
            address: string(rc, "address").unwrap_or_default(),
            kind: string(rc, "type"),
            data: rc.get("mode").and_then(Value::as_str) == Some("data"),
            provider: string(rc, "provider_name"),
            action,
            reason: string(rc, "action_reason"),
            previous_address: string(rc, "previous_address"),
            attributes,
            attributes_total: total,
        });
    }
    let drift = plan
        .get("resource_drift")
        .and_then(Value::as_array)
        .map(|d| {
            d.iter()
                .filter(|r| {
                    r.get("change")
                        .is_some_and(|c| actions(c) != PlanAction::NoOp)
                })
                .filter_map(|r| string(r, "address"))
                .collect()
        })
        .unwrap_or_default();
    let outputs = plan
        .get("output_changes")
        .and_then(Value::as_object)
        .map(|outputs| {
            outputs
                .iter()
                .filter_map(|(name, change)| {
                    let action = actions(change);
                    (action != PlanAction::NoOp).then(|| OutputChange {
                        name: name.clone(),
                        action,
                        before: leaf(
                            change.get("before"),
                            is_true(change.get("before_sensitive")),
                            false,
                        ),
                        after: leaf(
                            change.get("after"),
                            is_true(change.get("after_sensitive")),
                            is_true(change.get("after_unknown")),
                        ),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    TerraformPlanSpec {
        title: None,
        path: Some(path.to_owned()),
        source: PlanSource::Show,
        terraform_version: plan
            .get("terraform_version")
            .and_then(Value::as_str)
            .map(str::to_owned),
        resources,
        unchanged,
        drift,
        outputs,
        errored: plan
            .get("errored")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        applyable: plan.get("applyable").and_then(Value::as_bool),
    }
}

fn is_true(v: Option<&Value>) -> bool {
    v.and_then(Value::as_bool).unwrap_or(false)
}

/// A `replace_paths` entry (`["ingress", 0, "port"]`) as a flat path.
fn replace_path(steps: &Value) -> Option<String> {
    let mut out = String::new();
    for step in steps.as_array()? {
        match step {
            Value::Number(n) => out.push_str(&format!("[{n}]")),
            Value::String(key) => push_key(&mut out, key),
            _ => return None,
        }
    }
    Some(out)
}

/// Appends `key` to a path: `.key`, or `["a.b"]` when it is not a plain
/// identifier.
fn push_key(path: &mut String, key: &str) {
    let plain = !key.is_empty()
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if plain {
        if !path.is_empty() {
            path.push('.');
        }
        path.push_str(key);
    } else {
        path.push_str(&format!("[{}]", Value::String(key.to_owned())));
    }
}

/// One side of a change at some path.
#[derive(Clone, Copy)]
struct Side<'a> {
    value: Option<&'a Value>,
    /// `true` (sensitive here), or a structure to descend.
    sensitive: &'a Value,
}

impl<'a> Side<'a> {
    fn child(self, key: Key<'_>) -> Side<'a> {
        Side {
            value: self.value.and_then(|v| key.get(v)).filter(|v| !v.is_null()),
            sensitive: key.get(self.sensitive).unwrap_or(&Value::Null),
        }
    }

    fn sensitive_here(self) -> bool {
        self.sensitive.as_bool() == Some(true)
    }
}

#[derive(Clone, Copy)]
enum Key<'k> {
    Name(&'k str),
    Index(usize),
}

impl Key<'_> {
    fn get(self, v: &Value) -> Option<&Value> {
        match self {
            Key::Name(k) => v.get(k),
            Key::Index(i) => v.get(i),
        }
    }
}

fn leaf(value: Option<&Value>, sensitive: bool, unknown: bool) -> AttrValue {
    if sensitive {
        AttrValue::Sensitive
    } else if unknown {
        AttrValue::Unknown
    } else {
        match value {
            None | Some(Value::Null) => AttrValue::Absent,
            Some(v) => AttrValue::Known(v.clone()),
        }
    }
}

fn container(v: Option<&Value>) -> bool {
    match v {
        Some(Value::Object(m)) => !m.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        _ => false,
    }
}

/// Walks both sides at `path`, pushing a change for each differing
/// scalar (every set one for a create, a destroy or a read).
fn diff(
    path: &mut String,
    before: Side<'_>,
    after: Side<'_>,
    unknown: &Value,
    action: PlanAction,
    out: &mut Vec<AttrChange>,
) {
    if out.len() > MAX_ATTRS * 4 {
        return;
    }
    let unknown_here = unknown.as_bool() == Some(true);
    let at_leaf = before.sensitive_here()
        || after.sensitive_here()
        || unknown_here
        || !(container(before.value) || container(after.value));
    if at_leaf {
        let b = leaf(before.value, before.sensitive_here(), false);
        let a = leaf(after.value, after.sensitive_here(), unknown_here);
        let differs = match action {
            PlanAction::Create | PlanAction::Read => a != AttrValue::Absent,
            PlanAction::Delete | PlanAction::Forget => b != AttrValue::Absent,
            // Sensitive on either side: say it may change, as Terraform
            // does; the values themselves are not compared.
            _ => b != a || a == AttrValue::Sensitive && before.value != after.value,
        };
        if differs && !path.is_empty() {
            out.push(AttrChange {
                path: path.clone(),
                before: b,
                after: a,
                forces_replacement: false,
            });
        }
        return;
    }
    let len = path.len();
    let arrays = |s: Side<'_>| s.value.and_then(Value::as_array).map_or(0, Vec::len);
    let is_array =
        before.value.is_some_and(Value::is_array) || after.value.is_some_and(Value::is_array);
    if is_array {
        for i in 0..arrays(before).max(arrays(after)) {
            path.push_str(&format!("[{i}]"));
            diff(
                path,
                before.child(Key::Index(i)),
                after.child(Key::Index(i)),
                unknown.get(i).unwrap_or(&Value::Null),
                action,
                out,
            );
            path.truncate(len);
        }
        return;
    }
    let mut keys: Vec<&str> = Vec::new();
    for side in [before.value, after.value].into_iter().flatten() {
        if let Some(map) = side.as_object() {
            for k in map.keys() {
                if !keys.contains(&k.as_str()) {
                    keys.push(k);
                }
            }
        }
    }
    if let Some(map) = unknown.as_object() {
        for k in map.keys() {
            if !keys.contains(&k.as_str()) {
                keys.push(k);
            }
        }
    }
    keys.sort_unstable();
    for key in keys {
        push_key(path, key);
        diff(
            path,
            before.child(Key::Name(key)),
            after.child(Key::Name(key)),
            unknown.get(key).unwrap_or(&Value::Null),
            action,
            out,
        );
        path.truncate(len);
    }
}

/// `terraform plan -json`: the `planned_change` and `resource_drift`
/// lines.
fn stream(text: &str, path: &str) -> Result<TerraformPlanSpec, String> {
    let mut resources = Vec::new();
    let mut drift = Vec::new();
    let mut version = None;
    let mut errored = false;
    let mut seen = false;
    for line in text.lines().map(str::trim).filter(|l| l.starts_with('{')) {
        let Ok(message) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        seen = true;
        if version.is_none() {
            version = string(&message, "terraform").or_else(|| string(&message, "tofu"));
        }
        if message.get("@level").and_then(Value::as_str) == Some("error") {
            errored = true;
        }
        let kind = message.get("type").and_then(Value::as_str).unwrap_or("");
        let Some(change) = message.get("change") else {
            continue;
        };
        let address = change
            .get("resource")
            .and_then(|r| string(r, "addr"))
            .unwrap_or_default();
        let action = match change.get("action").and_then(Value::as_str) {
            Some("create") => PlanAction::Create,
            Some("update") => PlanAction::Update,
            Some("delete") => PlanAction::Delete,
            Some("replace") => PlanAction::Replace,
            Some("read") => PlanAction::Read,
            Some("remove" | "forget") => PlanAction::Forget,
            _ => PlanAction::NoOp,
        };
        match kind {
            "planned_change" if action != PlanAction::NoOp && resources.len() < MAX_RESOURCES => {
                resources.push(ResourceChange {
                    data: address.starts_with("data.") || address.contains(".data."),
                    address,
                    kind: change
                        .get("resource")
                        .and_then(|r| string(r, "resource_type")),
                    provider: None,
                    action,
                    reason: string(change, "reason"),
                    previous_address: change
                        .get("previous_resource")
                        .and_then(|r| string(r, "addr")),
                    attributes: Vec::new(),
                    attributes_total: 0,
                });
            }
            "resource_drift" if !address.is_empty() => drift.push(address),
            _ => {}
        }
    }
    if !seen {
        return Err(not_a_plan(path));
    }
    Ok(TerraformPlanSpec {
        title: None,
        path: Some(path.to_owned()),
        source: PlanSource::Stream,
        terraform_version: version,
        resources,
        unchanged: 0,
        drift,
        outputs: Vec::new(),
        errored,
        applyable: None,
    })
}
