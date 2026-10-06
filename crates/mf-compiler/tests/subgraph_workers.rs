extern crate mfn_core as _;

use mf_compiler::{
    NodeBuildError, NodeExecutionError, NodeRegistration, NodeRegistry, WorkflowDefinition,
    compile_definition, instantiate_compiled,
};
use mf_runtime::{
    ExecutionContext, Inputs, NodeResult, Outputs, PortSpec, PreparedNode, RuntimeOptions,
    TaskNode, ValueType,
};
use serde_json::{Value, json};
use std::{collections::BTreeSet, num::NonZeroUsize, sync::Mutex, thread};

struct WorkerProbe {
    workers: usize,
    threads: Mutex<BTreeSet<String>>,
}

impl TaskNode for WorkerProbe {
    fn execute(
        &self,
        _: Inputs,
        context: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        assert_eq!(
            context
                .worker_handle()
                .expect("workflow owns a worker pool")
                .worker_count(),
            self.workers
        );
        assert_eq!(context.worker_limit().get(), self.workers);
        let threads = context.run_parallel(2, |_| {
            (0..2)
                .map(|_| || format!("{:?}", thread::current().id()))
                .collect()
        })?;
        let mut recorded = self.threads.lock().unwrap();
        recorded.extend(threads);
        Ok(Outputs::from([(
            "value".into(),
            json!(recorded.iter().collect::<Vec<_>>()).into(),
        )])
        .into())
    }
}

fn probe_factory(config: Value) -> Result<PreparedNode, NodeBuildError> {
    let value_type = ValueType::List(Box::new(ValueType::String));
    Ok(PreparedNode::new(
        WorkerProbe {
            workers: config["workers"].as_u64().unwrap() as usize,
            threads: Mutex::default(),
        },
        mf_runtime::NodePorts {
            inputs: Vec::new(),
            outputs: vec![PortSpec::new("value", value_type, true)],
        },
    ))
}

inventory::submit! {
    NodeRegistration { kind: "test.worker_probe", factory: mf_runtime::NodeFactory::Plain(probe_factory) }
}

fn workflow(kind: &str, workers: usize) -> WorkflowDefinition {
    let probe = json!({"id": "probe", "kind": "test.worker_probe", "config": {"workers": workers}});
    let (subgraph, input, output, seed) = if kind == "loop" {
        (
            json!({
                "id": "subgraph", "kind": "workflow.loop", "loop": {
                    "max_iterations": 3,
                    "variables": [{"name": "threads", "type": {"list": "string"}}],
                    "body": {
                        "nodes": [probe, {"id": "assign", "kind": "workflow.loop_assign", "config": {"variable": "threads"}}],
                        "edges": [{"from_node": "probe", "from_output": "value", "to_node": "assign", "to_input": "value"}]
                    }
                }
            }),
            "threads",
            "threads",
            json!([]),
        )
    } else {
        (
            json!({
                "id": "subgraph", "kind": "builtin.iteration", "config": {
                    "mode": kind,
                    "body": {"nodes": [probe], "result": {"node": "probe", "port": "value"}}
                }
            }),
            "items",
            "results",
            json!([0, 1, 2]),
        )
    };
    serde_json::from_value(json!({
        "version": "2026-09-29", "dependencies": {},
        "nodes": [{"id": "seed", "kind": "builtin.constant", "config": {"value": seed}}, subgraph],
        "edges": [{"from_node": "seed", "from_output": "value", "to_node": "subgraph", "to_input": input}],
        "outputs": [{"name": "result", "node": "subgraph", "port": output}]
    })).unwrap()
}

#[test]
fn subgraphs_share_workflow_workers() {
    let registry = NodeRegistry::from_inventory().unwrap();
    for kind in ["loop", "sequential", "parallel"] {
        for workers in [1, 6] {
            let plan = compile_definition(&workflow(kind, workers), &registry).unwrap();
            let flow = instantiate_compiled(&plan, &registry).unwrap();
            let output = flow
                .execute_with_options(RuntimeOptions {
                    max_parallel_domains: NonZeroUsize::new(workers).unwrap(),
                })
                .unwrap();
            let output = serde_json::to_value(output).unwrap();
            let values: Vec<_> = if kind == "loop" {
                vec![&output["result"]]
            } else {
                let items = output["result"].as_array().unwrap();
                assert_eq!(items.len(), 3);
                items.iter().collect()
            };
            let threads: BTreeSet<_> = values
                .into_iter()
                .flat_map(|value| value.as_array().unwrap())
                .map(|value| value.as_str().unwrap())
                .collect();
            assert!(!threads.is_empty());
            assert!(threads.len() <= workers, "{kind}: {threads:?}");
        }
    }
}
