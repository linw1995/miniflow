use crate::{ValueKind, ValueRef};
use mf_telemetry::event::LoopPathEntry;
use rpds::RedBlackTreeMapSync;
use serde::{Deserialize, Serialize};
use serde_json::Number;
use snafu::{OptionExt, ResultExt, Snafu, ensure};
use std::{
    collections::{BTreeMap, HashMap},
    hash::{Hash, Hasher},
    sync::{Arc, Mutex, Weak},
};

#[derive(Debug, Snafu)]
pub enum SnapshotError {
    #[snafu(display("{message}"))]
    Invalid { message: String },
    #[snafu(display("snapshot sink failed: {source}"))]
    Sink {
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

pub const SNAPSHOT_VERSION: u32 = 1;
pub type ValueId = usize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotOutcome {
    Started,
    Succeeded,
    Failed,
    Skipped,
    Emitted,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeSnapshot {
    pub inputs: ValueRef,
    pub outputs: ValueRef,
    pub skipped: Arc<[String]>,
    pub outcome: SnapshotOutcome,
    pub error: Option<Arc<str>>,
}

impl NodeSnapshot {
    fn redact(&mut self) {
        self.inputs = self.inputs.redacted();
        self.outputs = self.outputs.redacted();
    }
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    scopes:
        RedBlackTreeMapSync<Vec<LoopPathEntry>, RedBlackTreeMapSync<Arc<str>, Arc<NodeSnapshot>>>,
}

impl Snapshot {
    pub fn node(&self, scope: &[LoopPathEntry], node: &str) -> Option<&Arc<NodeSnapshot>> {
        self.scopes.get(scope)?.get(node)
    }
    pub fn ptr_eq(&self, other: &Self) -> bool {
        self.scopes.ptr_eq(&other.scopes)
    }
}

#[derive(Clone, Debug)]
pub struct SnapshotEntry {
    pub scope: Vec<LoopPathEntry>,
    pub node: Arc<str>,
    pub snapshot: Snapshot,
}

// Only definitions contain payloads. All later records refer to their value IDs.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum ValueDefinition {
    Null,
    Bool(bool),
    Number(#[serde(with = "number_text")] Number),
    String(Arc<str>),
    Array(Vec<ValueId>),
    Object(BTreeMap<Arc<str>, ValueId>),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "record", rename_all = "snake_case", deny_unknown_fields)]
pub enum SnapshotRecord {
    Header {
        version: u32,
    },
    Value {
        id: ValueId,
        value: ValueDefinition,
    },
    Node {
        scope: Vec<LoopPathEntry>,
        node: Arc<str>,
        inputs: ValueId,
        outputs: ValueId,
        skipped: Arc<[String]>,
        outcome: SnapshotOutcome,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<Arc<str>>,
    },
    End,
}

// Shared node-state transitions for the decoder store and the producer encoder.
#[derive(Debug, Default)]
struct SnapshotState {
    current: Snapshot,
    history: Vec<SnapshotEntry>,
}

impl SnapshotState {
    fn apply_node(
        &mut self,
        scope: Vec<LoopPathEntry>,
        node: Arc<str>,
        next: NodeSnapshot,
    ) -> bool {
        if !matches!(
            next.outcome,
            SnapshotOutcome::Started | SnapshotOutcome::Emitted
        ) && self
            .current
            .node(&scope, &node)
            .is_some_and(|previous| previous.as_ref() == &next)
        {
            return false;
        }
        let mut nodes = self.current.scopes.get(&scope).cloned().unwrap_or_default();
        nodes.insert_mut(node.clone(), Arc::new(next));
        let mut scopes = self.current.scopes.clone();
        scopes.insert_mut(scope.clone(), nodes);
        self.current = Snapshot { scopes };
        self.history.push(SnapshotEntry {
            scope,
            node,
            snapshot: self.current.clone(),
        });
        true
    }
}

#[derive(Debug, Default)]
pub struct SnapshotStore {
    values: Vec<ValueRef>,
    state: SnapshotState,
    initialized: bool,
    complete: bool,
}

impl SnapshotStore {
    pub fn current(&self) -> &Snapshot {
        &self.state.current
    }
    pub fn history(&self) -> &[SnapshotEntry] {
        &self.state.history
    }
    pub fn value_count(&self) -> usize {
        self.values.len()
    }
    pub fn is_complete(&self) -> bool {
        self.complete
    }
    pub fn is_initialized(&self) -> bool {
        self.initialized
    }
    pub fn value(&self, id: ValueId) -> Option<&ValueRef> {
        self.values.get(id)
    }

    pub fn apply(&mut self, record: SnapshotRecord) -> Result<bool, SnapshotError> {
        ensure!(
            !self.complete,
            InvalidSnafu {
                message: "snapshot record followed the end marker"
            }
        );
        if let SnapshotRecord::Header { version } = record {
            ensure!(
                !self.initialized && version == SNAPSHOT_VERSION,
                InvalidSnafu {
                    message: "invalid snapshot header"
                }
            );
            self.initialized = true;
            return Ok(false);
        }
        ensure!(
            self.initialized,
            InvalidSnafu {
                message: "snapshot stream omitted its header"
            }
        );
        match record {
            SnapshotRecord::Value { id, value } => {
                ensure!(
                    id == self.values.len(),
                    InvalidSnafu {
                        message: "snapshot value IDs must be contiguous"
                    }
                );
                let reference = |id| {
                    self.values.get(id).cloned().context(InvalidSnafu {
                        message: "unknown snapshot value reference",
                    })
                };
                let value = match value {
                    ValueDefinition::Null => ValueRef::null(),
                    ValueDefinition::Bool(value) => ValueRef::from(value),
                    ValueDefinition::Number(value) => ValueRef::new(ValueKind::Number(value)),
                    ValueDefinition::String(value) => ValueRef::new(ValueKind::String(value)),
                    ValueDefinition::Array(items) => ValueRef::array(
                        items
                            .into_iter()
                            .map(reference)
                            .collect::<Result<Vec<_>, _>>()?,
                    ),
                    ValueDefinition::Object(entries) => ValueRef::object(
                        entries
                            .into_iter()
                            .map(|(key, id)| Ok((key, reference(id)?)))
                            .collect::<Result<Vec<_>, SnapshotError>>()?,
                    ),
                };
                self.values.push(value);
                Ok(false)
            }
            SnapshotRecord::Node {
                scope,
                node,
                inputs,
                outputs,
                skipped,
                outcome,
                error,
            } => {
                let inputs = self.values.get(inputs).cloned().context(InvalidSnafu {
                    message: "unknown input snapshot",
                })?;
                let outputs = self.values.get(outputs).cloned().context(InvalidSnafu {
                    message: "unknown output snapshot",
                })?;
                ensure!(
                    inputs.is_object() && outputs.is_object(),
                    InvalidSnafu {
                        message: "node snapshots require input and output objects"
                    }
                );
                let next = NodeSnapshot {
                    inputs,
                    outputs,
                    skipped,
                    outcome,
                    error,
                };
                Ok(self.state.apply_node(scope, node, next))
            }
            SnapshotRecord::End => {
                self.complete = true;
                Ok(false)
            }
            SnapshotRecord::Header { .. } => unreachable!("headers were handled above"),
        }
    }
}

pub fn ports_snapshot(ports: &crate::Inputs) -> ValueRef {
    ValueRef::object(
        ports
            .iter()
            .map(|(name, value)| (Arc::from(name.as_str()), value.clone())),
    )
}

// Child IDs already identify canonical values, so hashing a parent never walks its payload again.
fn definition_hash(value: &ValueDefinition) -> u64 {
    let mut state = std::collections::hash_map::DefaultHasher::new();
    std::mem::discriminant(value).hash(&mut state);
    match value {
        ValueDefinition::Null => {}
        ValueDefinition::Bool(value) => value.hash(&mut state),
        ValueDefinition::Number(value) => {
            value.is_f64().hash(&mut state);
            if value.is_f64() {
                value.as_f64().unwrap().to_bits().hash(&mut state);
            } else if let Some(value) = value.as_i64() {
                value.hash(&mut state);
            } else {
                value.as_u64().unwrap().hash(&mut state);
            }
        }
        ValueDefinition::String(value) => value.hash(&mut state),
        ValueDefinition::Array(value) => value.hash(&mut state),
        ValueDefinition::Object(value) => value.hash(&mut state),
    }
    state.finish()
}

type SnapshotSink =
    Box<dyn FnMut(&SnapshotRecord) -> Result<(), Box<dyn std::error::Error + Send + Sync>> + Send>;

struct Recording {
    state: SnapshotState,
    values: HashMap<ValueId, ValueRef>,
    next_id: ValueId,
    interned: HashMap<u64, Vec<ValueId>>,
    aliases: HashMap<usize, (Weak<ValueKind>, ValueId)>,
    bounded: Option<BoundedDedup>,
    sink: Option<SnapshotSink>,
    error: Option<Arc<SnapshotError>>,
    complete: bool,
    intern_depth: usize,
}

// LRU bookkeeping for a bounded dictionary: every live ID carries a tick and the
// oldest tick evicts first. Reverse indexes keep eviction O(log n).
#[derive(Debug)]
struct BoundedDedup {
    capacity: usize,
    tick_of: HashMap<ValueId, u64>,
    by_tick: BTreeMap<u64, ValueId>,
    hash_of: HashMap<ValueId, u64>,
    id_aliases: HashMap<ValueId, Vec<usize>>,
    next_tick: u64,
}

impl BoundedDedup {
    fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            tick_of: HashMap::new(),
            by_tick: BTreeMap::new(),
            hash_of: HashMap::new(),
            id_aliases: HashMap::new(),
            next_tick: 0,
        }
    }
}

impl Recording {
    fn write(&mut self, record: &SnapshotRecord) -> Result<(), SnapshotError> {
        self.sink
            .as_mut()
            .map_or(Ok(()), |sink| sink(record).context(SinkSnafu))
    }
    fn intern(&mut self, value: &ValueRef) -> Result<(ValueId, ValueRef), SnapshotError> {
        self.intern_depth += 1;
        let result = self.intern_inner(value);
        self.intern_depth -= 1;
        let id = result?;
        // Read the canonical value back before eviction so node snapshots share the
        // dictionary's allocations.
        let canonical = self
            .values
            .get(&id)
            .cloned()
            .expect("a freshly interned value is live");
        if self.intern_depth == 0 {
            self.evict_to_capacity();
        }
        Ok((id, canonical))
    }
    fn intern_inner(&mut self, value: &ValueRef) -> Result<ValueId, SnapshotError> {
        let alias = matches!(
            value.kind(),
            ValueKind::String(_) | ValueKind::Array(_) | ValueKind::Object(_)
        )
        .then(|| value.downgrade());
        if let Some(alias) = &alias
            && let Some((previous, id)) = self.aliases.get(&(alias.as_ptr() as usize))
            && previous.ptr_eq(alias)
        {
            let id = *id;
            self.touch(id);
            return Ok(id);
        }
        let shared_alias = alias.as_ref().is_some_and(|alias| alias.strong_count() > 1);
        let definition = match value.kind() {
            ValueKind::Null => ValueDefinition::Null,
            ValueKind::Bool(value) => ValueDefinition::Bool(*value),
            ValueKind::Number(value) => ValueDefinition::Number(value.clone()),
            ValueKind::String(value) => ValueDefinition::String(Arc::clone(value)),
            ValueKind::Array(items) => ValueDefinition::Array(
                items
                    .iter()
                    .map(|item| self.intern(item).map(|(id, _)| id))
                    .collect::<Result<_, _>>()?,
            ),
            ValueKind::Object(entries) => ValueDefinition::Object(
                entries
                    .iter()
                    .map(|(key, value)| Ok((key.clone(), self.intern(value)?.0)))
                    .collect::<Result<_, SnapshotError>>()?,
            ),
        };
        let canonical = match (&definition, value.kind()) {
            (ValueDefinition::Array(ids), ValueKind::Array(items)) => {
                if ids
                    .iter()
                    .zip(items.iter())
                    .all(|(id, item)| self.values[id].ptr_eq(item))
                {
                    value.clone()
                } else {
                    ValueRef::array(ids.iter().map(|id| self.values[id].clone()))
                }
            }
            (ValueDefinition::Object(ids), ValueKind::Object(entries)) => {
                if ids
                    .iter()
                    .all(|(key, id)| self.values[id].ptr_eq(entries.get(key.as_ref()).unwrap()))
                {
                    value.clone()
                } else {
                    ValueRef::object(
                        ids.iter()
                            .map(|(key, id)| (key.clone(), self.values[id].clone())),
                    )
                }
            }
            _ => value.clone(),
        };
        let hash = definition_hash(&definition);
        if let Some(candidates) = self.interned.get(&hash)
            && let Some(&id) = candidates.iter().find(|&&id| self.values[&id] == canonical)
        {
            if shared_alias {
                self.remember(alias, id);
            }
            self.touch(id);
            return Ok(id);
        }
        let id = self.next_id;
        self.next_id += 1;
        let retain_alias = shared_alias || canonical.ptr_eq(value);
        self.values.insert(id, canonical);
        self.write(&SnapshotRecord::Value {
            id,
            value: definition,
        })?;
        self.interned.entry(hash).or_default().push(id);
        if let Some(bounded) = &mut self.bounded {
            bounded.hash_of.insert(id, hash);
        }
        if retain_alias {
            self.remember(alias, id);
        }
        self.touch(id);
        Ok(id)
    }
    fn touch(&mut self, id: ValueId) {
        if let Some(bounded) = &mut self.bounded {
            let tick = bounded.next_tick;
            bounded.next_tick += 1;
            if let Some(previous) = bounded.tick_of.insert(id, tick) {
                bounded.by_tick.remove(&previous);
            }
            bounded.by_tick.insert(tick, id);
        }
    }
    fn remember(&mut self, alias: Option<Weak<ValueKind>>, id: ValueId) {
        if let Some(alias) = alias {
            let pointer = alias.as_ptr() as usize;
            self.aliases.insert(pointer, (alias, id));
            if let Some(bounded) = &mut self.bounded {
                bounded.id_aliases.entry(id).or_default().push(pointer);
            }
        }
        if self.aliases.len() > self.values.len().saturating_mul(2).saturating_add(64) {
            self.aliases
                .retain(|_, (value, _)| value.strong_count() != 0);
        }
    }
    // Eviction runs only after the outermost intern completes so a parent definition
    // never loses a child it just referenced. An evicted value is defined again under
    // a new ID when it reappears, which keeps the wire format unchanged.
    fn evict_to_capacity(&mut self) {
        let Some(bounded) = &mut self.bounded else {
            return;
        };
        while self.values.len() > bounded.capacity {
            let Some((_, id)) = bounded.by_tick.pop_first() else {
                break;
            };
            bounded.tick_of.remove(&id);
            self.values.remove(&id);
            if let Some(hash) = bounded.hash_of.remove(&id)
                && let Some(candidates) = self.interned.get_mut(&hash)
            {
                candidates.retain(|&candidate| candidate != id);
                if candidates.is_empty() {
                    self.interned.remove(&hash);
                }
            }
            if let Some(pointers) = bounded.id_aliases.remove(&id) {
                for pointer in pointers {
                    self.aliases.remove(&pointer);
                }
            }
        }
    }
}

/// Current node values and scope at one capture boundary.
#[derive(Clone, Debug)]
pub struct SnapshotEvent {
    pub scope: Vec<LoopPathEntry>,
    pub node: Arc<str>,
    pub snapshot: NodeSnapshot,
}

type SnapshotCallback = dyn Fn(SnapshotEvent) + Send + Sync;

/// A shared notification handle with no encoding or retained values.
#[derive(Clone)]
pub struct SnapshotRecorder {
    callback: Arc<SnapshotCallback>,
}

impl std::fmt::Debug for SnapshotRecorder {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SnapshotRecorder")
            .finish_non_exhaustive()
    }
}

impl SnapshotRecorder {
    /// The callback must be nonblocking and must not unwind.
    pub fn new(callback: impl Fn(SnapshotEvent) + Send + Sync + 'static) -> Self {
        Self {
            callback: Arc::new(callback),
        }
    }

    pub fn record(&self, mut event: SnapshotEvent) {
        event.snapshot.redact();
        (self.callback)(event);
    }
}

/// Dictionary retention policy for the producer-side snapshot encoder.
///
/// Bounding only affects the producer's deduplication cache: an evicted value is
/// defined again under a new ID when it reappears, so the wire format and consumer
/// stores are unaffected.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SnapshotRetention {
    /// Retain every distinct value for the lifetime of the recording.
    #[default]
    Unbounded,
    /// Bound the live deduplication dictionary, evicting least recently used values.
    Bounded { max_values: usize },
}

/// Caller-owned dictionary encoding and retained snapshot history.
#[derive(Clone)]
pub struct SnapshotRecording(Arc<Mutex<Recording>>);

impl fmt::Debug for SnapshotRecording {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SnapshotRecording").finish_non_exhaustive()
    }
}
use std::fmt;

impl SnapshotRecording {
    pub fn memory() -> Self {
        Self::new(None, SnapshotRetention::Unbounded)
            .expect("writing the snapshot header without a sink cannot fail")
    }
    pub fn with_sink(
        sink: impl FnMut(&SnapshotRecord) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
        + Send
        + 'static,
    ) -> Result<Self, SnapshotError> {
        Self::with_sink_and_retention(sink, SnapshotRetention::Unbounded)
    }
    pub fn with_sink_and_retention(
        sink: impl FnMut(&SnapshotRecord) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
        + Send
        + 'static,
        retention: SnapshotRetention,
    ) -> Result<Self, SnapshotError> {
        Self::new(Some(Box::new(sink)), retention)
    }
    fn new(
        sink: Option<SnapshotSink>,
        retention: SnapshotRetention,
    ) -> Result<Self, SnapshotError> {
        let mut recording = Recording {
            state: SnapshotState::default(),
            values: HashMap::new(),
            next_id: 0,
            interned: HashMap::new(),
            aliases: HashMap::new(),
            bounded: match retention {
                SnapshotRetention::Unbounded => None,
                SnapshotRetention::Bounded { max_values } => Some(BoundedDedup::new(max_values)),
            },
            sink,
            error: None,
            complete: false,
            intern_depth: 0,
        };
        recording.write(&SnapshotRecord::Header {
            version: SNAPSHOT_VERSION,
        })?;
        Ok(Self(Arc::new(Mutex::new(recording))))
    }
    pub fn current(&self) -> Snapshot {
        self.0
            .lock()
            .expect("snapshot recorder was not poisoned")
            .state
            .current
            .clone()
    }
    pub fn history(&self) -> Vec<SnapshotEntry> {
        self.0
            .lock()
            .expect("snapshot recorder was not poisoned")
            .state
            .history
            .clone()
    }
    /// Total value IDs assigned so far. Bounded retention evicts live entries without
    /// lowering this count, which matches the consumer's dictionary size.
    pub fn value_count(&self) -> usize {
        self.0
            .lock()
            .expect("snapshot recorder was not poisoned")
            .next_id
    }
    pub fn diagnostic(&self) -> Option<Arc<SnapshotError>> {
        self.0
            .lock()
            .expect("snapshot recorder was not poisoned")
            .error
            .clone()
    }

    pub fn recorder(&self) -> SnapshotRecorder {
        let recording = self.clone();
        SnapshotRecorder::new(move |event| recording.record_event(event))
    }

    pub fn record(&self, scope: Vec<LoopPathEntry>, node: &str, snapshot: NodeSnapshot) {
        self.record_event(SnapshotEvent {
            scope,
            node: node.into(),
            snapshot,
        });
    }

    pub fn record_event(&self, event: SnapshotEvent) {
        let SnapshotEvent {
            scope,
            node,
            mut snapshot,
        } = event;
        // Redact before deduplication, interning, retention, or calling an external sink.
        snapshot.redact();
        let mut recording = self.0.lock().expect("snapshot recorder was not poisoned");
        if recording.error.is_some() || recording.complete {
            return;
        }
        if !matches!(
            snapshot.outcome,
            SnapshotOutcome::Started | SnapshotOutcome::Emitted
        ) && recording
            .state
            .current
            .node(&scope, &node)
            .is_some_and(|previous| previous.as_ref() == &snapshot)
        {
            return;
        }
        let result = (|| {
            let (inputs, input_values) = recording.intern(&snapshot.inputs)?;
            let (outputs, output_values) = recording.intern(&snapshot.outputs)?;
            let record = SnapshotRecord::Node {
                scope: scope.clone(),
                node: node.clone(),
                inputs,
                outputs,
                skipped: snapshot.skipped.clone(),
                outcome: snapshot.outcome,
                error: snapshot.error.clone(),
            };
            recording.state.apply_node(
                scope,
                node,
                NodeSnapshot {
                    inputs: input_values,
                    outputs: output_values,
                    skipped: snapshot.skipped,
                    outcome: snapshot.outcome,
                    error: snapshot.error,
                },
            );
            // The consumer owns history; the producer keeps the current root and value pool.
            if recording.sink.is_some() {
                recording.state.history.clear();
            }
            recording.write(&record)
        })();
        if let Err(error) = result {
            recording.error = Some(Arc::new(error));
        }
    }
    pub fn finish(&self) {
        let mut recording = self.0.lock().expect("snapshot recorder was not poisoned");
        if recording.complete || recording.error.is_some() {
            return;
        }
        match recording.write(&SnapshotRecord::End) {
            Ok(()) => recording.complete = true,
            Err(error) => recording.error = Some(Arc::new(error)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn completed(value: ValueRef) -> NodeSnapshot {
        NodeSnapshot {
            inputs: ports_snapshot(&crate::Inputs::from([("input".into(), value.clone())])),
            outputs: ports_snapshot(&crate::Outputs::from([("value".into(), value)])),
            skipped: Arc::from([]),
            outcome: SnapshotOutcome::Succeeded,
            error: None,
        }
    }

    fn emitted(value: ValueRef) -> NodeSnapshot {
        NodeSnapshot {
            outcome: SnapshotOutcome::Emitted,
            ..completed(value)
        }
    }

    #[test]
    fn secrets_never_reach_callbacks_dictionaries_or_retained_history() {
        let records = Arc::new(Mutex::new(Vec::new()));
        let captured = records.clone();
        let recording = SnapshotRecording::with_sink(move |record| {
            captured.lock().unwrap().push(record.clone());
            Ok(())
        })
        .unwrap();
        let value = ValueRef::object([
            ("visible".into(), "public".into()),
            (
                "credential".into(),
                ValueRef::from("sentinel-secret").into_secret(),
            ),
            (
                "object".into(),
                ValueRef::from(json!({"secret-key": [1, 2]})).into_secret(),
            ),
        ]);
        let expected =
            json!({"visible": "public", "credential": "[REDACTED]", "object": "[REDACTED]"});
        let callback_expected = expected.clone();
        let recorder = SnapshotRecorder::new(move |event| {
            assert_eq!(event.snapshot.inputs["input"], callback_expected);
            assert!(!event.snapshot.inputs.contains_secret());
        });
        recorder.record(SnapshotEvent {
            scope: vec![],
            node: "node".into(),
            snapshot: completed(value.clone()),
        });
        recording.record(vec![], "node", completed(value));
        recording.finish();
        assert!(recording.diagnostic().is_none());
        assert_eq!(
            recording.current().node(&[], "node").unwrap().outputs["value"],
            expected
        );
        let records = records.lock().unwrap();
        let wire = serde_json::to_string(&*records).unwrap();
        assert!(!wire.contains("sentinel-secret"));
        assert!(!wire.contains("secret-key"));
        let mut store = SnapshotStore::default();
        for record in records.iter().cloned() {
            store.apply(record).unwrap();
        }
        assert!(store.is_complete());
        assert_eq!(store.history().len(), 1);
        assert_eq!(
            store.current().node(&[], "node").unwrap().outputs["value"],
            expected
        );
    }

    #[test]
    fn secret_aliases_do_not_reuse_public_dictionary_values_or_suppress_updates() {
        for secret_first in [false, true] {
            let recording = SnapshotRecording::memory();
            let public = ValueRef::from("same-payload");
            let secret = public.clone().into_secret();
            let values = if secret_first {
                [&secret, &public]
            } else {
                [&public, &secret]
            };
            for value in values {
                recording.record(vec![], "node", completed(value.clone()));
                let expected = if value.is_secret() {
                    "[REDACTED]"
                } else {
                    "same-payload"
                };
                assert_eq!(
                    recording.current().node(&[], "node").unwrap().outputs["value"],
                    json!(expected)
                );
            }
            assert_eq!(recording.history().len(), 2);
        }
    }

    #[test]
    fn recorder_retains_sink_sources_after_failure() {
        use std::{error::Error, io};
        let recorder = SnapshotRecording::with_sink(|record| {
            if matches!(record, SnapshotRecord::Header { .. }) {
                return Ok(());
            }
            Err(Box::new(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "sink closed",
            )))
        })
        .unwrap();
        recorder.record(vec![], "node", completed(42.into()));
        recorder.finish();
        assert_eq!(
            recorder
                .diagnostic()
                .unwrap()
                .source()
                .unwrap()
                .downcast_ref::<io::Error>()
                .unwrap()
                .kind(),
            io::ErrorKind::BrokenPipe
        );
    }

    #[test]
    fn history_shares_values_and_unchanged_nodes_across_global_roots() {
        let recorder = SnapshotRecording::memory();
        let value = ValueRef::from(json!({"data": [1, 2], "payload": "large payload"}));
        recorder.record(vec![], "first", completed(value.clone()));
        let first = recorder.current();
        let unique = recorder.value_count();
        let duplicate = ValueRef::from(serde_json::to_value(&value).unwrap());
        let weak = duplicate.downgrade();
        recorder.record(vec![], "second", completed(duplicate));
        assert!(weak.upgrade().is_none());
        let second = recorder.current();
        assert_eq!(recorder.value_count(), unique);
        assert!(Arc::ptr_eq(
            first.node(&[], "first").unwrap(),
            second.node(&[], "first").unwrap()
        ));
        assert!(
            second
                .node(&[], "first")
                .unwrap()
                .outputs
                .ptr_eq(&second.node(&[], "second").unwrap().outputs)
        );
        let changed = ValueRef::from(json!({"data": [3, 2], "payload": "large payload"}));
        recorder.record(vec![], "first", completed(changed));
        let latest = recorder.current();
        assert_eq!(
            first.node(&[], "first").unwrap().outputs["value"]["data"][0],
            json!(1)
        );
        assert_eq!(
            latest.node(&[], "first").unwrap().outputs["value"]["data"][0],
            json!(3)
        );
        assert!(
            first.node(&[], "first").unwrap().outputs["value"]["payload"]
                .ptr_eq(&latest.node(&[], "first").unwrap().outputs["value"]["payload"])
        );
        assert_eq!(recorder.history().len(), 3);
        assert!(recorder.diagnostic().is_none());
    }

    #[test]
    fn equal_updates_reuse_the_root_and_do_not_append_history() {
        let recorder = SnapshotRecording::memory();
        recorder.record(vec![], "node", completed(json!({"x": 1}).into()));
        let root = recorder.current();
        let unique = recorder.value_count();
        recorder.record(vec![], "node", completed(json!({"x": 1}).into()));
        assert!(root.ptr_eq(&recorder.current()));
        assert_eq!(recorder.history().len(), 1);
        assert_eq!(recorder.value_count(), unique);
        recorder.finish();
        recorder.finish();
        recorder.record(vec![], "node", completed(2.into()));
        assert!(root.ptr_eq(&recorder.current()));
    }

    fn bounded_sink(max_values: usize) -> (SnapshotRecording, Arc<Mutex<Vec<SnapshotRecord>>>) {
        let records = Arc::new(Mutex::new(Vec::new()));
        let captured = records.clone();
        let recording = SnapshotRecording::with_sink_and_retention(
            move |record| {
                captured.lock().unwrap().push(record.clone());
                Ok(())
            },
            SnapshotRetention::Bounded { max_values },
        )
        .unwrap();
        (recording, records)
    }

    fn replay(records: &[SnapshotRecord]) -> SnapshotStore {
        let mut store = SnapshotStore::default();
        for record in records.iter().cloned() {
            store.apply(record).unwrap();
        }
        store
    }

    fn live_values(recording: &SnapshotRecording) -> usize {
        recording.0.lock().unwrap().values.len()
    }

    #[test]
    fn bounded_dictionary_evicts_and_redefines_values() {
        let (recording, records) = bounded_sink(64);
        for value in 0..1_000i64 {
            recording.record(vec![], "node", completed(value.into()));
            let live = live_values(&recording);
            assert!(live <= 64, "live dictionary exceeded its capacity: {live}");
        }
        assert!(recording.history().is_empty());
        assert_eq!(
            recording.current().node(&[], "node").unwrap().outputs["value"],
            json!(999)
        );
        recording.finish();
        assert!(recording.diagnostic().is_none());
        let records = records.lock().unwrap();
        // Definitions only refer to earlier IDs, so the strict decoder replays the stream.
        for record in records.iter() {
            let SnapshotRecord::Value { id, value } = record else {
                continue;
            };
            if let ValueDefinition::Array(items) = value {
                assert!(items.iter().all(|child| child < id));
            }
            if let ValueDefinition::Object(entries) = value {
                assert!(entries.values().all(|child| child < id));
            }
        }
        let store = replay(&records);
        assert!(store.is_complete());
        assert_eq!(store.history().len(), 1_000);
        assert_eq!(store.value_count(), recording.value_count());
        assert_eq!(
            store.current().node(&[], "node").unwrap().outputs["value"],
            json!(999)
        );
    }

    #[test]
    fn evicted_value_receives_a_new_id_when_reencountered() {
        let (recording, records) = bounded_sink(8);
        recording.record(vec![], "node", completed("sentinel".into()));
        for value in 0..100i64 {
            recording.record(vec![], "node", completed(value.into()));
        }
        recording.record(vec![], "node", completed("sentinel".into()));
        recording.finish();
        let records = records.lock().unwrap();
        let definitions: Vec<ValueId> = records
            .iter()
            .filter_map(|record| match record {
                SnapshotRecord::Value {
                    id,
                    value: ValueDefinition::String(payload),
                } if payload.as_ref() == "sentinel" => Some(*id),
                _ => None,
            })
            .collect();
        assert_eq!(definitions.len(), 2);
        assert_ne!(definitions[0], definitions[1]);
        let store = replay(&records);
        assert!(store.is_complete());
        assert_eq!(store.history().len(), 102);
        assert_eq!(store.value_count(), recording.value_count());
        assert_eq!(
            store.current().node(&[], "node").unwrap().outputs["value"],
            json!("sentinel")
        );
    }

    #[test]
    fn hot_values_survive_lru_eviction() {
        let (recording, records) = bounded_sink(8);
        let hot = ValueRef::from("hot");
        for value in 0..50i64 {
            recording.record(vec![], "hot", emitted(hot.clone()));
            recording.record(vec![], "cold", emitted(value.into()));
        }
        recording.finish();
        let records = records.lock().unwrap();
        let hot_definitions: Vec<ValueId> = records
            .iter()
            .filter_map(|record| match record {
                SnapshotRecord::Value {
                    id,
                    value: ValueDefinition::String(payload),
                } if payload.as_ref() == "hot" => Some(*id),
                _ => None,
            })
            .collect();
        assert_eq!(hot_definitions.len(), 1);
        let hot_outputs: Vec<ValueId> = records
            .iter()
            .filter_map(|record| match record {
                SnapshotRecord::Node { node, outputs, .. } if node.as_ref() == "hot" => {
                    Some(*outputs)
                }
                _ => None,
            })
            .collect();
        assert_eq!(hot_outputs.len(), 50);
        assert!(hot_outputs.iter().all(|outputs| *outputs == hot_outputs[0]));
        let store = replay(&records);
        assert!(store.is_complete());
        assert_eq!(store.history().len(), 100);
        assert_eq!(
            store.current().node(&[], "hot").unwrap().outputs["value"],
            json!("hot")
        );
    }

    #[test]
    fn single_large_value_exceeding_capacity_stays_valid() {
        let (recording, records) = bounded_sink(4);
        let big = ValueRef::array((0..100i64).map(ValueRef::from));
        recording.record(vec![], "node", completed(big));
        recording.finish();
        assert!(recording.diagnostic().is_none());
        assert!(live_values(&recording) <= 4);
        let store = replay(&records.lock().unwrap());
        assert!(store.is_complete());
        assert_eq!(store.history().len(), 1);
        assert_eq!(
            store.current().node(&[], "node").unwrap().outputs["value"][99],
            json!(99)
        );
    }

    #[test]
    fn node_records_preserve_the_published_wire_format() {
        let wire = json!({
            "record": "node", "scope": [], "node": "step", "inputs": 0,
            "outputs": 1, "skipped": [], "outcome": "succeeded"
        });
        let record: SnapshotRecord = serde_json::from_value(wire.clone()).unwrap();
        assert!(matches!(record, SnapshotRecord::Node { .. }));
        assert_eq!(serde_json::to_value(record).unwrap(), wire);
    }

    #[test]
    fn structured_number_definitions_preserve_json_numeric_types() {
        for text in ["0", "0.0", "-0.0", "18446744073709551615", "1.5"] {
            let number: Number = text.parse().unwrap();
            let record = SnapshotRecord::Value {
                id: 0,
                value: ValueDefinition::Number(number),
            };
            let encoded = serde_json::to_value(record).unwrap();
            assert_eq!(encoded["value"]["data"], text);
            let decoded: SnapshotRecord = serde_json::from_value(encoded).unwrap();
            let SnapshotRecord::Value {
                value: ValueDefinition::Number(number),
                ..
            } = decoded
            else {
                panic!("expected number")
            };
            assert_eq!(number.to_string(), text);
        }
    }

    #[test]
    fn decoder_rejects_missing_headers_forward_references_and_trailing_records() {
        let mut store = SnapshotStore::default();
        assert!(store.apply(SnapshotRecord::End).is_err());
        assert!(
            store
                .apply(SnapshotRecord::Header {
                    version: SNAPSHOT_VERSION + 1
                })
                .is_err()
        );
        store
            .apply(SnapshotRecord::Header {
                version: SNAPSHOT_VERSION,
            })
            .unwrap();
        assert!(
            store
                .apply(SnapshotRecord::Value {
                    id: 0,
                    value: ValueDefinition::Array(vec![0])
                })
                .is_err()
        );
        assert_eq!(store.value_count(), 0);
        store
            .apply(SnapshotRecord::Value {
                id: 0,
                value: ValueDefinition::String("payload".into()),
            })
            .unwrap();
        store
            .apply(SnapshotRecord::Value {
                id: 1,
                value: ValueDefinition::Array(vec![0, 0]),
            })
            .unwrap();
        assert!(store.value(1).unwrap()[0].ptr_eq(&store.value(1).unwrap()[1]));
        assert!(
            store
                .apply(SnapshotRecord::Value {
                    id: 1,
                    value: ValueDefinition::Null
                })
                .is_err()
        );
        store.apply(SnapshotRecord::End).unwrap();
        assert!(store.is_complete());
        assert!(store.apply(SnapshotRecord::End).is_err());
    }
}

// OTLP integers are signed. Text inside the typed number definition preserves
// unsigned integers and distinctions such as integer zero versus negative zero.
mod number_text {
    use serde::{Deserialize, Deserializer, Serializer};
    use serde_json::Number;
    pub fn serialize<S: Serializer>(number: &Number, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&number.to_string())
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Number, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}
