use core_nodes as _;
use runtime::{NodeConfig, TaskNode};
use serde_json as json;
use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicUsize, Ordering},
};

compiler::embed_workflow!(pub mod answer);
compiler::embed_workflow!(pub mod repeat);
compiler::embed_workflow!(pub mod iteration);
compiler::embed_workflow!(pub mod missing);

static FACTORIES: AtomicUsize = AtomicUsize::new(0);

#[derive(NodeConfig)]
#[config(runtime = "::runtime")]
struct Config {
    body: runtime::Flow,
    opaque: json::Value,
}

struct Invoke(runtime::Flow);

impl runtime::NodePortContract for Invoke {
    fn ports(&self) -> runtime::NodePorts {
        runtime::NodePorts {
            inputs: vec![],
            outputs: vec![
                runtime::PortSpec::new("value", runtime::ValueType::Any, true),
                runtime::PortSpec::new("host", runtime::ValueType::Any, true),
            ],
        }
    }
}

impl TaskNode for Invoke {
    fn execute(
        &self,
        _: runtime::Inputs,
        _: &mut runtime::ExecutionContext,
    ) -> Result<runtime::NodeResult, runtime::NodeExecutionError> {
        Ok(runtime::NodeResult {
            outputs: self.0.execute()?,
            ..Default::default()
        })
    }
}

fn plain(_: json::Value) -> Result<runtime::PreparedNode, runtime::NodeBuildError> {
    panic!("the declared Flow must be passed to the map factory")
}

fn factory(
    _: &str,
    config: json::Value,
    bodies: BTreeMap<String, runtime::PreparedSubgraph>,
) -> Result<runtime::PreparedNode, runtime::NodeBuildError> {
    assert!(
        std::env::var_os("MF_EMBED_PLAN_ONLY").is_none(),
        "plan called a node factory"
    );
    FACTORIES.fetch_add(1, Ordering::SeqCst);
    let config = Config::from_config(config, bodies)?;
    assert_eq!(
        config.opaque,
        json::json!({
            "path": "does-not-exist.json",
            "body": {"path": "not-a-flow.json"}
        })
    );
    runtime::PreparedNode::new(Invoke(config.body), runtime::NodePorts::default())
}

inventory::submit! {
    runtime::NodeRegistration {
        kind: "host.invoke",
        factory: runtime::NodeFactory::Subgraph {
            flow_parameters: Config::flow_parameters,
            plain,
            prepared: factory,
        },
    }
}

fn builtin() -> Result<runtime::WorkflowDefinition, runtime::DefinitionParseError> {
    runtime::WorkflowDefinition::from_json(
        r#"{
        "version":"2026-10-25", "dependencies":{},
        "nodes":[{"id":"source","kind":"builtin.constant","config":{"value":91}}],
        "outputs":[{"name":"value","node":"source","port":"value"}]
    }"#,
    )
}

runtime::register_builtin_flow!("answer", builtin);

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::args().nth(1).as_deref() == Some("output-directory") {
        println!("{}", env!("OUT_DIR"));
        return Ok(());
    }
    let registry = runtime::NodeRegistry::from_inventory()?;
    if std::env::args().nth(1).as_deref() == Some("missing") {
        let error = missing::plan(&registry).unwrap_err();
        let mut source: Option<&(dyn std::error::Error + 'static)> = Some(&error);
        let mut messages = Vec::new();
        while let Some(error) = source {
            messages.push(error.to_string());
            source = error.source();
        }
        assert!(
            messages
                .iter()
                .any(|message| message.contains("not embedded")),
            "{messages:?}"
        );
        assert_eq!(FACTORIES.load(Ordering::SeqCst), 0);
        println!("missing source rejected");
        return Ok(());
    }
    let plan = answer::plan(&registry)?;
    assert_eq!(plan.graph.definition.nodes[0].kind, "host.invoke");
    let _ = repeat::plan(&registry)?;
    let _ = iteration::plan(&registry)?;
    assert_eq!(FACTORIES.load(Ordering::SeqCst), 0);
    if std::env::var_os("MF_EMBED_PLAN_ONLY").is_some() {
        println!("metadata only");
        return Ok(());
    }
    // Manifest discovery may prepare interfaces, but plan itself must not do so.
    let _ = answer::manifest(&registry)?;
    let before = FACTORIES.load(Ordering::SeqCst);
    let flow = answer::prepare_workflow(&registry, None)?;
    assert!(FACTORIES.load(Ordering::SeqCst) > before);
    let runtime = runtime::FlowRuntime::default();
    println!(
        "{}",
        json::json!({
            "answer": runtime.execute(&flow)?,
            "repeat": runtime.execute(&repeat::prepare_workflow(&registry, None)?)?,
            "iteration": runtime.execute(&iteration::prepare_workflow(&registry, None)?)?,
        })
    );
    Ok(())
}
