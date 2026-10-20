use mf_runtime::{
    ExecutionOutputs, FlowNode, NodePorts, OutputVisibility, Outputs, PortSpec, ValueType,
};
use std::{hint::black_box, time::Instant};

fn main() {
    let args: Vec<_> = std::env::args().collect();
    let count: usize = args.get(1).map_or(1000, |value| value.parse().unwrap());
    let rounds: usize = args.get(2).map_or(10000, |value| value.parse().unwrap());
    assert!(count > 0 && rounds > 0);
    let values: Outputs = (0..count)
        .map(|index| (format!("node{index:08}.value"), (index as u64).into()))
        .collect();
    let visibility = OutputVisibility::from_sorted(values.keys().cloned().collect());
    let mut outputs = ExecutionOutputs::from_values(values);
    let node = FlowNode {
        definition_id: "result".into(),
        node: (),
        metadata: NodePorts {
            inputs: vec![],
            outputs: vec![PortSpec::new("value", ValueType::Uint64, true)],
        }
        .into(),
    };
    let started = Instant::now();
    for _ in 0..rounds {
        black_box(outputs.fork_snapshot(Some(visibility.clone())));
    }
    let fork_ns = started.elapsed().as_nanos() / rounds as u128;
    let started = Instant::now();
    for index in 0..rounds {
        let mut child = outputs.fork_snapshot(Some(visibility.clone()));
        black_box(child.output("node00000000.value").unwrap());
        child
            .publish(
                &node,
                Some(Outputs::from([("value".into(), (index as u64).into())]).into()),
            )
            .unwrap();
        outputs.merge_nodes(&child, [&node]);
    }
    let transaction_ns = started.elapsed().as_nanos() / rounds as u128;
    let writes: Vec<(String, mf_runtime::ValueRef)> = (0..count)
        .map(|index| (format!("node{index:08}.value"), (index as u64).into()))
        .collect();
    let write_rounds = rounds.min(20);
    let started = Instant::now();
    for _ in 0..write_rounds {
        let mut state = ExecutionOutputs::default();
        for (key, value) in &writes {
            state.set_value(key.clone(), value.clone());
        }
        black_box(state);
    }
    let insert_ns = started.elapsed().as_nanos() / (write_rounds * count) as u128;
    println!(
        "{}",
        serde_json::json!({
            "outputs": count,
            "rounds": rounds,
            "fork_ns": fork_ns,
            "read_write_commit_ns": transaction_ns,
            "insert_ns": insert_ns,
        })
    );
}
