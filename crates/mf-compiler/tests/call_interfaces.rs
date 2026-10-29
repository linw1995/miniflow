extern crate mfn_code as _;
extern crate mfn_core as _;
extern crate mfn_llm as _;
#[path = "fixtures/connected_tools.rs"]
mod connected;
#[path = "fixtures/multi-nodes/src/directed_fixture.rs"]
mod fixture;
use mf_compiler::{NodeRegistry, WorkflowDefinition, compile_definition, instantiate_compiled};
use serde_json::{Value, json};

fn catalog(mut document: Value) -> Value {
    document["nodes"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"catalog","kind":"builtin.tool_catalog","config":{"source":"a_parent"}}));
    document["outputs"]
        .as_array_mut()
        .unwrap()
        .push(json!({"name":"catalog","node":"catalog","port":"tools"}));
    document
}
fn prepare(value: Value) -> Result<mf_runtime::Flow, Box<dyn std::error::Error>> {
    let registry = NodeRegistry::from_inventory()?;
    let definition: WorkflowDefinition = serde_json::from_value(value)?;
    let plan = compile_definition(&definition, &registry)?;
    Ok(instantiate_compiled(&plan, &registry)?)
}

#[test]
fn read_only_catalogs_derive_only_connected_arguments_without_running_the_owner() {
    let mut document = catalog(connected::document());
    document["nodes"][0]["config"]["repeat"] = 0.into();
    document["edges"] = json!([{"from_node":"catalog","to_node":"a_parent"}]);
    let outputs = prepare(document).unwrap().execute().unwrap();
    let tools = serde_json::to_value(&outputs["catalog"]).unwrap();
    assert_eq!(tools[0]["name"], "search");
    let encoded = tools.to_string();
    assert!(encoded.contains("query"));
    assert!(
        !encoded.contains("token")
            && !encoded.contains("credential")
            && !encoded.contains("internal")
    );
    assert_eq!(outputs["result"].as_str(), Some("unselected"));
}

#[test]
fn invalid_and_chained_catalog_sources_fail_during_preparation() {
    for source in ["missing", "catalog"] {
        let mut document = catalog(connected::document());
        document["nodes"][4]["config"]["source"] = source.into();
        assert!(prepare(document).is_err());
    }
    let mut document = catalog(connected::document());
    document["nodes"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"viewer","kind":"builtin.tool_catalog","config":{"source":"catalog"}}));
    assert!(prepare(document).is_err());
}

#[test]
fn nested_workflows_keep_independent_catalogs_for_reused_node_ids() {
    let mut inner = catalog(connected::document());
    inner["nodes"][0]["config"]["repeat"] = 0.into();
    let mut document = connected::document();
    let search = document["calls"]["a_parent"]
        .as_object_mut()
        .unwrap()
        .remove("search")
        .unwrap();
    document["calls"]["a_parent"]["outer_search"] = search;
    document["nodes"][0]["config"] = json!({"alias":"outer_search","capture_outputs":true});
    document["nodes"][2]["config"]["definition"] = inner;
    document["nodes"][2]
        .as_object_mut()
        .unwrap()
        .remove("inputs");
    let outputs = prepare(catalog(document)).unwrap().execute().unwrap();
    let root = serde_json::to_value(&outputs["catalog"]).unwrap();
    let child = serde_json::to_value(&outputs["result"]).unwrap();
    assert_eq!(root[0]["name"], "outer_search");
    assert_eq!(child["catalog"][0]["name"], "search");
}
