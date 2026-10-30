#![cfg(feature = "tokio")]

use mf_runtime::{
    Emitter, ExecutionContext, ExecutionDomain, ExecutionDomains, FlowDependency, FlowNode, Inputs,
    NodeExecution, NodeExecutionError, NodeId, NodePorts, NodeResult, OutputVisibility, Outputs,
    PortSpec, PreparedNode, PreparedStream, StreamExecution, StreamLimits, StreamNode, TaskNode,
    ValueType,
};
use serde_json::json;
use std::{
    borrow::Cow,
    future::pending,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

struct DropProbe(Arc<AtomicBool>);

impl Drop for DropProbe {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

struct Source;

impl StreamNode for Source {
    fn execute(
        &mut self,
        _: Inputs,
        context: &mut ExecutionContext,
        emitter: &mut Emitter<'_>,
    ) -> Result<(), NodeExecutionError> {
        let backend = context.execute_future(
            async { tokio::runtime::Handle::current().id().to_string() },
            None,
        )?;
        for item in 0..8 {
            emitter.send(
                Outputs::from([
                    ("item".into(), json!(item).into()),
                    ("backend".into(), json!(backend).into()),
                ])
                .into(),
            )?;
        }
        Ok(())
    }
}

struct Inspect {
    backend_dropped: Arc<AtomicBool>,
    pending: Option<(mpsc::Sender<()>, Arc<AtomicBool>)>,
}

impl TaskNode for Inspect {
    fn execute(
        &self,
        inputs: Inputs,
        context: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        assert!(context.output("inspect.item").is_err());
        let item = inputs["item"].as_i64().unwrap();
        let backend = context.execute_future(
            async {
                tokio::time::sleep(Duration::from_millis(1)).await;
                let backend = tokio::runtime::Handle::current();
                assert_eq!(backend.metrics().num_workers(), 1);
                backend.id().to_string()
            },
            None,
        )?;
        assert_eq!(inputs["backend"], json!(backend));
        assert!(!self.backend_dropped.load(Ordering::SeqCst));
        if item == 0 {
            let dropped = self.backend_dropped.clone();
            let (started, receiver) = mpsc::channel();
            context.execute_future(
                async move {
                    tokio::spawn(async move {
                        let _probe = DropProbe(dropped);
                        started.send(()).unwrap();
                        pending::<()>().await;
                    });
                },
                None,
            )?;
            receiver.recv_timeout(Duration::from_secs(5)).unwrap();
        } else if let Some((started, dropped)) = &self.pending {
            context.execute_future(
                async {
                    let _probe = DropProbe(dropped.clone());
                    started.send(()).unwrap();
                    pending::<()>().await;
                },
                None,
            )?;
        }
        Ok(inputs.into())
    }
}

fn program(
    execution_domains: ExecutionDomains,
    dependencies: Cow<'static, [Cow<'static, [FlowDependency]>]>,
) -> mf_runtime::StreamProgram {
    use mf_runtime::{MessageSchedule, StreamEmissionBinding, StreamMessageProgram, StreamProgram};
    StreamProgram {
        graph: Default::default(),
        execution_domains,
        messages: vec![
            StreamMessageProgram {
                source: None,
                schedule: MessageSchedule {
                    domains: vec![0].into(),
                    local: vec![0, usize::MAX].into(),
                    initial_deps: vec![0].into(),
                    successors: vec![Box::from([])].into(),
                },
                select_outputs: None,
            },
            StreamMessageProgram {
                source: Some(0),
                schedule: MessageSchedule {
                    domains: vec![1].into(),
                    local: vec![usize::MAX, 0].into(),
                    initial_deps: vec![0].into(),
                    successors: vec![Box::from([])].into(),
                },
                select_outputs: Some(Arc::new(|context| {
                    let mut outputs = mf_runtime::FlowOutputs::new();
                    for port in ["item", "backend"] {
                        if let Some(value) = context.select_output(port, "inspect", port, false)? {
                            outputs.insert(port.into(), value);
                        }
                    }
                    Ok(outputs)
                })),
            },
        ],
        emissions: vec![
            Some(StreamEmissionBinding {
                message: 1,
                connected: true,
                terminal: false,
                initialize: Arc::new(|plan, result| {
                    ExecutionContext::for_message(plan.emission_node(0), result)
                }),
            }),
            None,
        ],
        inputs: dependencies
            .into_owned()
            .into_iter()
            .enumerate()
            .map(|(index, dependencies)| {
                Arc::new(
                    move |plan: &mf_runtime::StreamPlan,
                          context: &ExecutionContext,
                          inputs: &mut Inputs,
                          causes| {
                        context.resolve_node_inputs(
                            &plan.nodes()[index],
                            dependencies.iter().map(FlowDependency::borrowed),
                            &[],
                            inputs,
                            causes,
                        )
                    },
                ) as Arc<mf_runtime::StreamInputResolver>
            })
            .collect(),
    }
}

fn prepare(inspect: Inspect) -> PreparedStream {
    let ports = vec![
        PortSpec::new("item", ValueType::Int64, true),
        PortSpec::new("backend", ValueType::String, true),
    ];
    PreparedStream::from_plan(
        StreamExecution {
            limits: StreamLimits {
                workers: 1,
                max_pending_messages: 1,
            },
        },
        vec![
            FlowNode::new(
                "source",
                PreparedNode::from_parts(
                    NodeExecution::Stream(Box::new(Source)),
                    NodePorts {
                        inputs: vec![],
                        outputs: ports.clone(),
                    },
                ),
            ),
            FlowNode::new(
                "inspect",
                PreparedNode::from_parts(
                    NodeExecution::Task(Box::new(inspect)),
                    NodePorts {
                        inputs: ports.clone(),
                        outputs: ports,
                    },
                ),
            ),
        ],
        program(
            ExecutionDomains::from_parts(
                (0..2)
                    .map(|id| ExecutionDomain {
                        id,
                        nodes: vec![NodeId::new(id)].into(),
                        positions: vec![id].into(),
                        predecessors: vec![].into(),
                        successors: vec![].into(),
                        first_position: id,
                        snapshot_visibility: OutputVisibility::Borrowed(&[
                            "source.backend",
                            "source.item",
                        ]),
                    })
                    .collect::<Vec<_>>()
                    .into(),
                vec![Cow::Owned(vec![]), Cow::Owned(vec![])].into(),
            ),
            vec![
                Cow::Owned(vec![]),
                Cow::Owned(
                    ["item", "backend"]
                        .into_iter()
                        .map(|port| FlowDependency {
                            exclusive: false,
                            input: Some(port.into()),
                            source_node: "source".into(),
                            source_output: port.into(),
                            source_key: format!("source.{port}").into(),
                        })
                        .collect(),
                ),
            ]
            .into(),
        ),
        Default::default(),
    )
}

#[test]
fn startup_and_messages_share_a_backend_until_stream_shutdown() {
    let mut previous_backend = None;
    for _ in 0..2 {
        let dropped = Arc::new(AtomicBool::new(false));
        let instance = prepare(Inspect {
            backend_dropped: dropped.clone(),
            pending: None,
        })
        .start()
        .unwrap();
        let mut backend = None;
        for item in 0..8 {
            let output = instance.recv().unwrap().unwrap();
            assert_eq!(output.outputs["item"], json!(item));
            let current = output.outputs["backend"].clone();
            assert_eq!(backend.get_or_insert(current.clone()), &current);
        }
        assert!(instance.recv().unwrap().is_none());
        // The caller may join inside an async host; backend destruction stays on workers.
        let host = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let summary = host.block_on(async { instance.join().unwrap() });
        assert_eq!(summary.delivered_outputs, 8);
        assert!(dropped.load(Ordering::SeqCst));
        assert_ne!(backend, previous_backend);
        previous_backend = backend;
    }
}

#[test]
fn cancellation_drops_pending_message_futures_and_the_shared_backend() {
    let backend_dropped = Arc::new(AtomicBool::new(false));
    let pending_dropped = Arc::new(AtomicBool::new(false));
    let (started, receiver) = mpsc::channel();
    let instance = prepare(Inspect {
        backend_dropped: backend_dropped.clone(),
        pending: Some((started, pending_dropped.clone())),
    })
    .start()
    .unwrap();
    assert_eq!(instance.recv().unwrap().unwrap().outputs["item"], json!(0));
    receiver.recv_timeout(Duration::from_secs(5)).unwrap();
    instance.cancellation().cancel(
        mf_runtime::StreamExecutionSnafu {
            message: "stop pending message",
        }
        .build(),
    );
    assert!(instance.join().is_err());
    assert!(pending_dropped.load(Ordering::SeqCst));
    assert!(backend_dropped.load(Ordering::SeqCst));
}
