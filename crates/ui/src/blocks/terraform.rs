//! Renderer of the `terraform_plan` block kind (`show_terraform_plan`):
//! a Terraform plan on the tree table widget.
//!
//! A summary line comes first: the Terraform version and the counts to
//! add (success color), change (warning), replace and destroy (danger),
//! with the unchanged count, drift and whether planning errored. Then the
//! plan ([`crate::widgets::tree`], built by
//! [`TerraformPlanSpec::to_tree`]): a row per resource badged with its
//! action, its attribute diff nested under it with before and after
//! values, sensitive values masked, unknown ones "known after apply" and
//! replacement causes badged.

use std::rc::Rc;

use domain::block::{BlockId, BlockKind};
use domain::devops::terraform::{PlanSource, TerraformPlanSpec};
use domain::widget::tree::TreeSpec;
use gpui::{
    AnyElement, App, Context, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    Styled as _, Window, div, prelude::FluentBuilder as _,
};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};

use super::custom::node_id;
use super::trace::caption;
use crate::timeline::TimelineEntry;
use crate::widgets;

/// Per-block state: the spec and its tree.
pub struct TerraformView {
    id: BlockId,
    spec: Rc<TerraformPlanSpec>,
    tree: Rc<TreeSpec>,
}

impl TerraformView {
    fn new(id: BlockId, spec: &TerraformPlanSpec) -> Self {
        let mut tree = spec.to_tree();
        tree.title = Some(caption(
            spec.title.as_deref(),
            spec.path.as_deref(),
            "Terraform plan",
        ));
        Self {
            id,
            spec: Rc::new(spec.clone()),
            tree: Rc::new(tree),
        }
    }

    fn sync(&mut self, spec: &TerraformPlanSpec) {
        if *self.spec != *spec {
            *self = Self::new(self.id, spec);
        }
    }

    /// The tree drawn.
    pub fn tree(&self) -> &TreeSpec {
        &self.tree
    }

    fn summary(&self, cx: &App) -> AnyElement {
        let spec = &self.spec;
        let theme = cx.theme();
        let c = spec.counts();
        let count = |n: usize, what: &str, color| {
            div()
                .when(n > 0, |d| d.text_color(color))
                .child(format!("{n} to {what}"))
        };
        let mut head = String::from("Terraform");
        if let Some(version) = &spec.terraform_version {
            head.push_str(&format!(" {version}"));
        }
        let mut tail = Vec::new();
        if c.read > 0 {
            tail.push(format!("{} to read", c.read));
        }
        if spec.unchanged > 0 {
            tail.push(format!("{} unchanged", spec.unchanged));
        }
        if !spec.drift.is_empty() {
            tail.push(format!("{} drifted", spec.drift.len()));
        }
        if spec.source == PlanSource::Stream {
            tail.push("actions only".to_owned());
        }
        h_flex()
            .flex_wrap()
            .gap_2()
            .text_xs()
            .text_color(theme.muted_foreground)
            .debug_selector(|| "terraform-summary".to_owned())
            .child(head)
            .child(count(c.create, "add", theme.success))
            .child(count(c.update, "change", theme.warning))
            .child(count(c.replace, "replace", theme.danger))
            .child(count(c.delete, "destroy", theme.danger))
            .children(tail)
            .when(spec.errored, |d| {
                d.child(div().text_color(theme.danger).child("planning errored"))
            })
            .into_any_element()
    }
}

impl Render for TerraformView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tree = self.tree.clone();
        let widget = widgets::tree::render(node_id(self.id, 0), &tree, window, cx);
        v_flex()
            .w_full()
            .gap_1()
            .debug_selector(|| "terraform".to_owned())
            .child(self.summary(cx))
            .child(widget)
    }
}

/// The plan entity of block `id`, in sync with `spec`.
pub fn view(
    id: BlockId,
    spec: &TerraformPlanSpec,
    window: &mut Window,
    cx: &mut App,
) -> gpui::Entity<TerraformView> {
    let view = super::block_state(id, window, cx, |_, _| TerraformView::new(id, spec));
    view.update(cx, |view, _| view.sync(spec));
    view
}

/// Draws the block held by `entry`.
pub fn render(entry: &TimelineEntry, window: &mut Window, cx: &mut App) -> AnyElement {
    let BlockKind::TerraformPlan(spec) = &entry.block.kind else {
        return super::placeholder(entry, cx);
    };
    view(entry.block.id, spec, window, cx).into_any_element()
}

#[cfg(test)]
mod tests {
    use application::ports::events::DomainEvent;
    use domain::block::Block;
    use gpui::{TestAppContext, point, px, size};
    use serde_json::json;

    use super::*;
    use crate::timeline::TimelineView as Timeline;

    fn spec() -> TerraformPlanSpec {
        serde_json::from_value(json!({
            "path": "infra/plan.json",
            "terraform_version": "1.9.5",
            "resources": [
                {"address": "aws_instance.web", "action": "replace", "attributes_total": 1,
                 "attributes": [{"path": "ami", "before": {"kind": "known", "value": "a"},
                     "after": {"kind": "known", "value": "b"}, "forces_replacement": true}]},
                {"address": "aws_iam_role.old", "action": "delete", "attributes_total": 0}
            ],
            "unchanged": 2
        }))
        .unwrap_or_else(|e| panic!("{e}"))
    }

    #[gpui::test]
    fn draws_counts_and_the_resource_tree(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, cx| Timeline::new(cx));
        let block = Block::new(BlockKind::TerraformPlan(Box::new(spec())));
        let id = block.id;
        view.update(cx, |view, cx| {
            view.apply_all([DomainEvent::BlockAdded { block }], cx);
        });
        cx.draw(point(px(0.), px(0.)), size(px(900.), px(700.)), |_, _| {
            div().size_full().child(view.clone())
        });
        cx.run_until_parked();
        for selector in ["terraform", "terraform-summary", "tree", "tree-row-1"] {
            assert!(cx.debug_bounds(selector).is_some(), "{selector}");
        }
        let plan = cx.update(|window, cx| super::view(id, &spec(), window, cx));
        assert_eq!(
            plan.read_with(cx, |p, _| p.tree().title.clone()).as_deref(),
            Some("plan.json")
        );
        assert_eq!(plan.read_with(cx, |p, _| p.tree().nodes.len()), 2);
    }
}
