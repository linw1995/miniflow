extern crate mfn_core as _;

use mf_compiler::{NodeRegistry, execute_compiled, plan_definition};
use mf_telemetry::{identity::RunId, observation::Observer};
use opentelemetry_sdk::{
    logs::{InMemoryLogExporter, SdkLoggerProvider},
    trace::{InMemorySpanExporter, SdkTracerProvider},
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let definition = serde_json::from_str(
        r#"{
        "version":"2026-09-26", "dependencies":{},
        "nodes":[{"id":"answer","kind":"builtin.constant","config":{"value":42}}],
        "outputs":[{"name":"answer","node":"answer","port":"value"}]
    }"#,
    )?;
    let plan = plan_definition(&definition)?;
    let span_exporter = InMemorySpanExporter::default();
    let log_exporter = InMemoryLogExporter::default();
    let traces = SdkTracerProvider::builder()
        .with_simple_exporter(span_exporter.clone())
        .build();
    let logs = SdkLoggerProvider::builder()
        .with_simple_exporter(log_exporter.clone())
        .build();
    let observer = Observer::new(&traces, &logs);
    let observation = plan.start_observation(&observer, RunId::new())?;
    let registry = NodeRegistry::from_inventory()?;
    let result = execute_compiled(&plan, &registry, Some(observation));
    // Provider lifetime belongs to the application, including handled failure paths.
    traces.force_flush()?;
    logs.force_flush()?;
    for log in log_exporter.get_emitted_logs()? {
        println!("{}", log.record.event_name().unwrap_or("unnamed"));
    }
    println!("spans: {}", span_exporter.get_finished_spans()?.len());
    traces.shutdown()?;
    logs.shutdown()?;
    println!("{}", serde_json::to_string(&result?)?);
    Ok(())
}
