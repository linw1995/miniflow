#![allow(dead_code)]

use mf_runtime::{
    Emitter, ExecutionContext, Inputs, NodeExecutionError, NodeFactory, NodePorts,
    NodeRegistration, Outputs, PortSpec, PreparedNode, PreparedStream, StreamError, StreamInstance,
    StreamNode, StreamOptions, StreamSummary, ValueRef, ValueType,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex, OnceLock, Weak,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::Duration,
};

struct Control {
    sender: Mutex<Option<mpsc::Sender<ValueRef>>>,
    receiver: Mutex<Option<mpsc::Receiver<ValueRef>>>,
    issued: AtomicU64,
    emitted: AtomicU64,
}

fn controls() -> &'static Mutex<BTreeMap<String, Weak<Control>>> {
    static CONTROLS: OnceLock<Mutex<BTreeMap<String, Weak<Control>>>> = OnceLock::new();
    CONTROLS.get_or_init(Mutex::default)
}

#[derive(Clone)]
pub struct Controller(Arc<Control>);

impl Controller {
    pub fn send(&self, value: impl Into<ValueRef>) -> Result<(), StreamError> {
        let sender = self.0.sender.lock().unwrap();
        let sender = sender.as_ref().ok_or_else(|| StreamError::Execution {
            message: "test source is closed".into(),
        })?;
        sender
            .send(value.into())
            .map_err(|_| StreamError::Execution {
                message: "test source stopped".into(),
            })?;
        self.0.issued.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    pub fn close(&self) {
        self.0.sender.lock().unwrap().take();
    }

    pub fn settled(&self) -> bool {
        self.0.issued.load(Ordering::SeqCst) == self.0.emitted.load(Ordering::SeqCst)
    }
}

struct Source;

impl StreamNode for Source {
    fn execute(
        &mut self,
        inputs: Inputs,
        context: &mut ExecutionContext,
        emitter: &mut Emitter<'_>,
    ) -> Result<(), NodeExecutionError> {
        let key = inputs["control"].as_str().unwrap();
        let control = controls().lock().unwrap()[key]
            .upgrade()
            .expect("test control is alive");
        let receiver = control
            .receiver
            .lock()
            .unwrap()
            .take()
            .expect("source starts once");
        loop {
            if let Some(source) = context.cancellation().failure() {
                return Err(NodeExecutionError::PluginFailed {
                    source: Box::new(source),
                });
            }
            match receiver.recv_timeout(Duration::from_millis(5)) {
                Ok(value) => {
                    emitter.send(Outputs::from([("item".into(), value)]).into())?;
                    control.emitted.fetch_add(1, Ordering::SeqCst);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
            }
        }
    }
}

inventory::submit! { NodeRegistration {
    kind: "test.controlled_source",
    factory: NodeFactory::Plain(|config| {
        let value_type: ValueType = serde_json::from_value(config["item_type"].clone()).unwrap();
        Ok(PreparedNode::stream(Source, NodePorts {
            inputs: vec![PortSpec::new("control", ValueType::String, true)],
            outputs: vec![PortSpec::new("item", value_type, true)],
        }))
    }),
} }

pub fn prepare_control(node: &str, options: &mut StreamOptions) -> Controller {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let key = NEXT.fetch_add(1, Ordering::Relaxed).to_string();
    let (sender, receiver) = mpsc::channel();
    let control = Arc::new(Control {
        sender: Mutex::new(Some(sender)),
        receiver: Mutex::new(Some(receiver)),
        issued: AtomicU64::new(0),
        emitted: AtomicU64::new(0),
    });
    let mut entries = controls().lock().unwrap();
    entries.retain(|_, control| control.strong_count() != 0);
    entries.insert(key.clone(), Arc::downgrade(&control));
    options
        .arguments
        .0
        .entry(node.into())
        .or_default()
        .insert("control".into(), key.into());
    Controller(control)
}

pub struct SourceRun {
    pub instance: StreamInstance,
    pub source: Controller,
}

impl std::ops::Deref for SourceRun {
    type Target = StreamInstance;
    fn deref(&self) -> &Self::Target {
        &self.instance
    }
}

impl SourceRun {
    pub fn start(
        prepared: PreparedStream,
        node: &str,
        mut options: StreamOptions,
    ) -> Result<Self, StreamError> {
        let source = prepare_control(node, &mut options);
        let instance = prepared.start_with_options(options)?;
        Ok(Self { instance, source })
    }

    pub fn join(self) -> Result<StreamSummary, StreamError> {
        self.instance.join()
    }

    pub fn settled(&self) -> bool {
        let summary = self.instance.summary();
        self.source.settled()
            && summary.completed_frames == summary.startup_frames + summary.emitted_messages
    }
}

pub fn source(item_type: Value) -> Value {
    json!({"id":"feed", "kind":"test.controlled_source", "config":{"item_type":item_type}})
}
