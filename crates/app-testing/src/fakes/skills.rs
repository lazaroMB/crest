//! [`FixedSkills`]: a [`SkillsPort`] with skills set by the test.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use crate::ports::skills::{Skill, SkillMeta, SkillsPort};

/// Skills held in memory. Adding one moves the revision; the paths given
/// to [`SkillsPort::discover`] are recorded.
#[derive(Debug, Default)]
pub struct FixedSkills {
    skills: Mutex<Vec<Skill>>,
    revision: AtomicU64,
    discovered: Mutex<Vec<String>>,
}

impl FixedSkills {
    /// No skills.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds `meta` with `body`.
    #[must_use]
    pub fn with(self, meta: SkillMeta, body: &str) -> Self {
        self.add(meta, body);
        self
    }

    /// Adds `meta` with `body`, as if its file appeared.
    pub fn add(&self, meta: SkillMeta, body: &str) {
        self.skills
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Skill {
                meta,
                body: body.to_owned(),
            });
        self.revision.fetch_add(1, Ordering::AcqRel);
    }

    /// The paths given to [`SkillsPort::discover`].
    pub fn discovered(&self) -> Vec<String> {
        self.discovered
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl SkillsPort for FixedSkills {
    fn list(&self) -> Arc<[SkillMeta]> {
        self.skills
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .map(|skill| skill.meta.clone())
            .collect()
    }

    fn load(&self, name: &str) -> Option<Skill> {
        self.skills
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .find(|skill| skill.meta.name == name)
            .cloned()
    }

    fn discover(&self, path: &str) {
        self.discovered
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(path.to_owned());
    }

    fn revision(&self) -> u64 {
        self.revision.load(Ordering::Acquire)
    }
}
