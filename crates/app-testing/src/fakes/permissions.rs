//! [`PermissionStorePort`] over files kept in memory, read and written with
//! the real TOML parser and writer.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

use async_trait::async_trait;
use domain::permission::PermissionRule;

use crate::ports::permissions::{
    LoadedRules, PermissionStoreError, PermissionStorePort, parse_permissions_toml,
    permissions_toml,
};

/// The text of each workspace's `permissions.local.toml`, in memory.
#[derive(Debug, Default)]
pub struct MemoryPermissionStore {
    files: Mutex<HashMap<PathBuf, String>>,
    fail_saves: AtomicBool,
    saves: AtomicUsize,
}

impl MemoryPermissionStore {
    /// No files.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn files(&self) -> MutexGuard<'_, HashMap<PathBuf, String>> {
        self.files.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Sets the file of `workspace` to `text`.
    pub fn set_text(&self, workspace: &Path, text: &str) {
        self.files()
            .insert(workspace.to_path_buf(), text.to_owned());
    }

    /// The file of `workspace`, if it was written.
    #[must_use]
    pub fn text(&self, workspace: &Path) -> Option<String> {
        self.files().get(workspace).cloned()
    }

    /// The rules in the file of `workspace`.
    #[must_use]
    pub fn rules(&self, workspace: &Path) -> Vec<PermissionRule> {
        self.text(workspace)
            .map(|text| parse_permissions_toml(&text, "permissions.local.toml").rules)
            .unwrap_or_default()
    }

    /// Makes every save fail ("disk full"), or work again.
    pub fn fail_saves(&self, fail: bool) {
        self.fail_saves.store(fail, Ordering::Relaxed);
    }

    /// Number of successful saves.
    #[must_use]
    pub fn saves(&self) -> usize {
        self.saves.load(Ordering::Relaxed)
    }
}

#[async_trait]
impl PermissionStorePort for MemoryPermissionStore {
    async fn load(&self, workspace: &Path) -> Result<LoadedRules, PermissionStoreError> {
        Ok(self
            .text(workspace)
            .map(|text| parse_permissions_toml(&text, "permissions.local.toml"))
            .unwrap_or_default())
    }

    async fn save(
        &self,
        workspace: &Path,
        rules: &[PermissionRule],
    ) -> Result<(), PermissionStoreError> {
        if self.fail_saves.load(Ordering::Relaxed) {
            return Err(PermissionStoreError("disk full".to_owned()));
        }
        self.set_text(workspace, &permissions_toml(rules));
        self.saves.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}
