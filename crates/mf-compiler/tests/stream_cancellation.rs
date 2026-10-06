#[allow(dead_code)]
#[path = "fixtures/observation_capture.rs"]
mod capture;
extern crate mfn_core as _;

use mf_compiler::{NodeRegistry, WorkflowDefinition, compile_definition, instantiate_stream};
use mf_runtime::{
    EventContext, EventEffects, EventNode, NodeEvent, NodeExecutionError, NodeFactory, NodePorts,
    NodeRegistration, NodeResult, Outputs, PortSpec, PreparedNode, StreamCancellation, StreamError,
    StreamOptions, TimerUpdate, ValueType,
};
use mf_telemetry::{
    INSTRUMENTATION_SCOPE,
    identity::RunId,
    observation::Observer,
    stream::{StreamEvent, StreamPayload, StreamRecord},
};
use opentelemetry::InstrumentationScope;
use opentelemetry_sdk::{
    error::OTelSdkResult,
    logs::{LogProcessor, SdkLogRecord, SdkLoggerProvider},
    trace::SdkTracerProvider,
};
use serde_json::json;
use std::{
    process::Command,
    sync::{
        Condvar, Mutex, OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

const CASE_ENV: &str = "MF_EVENT_CANCELLATION_CASE";
static CALLS: AtomicUsize = AtomicUsize::new(0);
static TELEMETRY_CANCELLATION: OnceLock<StreamCancellation> = OnceLock::new();
static GATE: OnceLock<(Mutex<(bool, bool)>, Condvar)> = OnceLock::new();

fn cancel(cancellation: &StreamCancellation) {
    cancellation.cancel(StreamError::Execution {
        message: "callback cancellation".into(),
    });
}

#[derive(Debug)]
struct CancelOnFinish;

impl LogProcessor for CancelOnFinish {
    fn emit(&self, record: &mut SdkLogRecord, _: &InstrumentationScope) {
        if record.event_name() == Some("mf.node.finished")
            && let Some(cancellation) = TELEMETRY_CANCELLATION.get()
        {
            cancel(cancellation);
        }
    }

    fn force_flush(&self) -> OTelSdkResult {
        Ok(())
    }

    fn shutdown_with_timeout(&self, _: Duration) -> OTelSdkResult {
        Ok(())
    }
}

struct CancelEvent {
    mode: String,
    cancellation: Option<StreamCancellation>,
}

impl EventNode for CancelEvent {
    fn on_event(
        &mut self,
        event: NodeEvent,
        context: &EventContext<'_>,
    ) -> Result<EventEffects, NodeExecutionError> {
        CALLS.fetch_add(1, Ordering::SeqCst);
        if !matches!(event, NodeEvent::Input(_)) {
            return Ok(EventEffects::default());
        }
        let cancellation = context.input.unwrap().cancellation();
        match self.mode.as_str() {
            "input" => cancel(&cancellation),
            "buffered" => {}
            "external" => {
                let (gate, changed) = GATE.get_or_init(Default::default);
                let mut state = gate.lock().unwrap();
                state.0 = true;
                changed.notify_all();
                drop(changed.wait_while(state, |state| !state.1).unwrap());
            }
            "telemetry" => TELEMETRY_CANCELLATION.set(cancellation.clone()).unwrap(),
            _ => unreachable!(),
        }
        self.cancellation = Some(cancellation);
        Ok(EventEffects {
            emissions: vec![
                NodeResult::from(Outputs::from([("item".into(), json!(42).into())])).into(),
            ],
            timer: if self.mode == "telemetry" {
                TimerUpdate::Cancel
            } else {
                // Cancellation must discard this invalid timer and the emission together.
                TimerUpdate::Set(context.now)
            },
        })
    }

    fn buffered_items(&self) -> Option<usize> {
        if self.mode == "buffered" {
            cancel(self.cancellation.as_ref().unwrap());
        }
        Some(0)
    }
}

inventory::submit! {
    NodeRegistration {
        kind: "test.cancel_event",
        factory: NodeFactory::Plain(|config| {
            Ok(PreparedNode::event(CancelEvent {
                mode: config["mode"].as_str().unwrap().into(),
                cancellation: None,
            }, NodePorts {
                inputs: vec![PortSpec::new("item", ValueType::Any, true)],
                outputs: vec![PortSpec::new("item", ValueType::Int64, true)],
            }))
        }),
    }
}

fn run_case(mode: &str) {
    let definition: WorkflowDefinition = serde_json::from_value(json!({
        "version":"2026-10-03", "execution":{"mode":"stream"}, "dependencies":{},
        "nodes":[
            {"id":"feed", "kind":"builtin.constant", "config":{"value":1}},
            {"id":"event", "kind":"test.cancel_event", "config":{"mode":mode}}],
        "edges":[{"from_node":"feed", "from_output":"value", "to_node":"event", "to_input":"item"}],
        "outputs":[{"name":"item", "node":"event", "port":"item"}]
    }))
    .unwrap();
    let registry = NodeRegistry::from_inventory().unwrap();
    let plan = compile_definition(&definition, &registry).unwrap();
    let capture = capture::Capture::default();
    let traces = SdkTracerProvider::builder().build();
    let logs = SdkLoggerProvider::builder()
        .with_log_processor(capture.clone())
        .with_log_processor(CancelOnFinish)
        .build();
    let observer = Observer::new(&traces, &logs);
    let instance = instantiate_stream(&plan, &registry)
        .unwrap()
        .start_with_options(StreamOptions {
            observation: Some(
                plan.start_stream_observation(&observer, RunId::new())
                    .unwrap(),
            ),
            ..Default::default()
        })
        .unwrap();
    if mode == "external" {
        let (gate, changed) = GATE.get_or_init(Default::default);
        drop(
            changed
                .wait_while(gate.lock().unwrap(), |state| !state.0)
                .unwrap(),
        );
        // These calls must finish while the callback is still waiting.
        assert!(instance.failure().is_none());
        assert_eq!(instance.metrics().pending_frames, 1);
        assert_eq!(instance.summary().emitted_messages, 0);
        cancel(&instance.cancellation());
        gate.lock().unwrap().1 = true;
        changed.notify_all();
    }
    assert!(
        instance
            .recv()
            .unwrap_err()
            .to_string()
            .contains("callback cancellation")
    );
    assert!(
        instance
            .join()
            .unwrap_err()
            .to_string()
            .contains("callback cancellation")
    );
    assert_eq!(CALLS.load(Ordering::SeqCst), 1);
    // recv can observe failure before the callback settles; only the terminal count is final.
    let records = capture.0.lock().unwrap();
    assert!(
        records
            .iter()
            .filter(|record| record.scope == INSTRUMENTATION_SCOPE)
            .map(|record| StreamRecord::decode(record).unwrap())
            .any(|record| matches!(record.payload,
            StreamPayload::Control(StreamEvent::Finished { counts, .. })
            if counts.emitted_messages == u64::from(mode == "telemetry")))
    );
}

#[test]
fn event_callbacks_release_scheduler_lock_and_discard_cancelled_effects() {
    if let Ok(mode) = std::env::var(CASE_ENV) {
        run_case(&mode);
        return;
    }
    // Bound a regression deadlock without leaving blocked workflow threads in the test runner.
    for mode in ["input", "buffered", "external", "telemetry"] {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "event_callbacks_release_scheduler_lock_and_discard_cancelled_effects",
                "--nocapture",
            ])
            .env(CASE_ENV, mode)
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(status.success(), "scenario {mode}: {status}");
                break;
            }
            if Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("scenario {mode} deadlocked");
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}
