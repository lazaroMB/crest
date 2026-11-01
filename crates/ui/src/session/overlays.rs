//! The help and the `/permissions` modals.

use std::sync::Arc;

use application_core::usecases::permissions::RuleListing;
use domain::block::CustomNode;
use domain::modal::{ModalPosition, ModalSpec, Percent};
use gpui::{App, AppContext as _, Context, Entity, Focusable as _, Window};

use super::{PERMISSION_ACTION, Session};
use crate::help::{HelpSnapshot, HelpView, LocalHelp, Section, help_modal_id, parse_help_arg};
use crate::permissions::{PermissionsEvent, PermissionsView, permissions_modal_id};

impl Session {
    /// Opens the help modal: `arg` names a section or is a search (`/help
    /// tools`, `/help jira`). The controller's part arrives first.
    pub fn open_help(&mut self, arg: &str, window: &mut Window, cx: &mut Context<Self>) {
        let (section, query) = parse_help_arg(arg);
        let snapshot = self.controller.help();
        cx.spawn_in(window, async move |this, cx| {
            let snapshot = snapshot.await;
            this.update_in(cx, |this, window, cx| {
                this.show_help(snapshot, section, &query, window, cx);
            })
            .ok();
        })
        .detach();
    }

    fn show_help(
        &mut self,
        snapshot: HelpSnapshot,
        section: Option<Section>,
        query: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let keymap = self.input.read(cx).keymap().clone();
        let models = self.controller.models();
        let snapshot = snapshot.complete(&LocalHelp {
            keymap: &keymap,
            models: &models,
            permission_mode: self.controller.permission_mode(),
            turn_override: self.model_override.as_deref(),
        });
        let view = cx.new(|cx| HelpView::new(snapshot, section, query, window, cx));
        let focus = view.focus_handle(cx);
        let size = Percent::new(90.0).unwrap_or(Percent::FULL);
        let spec = ModalSpec {
            id: help_modal_id(),
            plugin: "crest".into(),
            title: "Help".into(),
            width: size,
            height: size,
            position: ModalPosition::CENTER,
            content: CustomNode::Text {
                text: String::new(),
            },
        };
        self.modal.update(cx, |modal, cx| {
            modal.open_view(spec, view.into(), window, cx)
        });
        // Keys reach the help first; Esc still bubbles up to the host.
        window.focus(&focus, cx);
    }

    /// Opens the `/permissions` modal once the controller listed the
    /// rules.
    pub fn open_permissions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rules = self.controller.permission_rules();
        cx.spawn_in(window, async move |this, cx| {
            let listing = rules.await;
            this.update_in(cx, |this, window, cx| {
                this.show_permissions(listing, window, cx);
            })
            .ok();
        })
        .detach();
    }

    fn show_permissions(
        &mut self,
        listing: RuleListing,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let keys = crate::help::action_keys(self.input.read(cx).keymap(), PERMISSION_ACTION);
        let controller = Arc::clone(&self.controller);
        let view = cx.new(|cx| PermissionsView::new(controller, listing, keys, window, cx));
        let cycle = cx.subscribe(&view, |this, view, event: &PermissionsEvent, cx| {
            let PermissionsEvent::CycleMode = event;
            this.cycle_permission_mode(cx);
            let mode = this.controller.permission_mode();
            view.update(cx, |view, cx| view.set_mode(mode, cx));
        });
        self.permissions_events = Some(cycle);
        let focus = view.focus_handle(cx);
        let spec = ModalSpec {
            id: permissions_modal_id(),
            plugin: "crest".into(),
            title: "Permissions".into(),
            width: Percent::new(70.0).unwrap_or(Percent::FULL),
            height: Percent::new(80.0).unwrap_or(Percent::FULL),
            position: ModalPosition::CENTER,
            content: CustomNode::Text {
                text: String::new(),
            },
        };
        self.modal.update(cx, |modal, cx| {
            modal.open_view(spec, view.into(), window, cx)
        });
        // Keys reach the list first; Esc still bubbles up to the host.
        window.focus(&focus, cx);
    }

    /// The `/permissions` modal's view, while it is open.
    pub fn permissions_view(&self, cx: &App) -> Option<Entity<PermissionsView>> {
        self.modal
            .read(cx)
            .view(&permissions_modal_id())
            .and_then(|view| view.clone().downcast::<PermissionsView>().ok())
    }
}
