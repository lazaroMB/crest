//! [`ImageGuardSink`]: checks the image files that plugins name before the
//! front end loads them.
//!
//! A plugin can put `app.ui.image{ path = ... }` in a modal or a `Custom`
//! block, and the UI reads that file. So that this cannot show files the
//! file tools would refuse, every such path is resolved with the
//! workspace guard: it must be inside the workspace (symlinks followed)
//! and must not match the sensitive-path rules. An accepted path is
//! replaced with its canonical form; a rejected image becomes a text node
//! saying why. Other events pass through untouched.

use std::sync::{Arc, PoisonError, RwLock};

use adapter_fs::WorkspaceGuard;
use application::ports::events::{DomainEvent, EventSink};
use domain::block::{BlockKind, CustomNode};

/// The guard the sink checks with; the composition root swaps in the one
/// with the configured sensitive-path rules once the config is loaded.
pub type SharedGuard = Arc<RwLock<WorkspaceGuard>>;

/// Forwards events to `inner` after checking their image paths.
pub struct ImageGuardSink {
    inner: Arc<dyn EventSink>,
    guard: SharedGuard,
}

impl ImageGuardSink {
    /// A sink over `inner` checking with `guard`.
    #[must_use]
    pub fn new(inner: Arc<dyn EventSink>, guard: SharedGuard) -> Self {
        Self { inner, guard }
    }
}

/// The tree of `event` that may name image files, if any.
fn tree_of(event: &mut DomainEvent) -> Option<&mut CustomNode> {
    match event {
        DomainEvent::ModalOpened { spec } => Some(&mut spec.content),
        DomainEvent::ModalUpdated { content, .. } => Some(content),
        DomainEvent::BlockAdded { block } | DomainEvent::BlockUpdated { block } => {
            match &mut block.kind {
                BlockKind::Custom { tree, .. } => Some(tree),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Checks every image path of `event` with `guard`.
pub fn check_images(guard: &WorkspaceGuard, mut event: DomainEvent) -> DomainEvent {
    if let Some(tree) = tree_of(&mut event)
        && tree.has_image_paths()
    {
        tree.check_image_paths(&mut |path| {
            guard
                .resolve_readable_blocking(path)
                .map(|(_, canonical)| canonical.display().to_string())
                .map_err(|err| err.to_string())
        });
    }
    event
}

impl EventSink for ImageGuardSink {
    fn emit(&self, event: DomainEvent) {
        let event = {
            let guard = self.guard.read().unwrap_or_else(PoisonError::into_inner);
            check_images(&guard, event)
        };
        self.inner.emit(event);
    }
}

#[cfg(test)]
mod tests {
    use domain::block::{Block, ImageData, ImageFit};
    use domain::modal::{ModalId, ModalSpec};
    use domain::workspace::SensitivePathRules;

    use super::*;

    fn image(path: &str) -> CustomNode {
        CustomNode::Image {
            source: ImageData::Path { path: path.into() },
            media_type: None,
            alt: None,
            fit: ImageFit::Contain,
        }
    }

    fn texts(tree: &CustomNode) -> Vec<String> {
        let CustomNode::Col { children } = tree else {
            panic!("{tree:?}")
        };
        children
            .iter()
            .map(|child| match child {
                CustomNode::Image {
                    source: ImageData::Path { path },
                    ..
                } => format!("image {path}"),
                CustomNode::Text { text } => text.clone(),
                other => format!("{other:?}"),
            })
            .collect()
    }

    #[test]
    fn modal_and_block_images_are_resolved_or_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let base = std::fs::canonicalize(dir.path()).unwrap();
        let ws = base.join("ws");
        std::fs::create_dir_all(ws.join("out")).unwrap();
        std::fs::write(ws.join("out/plot.png"), "png").unwrap();
        std::fs::write(ws.join("id_rsa.png"), "key").unwrap();
        std::fs::write(base.join("outside.png"), "x").unwrap();
        let guard = WorkspaceGuard::open(&ws)
            .unwrap()
            .with_sensitive_rules(SensitivePathRules::new(["id_rsa*"]).unwrap());
        let tree = CustomNode::Col {
            children: vec![
                image("out/plot.png"),
                image("../outside.png"),
                image("id_rsa.png"),
                image("missing.png"),
            ],
        };
        let event = DomainEvent::ModalOpened {
            spec: ModalSpec {
                id: ModalId::new("p/a"),
                plugin: "p".into(),
                title: "t".into(),
                width: domain::modal::Percent::default(),
                height: domain::modal::Percent::default(),
                position: domain::modal::ModalPosition::default(),
                content: tree.clone(),
            },
        };
        let DomainEvent::ModalOpened { spec } = check_images(&guard, event) else {
            panic!("not a modal")
        };
        let got = texts(&spec.content);
        assert_eq!(
            got[0],
            format!("image {}", ws.join("out/plot.png").display())
        );
        for (ix, name) in [(1, "../outside.png"), (2, "id_rsa.png"), (3, "missing.png")] {
            assert!(
                got[ix].starts_with(&format!("[image unavailable: {name}: ")),
                "{}",
                got[ix]
            );
        }

        let block = Block::new(BlockKind::Custom {
            plugin: "p".into(),
            name: "b".into(),
            tree,
        });
        let DomainEvent::BlockAdded { block } =
            check_images(&guard, DomainEvent::BlockAdded { block })
        else {
            panic!("not a block")
        };
        let BlockKind::Custom { tree, .. } = block.kind else {
            panic!("not custom")
        };
        assert_eq!(
            texts(&tree)[0],
            format!("image {}", ws.join("out/plot.png").display())
        );
    }
}
