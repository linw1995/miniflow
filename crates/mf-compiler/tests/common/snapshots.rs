#![allow(dead_code)]
//! Test-owned snapshot collection; the execution runtime only supplies events.
use mf_runtime::{NodeSnapshot, SnapshotEvent, SnapshotOutcome, SnapshotRecorder};
use mf_telemetry::{
    capture::{SnapshotPublishError, SnapshotPublisher},
    event::LoopPathEntry,
};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

#[derive(Clone, Default)]
pub struct Snapshot(BTreeMap<Vec<LoopPathEntry>, BTreeMap<Arc<str>, Arc<NodeSnapshot>>>);
impl Snapshot {
    pub fn node(&self, scope: &[LoopPathEntry], node: &str) -> Option<&Arc<NodeSnapshot>> {
        self.0.get(scope)?.get(node)
    }
}
#[derive(Clone)]
pub struct Entry {
    pub scope: Vec<LoopPathEntry>,
    pub node: Arc<str>,
    pub snapshot: Snapshot,
}
#[derive(Default)]
pub struct SnapshotStore {
    current: Snapshot,
    history: Vec<Entry>,
}
impl SnapshotStore {
    pub fn current(&self) -> Snapshot {
        self.current.clone()
    }
    pub fn history(&self) -> Vec<Entry> {
        self.history.clone()
    }
    fn push(&mut self, event: SnapshotEvent) {
        let SnapshotEvent {
            scope,
            node,
            snapshot,
        } = event;
        let equal = self
            .current
            .node(&scope, &node)
            .is_some_and(|previous| previous.as_ref() == &snapshot);
        if equal
            && !matches!(
                snapshot.outcome,
                SnapshotOutcome::Started | SnapshotOutcome::Emitted
            )
        {
            return;
        }
        self.current
            .0
            .entry(scope.clone())
            .or_default()
            .insert(node.clone(), Arc::new(snapshot));
        self.history.push(Entry {
            scope,
            node,
            snapshot: self.current.clone(),
        });
    }
}
#[derive(Clone)]
pub struct Snapshots {
    store: Arc<Mutex<SnapshotStore>>,
    publisher: Option<SnapshotPublisher>,
}
impl Snapshots {
    pub fn memory() -> Self {
        Self {
            store: Arc::default(),
            publisher: None,
        }
    }
    pub fn with_sink(
        sink: impl FnMut(serde_json::Value) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
        + Send
        + 'static,
    ) -> Result<Self, SnapshotPublishError> {
        Ok(Self {
            store: Arc::default(),
            publisher: Some(SnapshotPublisher::new(sink)?),
        })
    }
    pub fn recorder(&self) -> SnapshotRecorder {
        let capture = self.clone();
        SnapshotRecorder::new(move |event| {
            if let Some(publisher) = &capture.publisher {
                publisher.record(event);
            } else {
                capture.store.lock().unwrap().push(event);
            }
        })
    }
    pub fn current(&self) -> Snapshot {
        self.store.lock().unwrap().current()
    }
    pub fn history(&self) -> Vec<Entry> {
        self.store.lock().unwrap().history()
    }
    pub fn finish(&self) {
        if let Some(publisher) = &self.publisher {
            publisher.finish();
        }
    }
    pub fn diagnostic(&self) -> Option<Arc<SnapshotPublishError>> {
        self.publisher
            .as_ref()
            .and_then(SnapshotPublisher::diagnostic)
    }
}
