use mf_runtime::{
    ExecutionContext, NodeBuildError, NodeExecutionError, NodeFactory, NodeInputs, NodePorts,
    NodeRegistration, NodeResult, Outputs, PortSpec, PreparedNode, TypedTaskNode, ValueRef,
    ValueType, deserialize_config,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::PathBuf};

#[derive(NodeInputs)]
struct StructInputs<T> {
    count: T,
    ratio: f64,
    active: bool,
    #[input(rename = "rows./~")]
    rows: Vec<BTreeMap<String, i64>>,
    label: Option<String>,
    raw: Option<ValueRef>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    marker: Option<PathBuf>,
}

struct StructTask(Config);
impl TypedTaskNode for StructTask {
    type Input = StructInputs<i64>;

    fn execute(
        &self,
        input: Self::Input,
        _: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        if let Some(path) = &self.0.marker {
            std::fs::write(path, b"executed").unwrap();
        }
        let raw_present = input.raw.is_some();
        let summary = ValueRef::object([
            ("count".into(), input.count.into()),
            ("ratio".into(), json!(input.ratio).into()),
            ("active".into(), input.active.into()),
            ("rows".into(), json!(input.rows).into()),
            ("label".into(), json!(input.label).into()),
            ("raw".into(), input.raw.unwrap_or_else(ValueRef::null)),
            ("raw_present".into(), raw_present.into()),
        ]);
        Ok(Outputs::from([("value".into(), summary)]).into())
    }
}

struct DriftTask;
impl TypedTaskNode for DriftTask {
    type Input = StructInputs<String>;

    fn execute(
        &self,
        _: Self::Input,
        _: &mut ExecutionContext,
    ) -> Result<NodeResult, NodeExecutionError> {
        panic!("manifest drift must prevent business dispatch");
    }
}

fn factory(config: Value) -> Result<PreparedNode, NodeBuildError> {
    let config = deserialize_config(config)?;
    let ports = NodePorts {
        inputs: vec![],
        outputs: vec![PortSpec::new("value", ValueType::Object, true)],
    };
    // Deliberately violate interface stability to exercise the manifest guard.
    if std::env::var_os("MF_FIXTURE_TYPED_INPUT_DRIFT").is_some() {
        PreparedNode::typed_task(DriftTask, ports)
    } else {
        PreparedNode::typed_task(StructTask(config), ports)
    }
}

inventory::submit! { NodeRegistration { kind: "fixture.struct_inputs", factory: NodeFactory::Plain(factory) } }
