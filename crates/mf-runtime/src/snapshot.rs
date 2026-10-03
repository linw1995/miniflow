use crate::value::ValueData;
use crate::{ValueKind, ValueRef};
use mf_telemetry::event::LoopPathEntry;
use rpds::RedBlackTreeMapSync;
use serde::{Deserialize, Serialize};
use serde_json::Number;
use std::{
    collections::{BTreeMap, HashMap},
    hash::{Hash, Hasher},
    sync::{Arc, Mutex, Weak},
};

pub const SNAPSHOT_VERSION: u32 = 1;
pub type ValueId = usize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotOutcome {
    Started,
    Succeeded,
    Failed,
    Skipped,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeSnapshot {
    pub inputs: ValueRef,
    pub outputs: ValueRef,
    pub skipped: Arc<[String]>,
    pub outcome: SnapshotOutcome,
    pub error: Option<Arc<str>>,
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

#[derive(Debug, Default)]
pub struct SnapshotStore {
    values: Vec<ValueRef>,
    current: Snapshot,
    history: Vec<SnapshotEntry>,
    initialized: bool,
    complete: bool,
}

impl SnapshotStore {
    pub fn current(&self) -> &Snapshot {
        &self.current
    }
    pub fn history(&self) -> &[SnapshotEntry] {
        &self.history
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

    pub fn apply(&mut self, record: SnapshotRecord) -> Result<bool, String> {
        if self.complete {
            return Err("snapshot record followed the end marker".into());
        }
        if let SnapshotRecord::Header { version } = record {
            if self.initialized || version != SNAPSHOT_VERSION {
                return Err("invalid snapshot header".into());
            }
            self.initialized = true;
            return Ok(false);
        }
        if !self.initialized {
            return Err("snapshot stream omitted its header".into());
        }
        match record {
            SnapshotRecord::Value { id, value } => {
                if id != self.values.len() {
                    return Err("snapshot value IDs must be contiguous".into());
                }
                let reference = |id| {
                    self.values
                        .get(id)
                        .cloned()
                        .ok_or_else(|| "unknown snapshot value reference".to_owned())
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
                            .collect::<Result<Vec<_>, String>>()?,
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
                let inputs = self
                    .values
                    .get(inputs)
                    .cloned()
                    .ok_or("unknown input snapshot")?;
                let outputs = self
                    .values
                    .get(outputs)
                    .cloned()
                    .ok_or("unknown output snapshot")?;
                if !inputs.is_object() || !outputs.is_object() {
                    return Err("node snapshots require input and output objects".into());
                }
                let next = NodeSnapshot {
                    inputs,
                    outputs,
                    skipped,
                    outcome,
                    error,
                };
                if self
                    .current
                    .node(&scope, &node)
                    .is_some_and(|previous| previous.as_ref() == &next)
                {
                    return Ok(false);
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
                Ok(true)
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

type SnapshotSink = Box<dyn FnMut(&SnapshotRecord) -> Result<(), String> + Send>;

struct Recording {
    store: SnapshotStore,
    interned: HashMap<u64, Vec<ValueId>>,
    aliases: HashMap<usize, (Weak<ValueData>, ValueId)>,
    sink: Option<SnapshotSink>,
    error: Option<String>,
}

impl Recording {
    fn append(&mut self, record: SnapshotRecord) -> Result<(), String> {
        self.store.apply(record.clone())?;
        // The consumer owns history; the producer keeps the current root and value pool.
        if self.sink.is_some() {
            self.store.history.clear();
        }
        self.write(&record)
    }
    fn write(&mut self, record: &SnapshotRecord) -> Result<(), String> {
        self.sink.as_mut().map_or(Ok(()), |sink| sink(record))
    }
    fn intern(&mut self, value: &ValueRef) -> Result<ValueId, String> {
        let alias = matches!(
            value.kind(),
            ValueKind::String(_) | ValueKind::Array(_) | ValueKind::Object(_)
        )
        .then(|| value.downgrade());
        if let Some(alias) = &alias
            && let Some((previous, id)) = self.aliases.get(&(alias.as_ptr() as usize))
            && previous.ptr_eq(alias)
        {
            return Ok(*id);
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
                    .map(|item| self.intern(item))
                    .collect::<Result<_, _>>()?,
            ),
            ValueKind::Object(entries) => ValueDefinition::Object(
                entries
                    .iter()
                    .map(|(key, value)| Ok((key.clone(), self.intern(value)?)))
                    .collect::<Result<_, String>>()?,
            ),
        };
        let canonical = match (&definition, value.kind()) {
            (ValueDefinition::Array(ids), ValueKind::Array(items)) => {
                if ids
                    .iter()
                    .zip(items.iter())
                    .all(|(id, item)| self.store.values[*id].ptr_eq(item))
                {
                    value.clone()
                } else {
                    ValueRef::array(ids.iter().map(|id| self.store.values[*id].clone()))
                }
            }
            (ValueDefinition::Object(ids), ValueKind::Object(entries)) => {
                if ids.iter().all(|(key, id)| {
                    self.store.values[*id].ptr_eq(entries.get(key.as_ref()).unwrap())
                }) {
                    value.clone()
                } else {
                    ValueRef::object(
                        ids.iter()
                            .map(|(key, id)| (key.clone(), self.store.values[*id].clone())),
                    )
                }
            }
            _ => value.clone(),
        };
        let hash = definition_hash(&definition);
        if let Some(candidates) = self.interned.get(&hash)
            && let Some(&id) = candidates
                .iter()
                .find(|&&id| self.store.values[id] == canonical)
        {
            if shared_alias {
                self.remember(alias, id);
            }
            return Ok(id);
        }
        let id = self.store.values.len();
        let retain_alias = shared_alias || canonical.ptr_eq(value);
        self.store.values.push(canonical);
        self.write(&SnapshotRecord::Value {
            id,
            value: definition,
        })?;
        self.interned.entry(hash).or_default().push(id);
        if retain_alias {
            self.remember(alias, id);
        }
        Ok(id)
    }
    fn remember(&mut self, alias: Option<Weak<ValueData>>, id: ValueId) {
        if let Some(alias) = alias {
            self.aliases.insert(alias.as_ptr() as usize, (alias, id));
        }
        if self.aliases.len() > self.store.values.len().saturating_mul(2).saturating_add(64) {
            self.aliases
                .retain(|_, (value, _)| value.strong_count() != 0);
        }
    }
}

/// A recorder is allocated only when its owner explicitly requests history.
#[derive(Clone)]
pub struct SnapshotRecorder(Arc<Mutex<Recording>>);

impl fmt::Debug for SnapshotRecorder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SnapshotRecorder").finish_non_exhaustive()
    }
}
use std::fmt;

impl SnapshotRecorder {
    pub fn memory() -> Self {
        let mut recording = Recording {
            store: SnapshotStore::default(),
            interned: HashMap::new(),
            aliases: HashMap::new(),
            sink: None,
            error: None,
        };
        recording
            .append(SnapshotRecord::Header {
                version: SNAPSHOT_VERSION,
            })
            .expect("valid initial snapshot header");
        Self(Arc::new(Mutex::new(recording)))
    }
    pub fn with_sink(
        sink: impl FnMut(&SnapshotRecord) -> Result<(), String> + Send + 'static,
    ) -> Result<Self, String> {
        let recorder = Self::memory();
        {
            let mut recording = recorder
                .0
                .lock()
                .expect("snapshot recorder was not poisoned");
            recording.sink = Some(Box::new(sink));
            recording.write(&SnapshotRecord::Header {
                version: SNAPSHOT_VERSION,
            })?;
        }
        Ok(recorder)
    }
    pub fn current(&self) -> Snapshot {
        self.0
            .lock()
            .expect("snapshot recorder was not poisoned")
            .store
            .current
            .clone()
    }
    pub fn history(&self) -> Vec<SnapshotEntry> {
        self.0
            .lock()
            .expect("snapshot recorder was not poisoned")
            .store
            .history
            .clone()
    }
    pub fn value_count(&self) -> usize {
        self.0
            .lock()
            .expect("snapshot recorder was not poisoned")
            .store
            .value_count()
    }
    pub fn diagnostic(&self) -> Option<String> {
        self.0
            .lock()
            .expect("snapshot recorder was not poisoned")
            .error
            .clone()
    }

    pub fn record(&self, scope: Vec<LoopPathEntry>, node: &str, snapshot: NodeSnapshot) {
        let mut recording = self.0.lock().expect("snapshot recorder was not poisoned");
        if recording.error.is_some() || recording.store.complete {
            return;
        }
        if recording
            .store
            .current
            .node(&scope, node)
            .is_some_and(|previous| previous.as_ref() == &snapshot)
        {
            return;
        }
        let result = (|| {
            let inputs = recording.intern(&snapshot.inputs)?;
            let outputs = recording.intern(&snapshot.outputs)?;
            recording.append(SnapshotRecord::Node {
                scope,
                node: node.into(),
                inputs,
                outputs,
                skipped: snapshot.skipped,
                outcome: snapshot.outcome,
                error: snapshot.error,
            })?;
            Ok::<(), String>(())
        })();
        if let Err(error) = result {
            recording.error = Some(error);
        }
    }
    pub fn finish(&self) {
        let mut recording = self.0.lock().expect("snapshot recorder was not poisoned");
        if recording.store.complete || recording.error.is_some() {
            return;
        }
        let result = recording.append(SnapshotRecord::End);
        if let Err(error) = result {
            recording.error = Some(error);
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

    #[test]
    fn history_shares_values_and_unchanged_nodes_across_global_roots() {
        let recorder = SnapshotRecorder::memory();
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
        let recorder = SnapshotRecorder::memory();
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
