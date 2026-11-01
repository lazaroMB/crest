//! In-memory fakes of the discovery ports.

use std::collections::BTreeMap;
use std::sync::{Mutex, PoisonError};

use async_trait::async_trait;
use domain::discovery::{DiscoveryEvent, Level, Logged, SourceId};

use crate::ports::discovery::{
    Appended, DiscoveryId, DiscoveryLogError, DiscoveryLogPort, DiscoverySummary, SourceStoreError,
    SourceStorePort, SourceText, StoredSource, judge, text_hash,
};

#[derive(Debug, Default, Clone)]
struct Entry {
    workspace_root: String,
    events: Vec<Logged>,
    artifacts: BTreeMap<String, String>,
}

/// A [`DiscoveryLogPort`] in memory.
#[derive(Debug, Default)]
pub struct MemoryDiscoveryLog {
    entries: Mutex<BTreeMap<DiscoveryId, Entry>>,
}

impl MemoryDiscoveryLog {
    /// An empty log store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn with<T>(&self, f: impl FnOnce(&mut BTreeMap<DiscoveryId, Entry>) -> T) -> T {
        let mut guard = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        f(&mut guard)
    }

    /// The events of every discovery, for assertions.
    #[must_use]
    pub fn events(&self, id: &DiscoveryId) -> Vec<Logged> {
        self.with(|m| m.get(id).map(|e| e.events.clone()).unwrap_or_default())
    }
}

#[async_trait]
impl DiscoveryLogPort for MemoryDiscoveryLog {
    async fn start(
        &self,
        id: &DiscoveryId,
        workspace_root: &str,
        problem: &str,
        level: Level,
        at: u64,
    ) -> Result<Logged, DiscoveryLogError> {
        self.with(|m| {
            if m.contains_key(id) {
                return Err(DiscoveryLogError::AlreadyExists(id.clone()));
            }
            let logged = Logged::new(
                1,
                at,
                DiscoveryEvent::Started {
                    problem: problem.to_owned(),
                    level,
                },
            );
            m.insert(
                id.clone(),
                Entry {
                    workspace_root: workspace_root.to_owned(),
                    events: vec![logged.clone()],
                    artifacts: BTreeMap::new(),
                },
            );
            Ok(logged)
        })
    }

    async fn append(
        &self,
        id: &DiscoveryId,
        event: DiscoveryEvent,
        at: u64,
    ) -> Result<Logged, DiscoveryLogError> {
        self.with(|m| {
            let entry = m
                .get_mut(id)
                .ok_or_else(|| DiscoveryLogError::NotFound(id.clone()))?;
            match judge(&entry.events, event, at)
                .map_err(|e| DiscoveryLogError::Corrupt(id.clone(), e.to_string()))?
            {
                Appended::Accepted(logged) => {
                    entry.events.push(logged.clone());
                    Ok(logged)
                }
                Appended::Refused { refusal, reason } => {
                    entry.events.push(refusal);
                    Err(DiscoveryLogError::Illegal {
                        id: id.clone(),
                        reason,
                        logged: true,
                    })
                }
                Appended::Frozen(reason) => Err(DiscoveryLogError::Illegal {
                    id: id.clone(),
                    reason,
                    logged: false,
                }),
            }
        })
    }

    async fn load(&self, id: &DiscoveryId) -> Result<Vec<Logged>, DiscoveryLogError> {
        self.with(|m| {
            m.get(id)
                .map(|e| e.events.clone())
                .ok_or_else(|| DiscoveryLogError::NotFound(id.clone()))
        })
    }

    async fn list(&self, workspace_root: &str) -> Result<Vec<DiscoverySummary>, DiscoveryLogError> {
        self.with(|m| {
            let mut out = Vec::new();
            for (id, entry) in m.iter().filter(|(_, e)| e.workspace_root == workspace_root) {
                // As the SQLite store: a log that does not replay is left out.
                if let Ok(summary) =
                    DiscoverySummary::from_events(id.clone(), workspace_root, &entry.events)
                {
                    out.push(summary);
                }
            }
            out.sort_by(|a, b| b.started_at.cmp(&a.started_at).then(b.id.cmp(&a.id)));
            Ok(out)
        })
    }

    async fn put_artifact(
        &self,
        id: &DiscoveryId,
        name: &str,
        text: &str,
    ) -> Result<(), DiscoveryLogError> {
        self.with(|m| {
            let entry = m
                .get_mut(id)
                .ok_or_else(|| DiscoveryLogError::NotFound(id.clone()))?;
            entry.artifacts.insert(name.to_owned(), text.to_owned());
            Ok(())
        })
    }

    async fn artifact(
        &self,
        id: &DiscoveryId,
        name: &str,
    ) -> Result<Option<String>, DiscoveryLogError> {
        self.with(|m| {
            let entry = m
                .get(id)
                .ok_or_else(|| DiscoveryLogError::NotFound(id.clone()))?;
            Ok(entry.artifacts.get(name).cloned())
        })
    }

    async fn artifacts(&self, id: &DiscoveryId) -> Result<Vec<String>, DiscoveryLogError> {
        self.with(|m| {
            let entry = m
                .get(id)
                .ok_or_else(|| DiscoveryLogError::NotFound(id.clone()))?;
            Ok(entry.artifacts.keys().cloned().collect())
        })
    }
}

/// A [`SourceStorePort`] in memory.
#[derive(Debug, Default)]
pub struct MemorySourceStore {
    sources: Mutex<BTreeMap<DiscoveryId, Vec<StoredSource>>>,
}

impl MemorySourceStore {
    /// An empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl SourceStorePort for MemorySourceStore {
    async fn put(
        &self,
        discovery: &DiscoveryId,
        source: SourceText,
    ) -> Result<StoredSource, SourceStoreError> {
        let mut guard = self.sources.lock().unwrap_or_else(PoisonError::into_inner);
        let list = guard.entry(discovery.clone()).or_default();
        let next = list.iter().map(|s| s.id.0).max().unwrap_or(0) + 1;
        let stored = StoredSource {
            id: SourceId(next),
            hash: text_hash(&source.text),
            source,
        };
        list.push(stored.clone());
        Ok(stored)
    }

    async fn get(
        &self,
        discovery: &DiscoveryId,
        id: SourceId,
    ) -> Result<StoredSource, SourceStoreError> {
        let guard = self.sources.lock().unwrap_or_else(PoisonError::into_inner);
        guard
            .get(discovery)
            .and_then(|l| l.iter().find(|s| s.id == id).cloned())
            .ok_or_else(|| SourceStoreError::NotFound(discovery.clone(), id))
    }

    async fn sources(
        &self,
        discovery: &DiscoveryId,
    ) -> Result<Vec<StoredSource>, SourceStoreError> {
        let guard = self.sources.lock().unwrap_or_else(PoisonError::into_inner);
        Ok(guard.get(discovery).cloned().unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use domain::discovery::{Section, SourceKind, Status};

    use super::*;

    fn id() -> DiscoveryId {
        DiscoveryId::new("0001-p")
    }

    #[tokio::test]
    async fn the_log_starts_appends_refuses_and_lists() {
        let log = MemoryDiscoveryLog::new();
        log.start(&id(), "/ws", "slow", Level::Research, 1)
            .await
            .unwrap();
        assert_eq!(
            log.start(&id(), "/ws", "slow", Level::Research, 1)
                .await
                .unwrap_err(),
            DiscoveryLogError::AlreadyExists(id())
        );
        let written = log
            .append(
                &id(),
                DiscoveryEvent::SectionWritten {
                    section: Section::Goal,
                    hash: "h".into(),
                },
                2,
            )
            .await
            .unwrap();
        assert_eq!(written.seq, 2);
        // Writing a section that is not open is refused and logged.
        let err = log
            .append(
                &id(),
                DiscoveryEvent::SectionWritten {
                    section: Section::Scope,
                    hash: "h".into(),
                },
                3,
            )
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            DiscoveryLogError::Illegal { logged: true, .. }
        ));
        let events = log.load(&id()).await.unwrap();
        assert_eq!(events.len(), 3);
        assert!(matches!(events[2].event, DiscoveryEvent::Refused { .. }));
        // Frozen: nothing stored.
        log.append(&id(), DiscoveryEvent::Aborted, 4).await.unwrap();
        let err = log
            .append(&id(), DiscoveryEvent::Aborted, 5)
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            DiscoveryLogError::Illegal { logged: false, .. }
        ));
        assert_eq!(log.load(&id()).await.unwrap().len(), 4);
        let list = log.list("/ws").await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].status, Status::Aborted);
        assert!(log.list("/other").await.unwrap().is_empty());
        assert_eq!(
            log.load(&DiscoveryId::new("nope")).await.unwrap_err(),
            DiscoveryLogError::NotFound(DiscoveryId::new("nope"))
        );
    }

    #[tokio::test]
    async fn artifacts_are_named_texts() {
        let log = MemoryDiscoveryLog::new();
        log.start(&id(), "/ws", "slow", Level::Deep, 1)
            .await
            .unwrap();
        assert_eq!(log.artifact(&id(), "sections/goal.md").await.unwrap(), None);
        log.put_artifact(&id(), "sections/goal.md", "Enable ...")
            .await
            .unwrap();
        log.put_artifact(&id(), "records/claims.json", "[]")
            .await
            .unwrap();
        assert_eq!(
            log.artifact(&id(), "sections/goal.md")
                .await
                .unwrap()
                .as_deref(),
            Some("Enable ...")
        );
        assert_eq!(
            log.artifacts(&id()).await.unwrap(),
            vec!["records/claims.json", "sections/goal.md"]
        );
    }

    #[tokio::test]
    async fn sources_get_ids_and_hashes() {
        let store = MemorySourceStore::new();
        let source = |text: &str| SourceText {
            location: "https://x".into(),
            title: None,
            kind: SourceKind::Study,
            text: text.into(),
            fetched_at: 1,
        };
        let a = store.put(&id(), source("one")).await.unwrap();
        let b = store.put(&id(), source("two")).await.unwrap();
        assert_eq!((a.id, b.id), (SourceId(1), SourceId(2)));
        assert_ne!(a.hash, b.hash);
        assert_eq!(
            store.get(&id(), SourceId(2)).await.unwrap().source.text,
            "two"
        );
        assert_eq!(store.sources(&id()).await.unwrap().len(), 2);
        assert_eq!(
            store.get(&id(), SourceId(3)).await.unwrap_err(),
            SourceStoreError::NotFound(id(), SourceId(3))
        );
    }
}
