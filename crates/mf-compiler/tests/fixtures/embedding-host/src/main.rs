use code_nodes as _;
use core_nodes as _;
use fixture_provider as _;

compiler::embed_workflow!(pub mod answer);
compiler::embed_workflow!(pub mod events);
compiler::embed_workflow!(pub mod contract);
compiler::embed_workflow!(pub mod contract_stream);
compiler::embed_workflow!(pub mod typed);
compiler::embed_workflow!(pub mod counter);
compiler::embed_workflow!(pub mod repeat);
compiler::embed_workflow!(pub mod iteration);

mod capture {
    use json as serde_json;
    include!("capture.rs");
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var_os("MF_EMBEDDING_OUTPUT").is_some() {
        println!("{}", env!("OUT_DIR"));
        return Ok(());
    }
    let registry = runtime::NodeRegistry::from_inventory()?;
    if std::env::var_os("MF_EMBEDDING_DESCRIBE").is_some() {
        println!(
            "{:?}/{:?}",
            contract::manifest(&registry)?.input_schema.inputs["source"]["path./~"].value_type,
            contract_stream::manifest(&registry)?.input_schema.inputs["source"]["path./~"]
                .value_type,
        );
        return Ok(());
    }
    if std::env::var_os("MF_EMBEDDING_VALIDATE").is_some() {
        let _ = answer::prepare_workflow(&registry, None)?;
        return Ok(());
    }
    if std::env::var_os("MF_EMBEDDING_VALIDATE_CONTRACT").is_some() {
        if std::env::var_os("MF_EMBEDDING_VALIDATE_STREAM").is_some() {
            let _ = contract_stream::prepare_stream(&registry)?;
            return Ok(());
        }
        let harness = capture::Harness::new(true);
        let observation = contract::plan()?
            .start_observation(&harness.observer(), mf_telemetry::identity::RunId::new())?;
        let result = runtime::ExecutionContext::run(Some(observation), |state| {
            contract::prepare_workflow(&registry, state.observation_mut())
        });
        let error = match result {
            Ok(_) => panic!("invalid node configuration was accepted"),
            Err(error) => error,
        };
        let mut current: Option<&(dyn std::error::Error + 'static)> = Some(&error);
        let mut found = false;
        while let Some(source) = current {
            found |= source.is::<runtime::NodeBuildError>();
            current = source.source();
        }
        assert!(found, "{error}");
        let records = harness.records();
        assert!(records.iter().any(|record| {
            record.event_name == "mf.node.finished"
                && record.attributes["mf.outcome"] == "failed"
                && record.attributes["mf.node.id"] == "source"
                && record.attributes["mf.failure.phase"] == "preparation"
        }));
        assert_eq!(
            records.last().unwrap().attributes["mf.failure.phase"],
            "preparation"
        );
        return Err(error.into());
    }
    let runtime = runtime::FlowRuntime::default();
    let counter = counter::prepare_workflow(&registry, None)?;
    assert_eq!(runtime.execute(&counter)?["count"].as_u64(), Some(0));
    assert_eq!(runtime.execute(&counter)?["count"].as_u64(), Some(1));
    assert_eq!(
        runtime.execute(&counter::prepare_workflow(&registry, None)?)?["count"].as_u64(),
        Some(0)
    );
    assert_eq!(
        runtime.execute(&repeat::prepare_workflow(&registry, None)?)?["count"].as_i64(),
        Some(3)
    );
    assert_eq!(
        json::to_value(runtime.execute(&iteration::prepare_workflow(&registry, None)?)?)?,
        json::json!({"results":[2,5,8]})
    );
    let mut typed_context = runtime::ExecutionContext::default();
    typed_context.set_workflow_arguments(runtime::WorkflowArguments::try_from(
        json::json!({"source.text":"hello"}),
    )?);
    runtime.execute_in_context(
        &typed::prepare_workflow(&registry, None)?,
        &mut typed_context,
    )?;
    match typed_context.output("source.text")? {
        runtime::ContextValue::Value(value) => assert_eq!(value.as_str(), Some("hello")),
        runtime::ContextValue::Skipped => {
            panic!("unobserved intermediate typed output was skipped")
        }
    }
    let harness = capture::Harness::new(true);
    let manifest = typed::manifest(&registry)?;
    let workflow_id = manifest.workflow_id()?;
    let observation = harness.observer().start_with_description(
        manifest.description()?,
        mf_telemetry::identity::RunId::new(),
    )?;
    let typed_outputs = runtime::ExecutionContext::run(
        Some(observation),
        |state| -> Result<_, Box<dyn std::error::Error>> {
            let flow = typed::prepare_workflow(&registry, state.observation_mut())?;
            state.set_workflow_arguments(runtime::WorkflowArguments::try_from(
                json::json!({"source.text":"hello"}),
            )?);
            let result = runtime.execute_in_context(&flow, state)?;
            match state.output("source.text")? {
                runtime::ContextValue::Value(value) => assert_eq!(value.as_str(), Some("hello")),
                runtime::ContextValue::Skipped => panic!("intermediate typed output was skipped"),
            }
            Ok(result)
        },
    )?;
    assert_eq!(typed_outputs["reply"].as_str(), Some("hello"));
    let records = harness.records();
    assert_eq!(records.len(), 6);
    assert_eq!(records.first().unwrap().event_name, "mf.workflow.started");
    assert_eq!(records.last().unwrap().event_name, "mf.workflow.finished");
    assert!(
        records
            .iter()
            .all(|record| record.attributes["mf.workflow.id"]
                == json::json!(workflow_id.to_string()))
    );
    assert_eq!(harness.spans.get_finished_spans()?.len(), 3);
    let flow = answer::prepare_workflow(&registry, None)?;
    let outputs = runtime.execute(&flow)?;
    let prepared = events::prepare_stream(&registry)?;
    let options = runtime::StreamOptions {
        stdin: Some(runtime::TextInput::new(std::fs::File::open(
            std::env::args_os().nth(1).unwrap(),
        )?)),
        ..Default::default()
    };
    let stream = runtime.start_stream(prepared, options)?;
    let mut messages = Vec::new();
    while let Some(message) = stream.recv()? {
        messages.push(json::to_value(message.outputs)?);
    }
    stream.join()?;
    println!("{}", json::json!({"answer": outputs, "events": messages}));
    Ok(())
}
