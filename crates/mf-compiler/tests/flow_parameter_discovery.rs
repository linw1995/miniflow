//! Standalone discovery follows typed parameters across newly discovered providers.
#![cfg(feature = "codegen")]

extern crate mfn_core as _;
#[path = "fixtures/flow-arguments-provider/src/lib.rs"]
mod choose;
mod common;
#[path = "fixtures/flow-argument-stream-provider/src/lib.rs"]
mod stream_provider;

use mf_compiler::{CompileRequest, NodeRegistry, SupportPackages, compile_project};
use serde_json::{Value, json};
use std::{fs, path::Path, process::Command};

fn copy_provider(root: &Path, name: &str) -> std::path::PathBuf {
    let fixture = common::crates_dir()
        .join("mf-compiler/tests/fixtures")
        .join(name);
    let target = root.join(name);
    fs::create_dir_all(target.join("src")).unwrap();
    let manifest = fs::read_to_string(fixture.join("Cargo.toml"))
        .unwrap()
        .replace(
            "\"../../../../mf-runtime\"",
            &serde_json::to_string(&common::crates_dir().join("mf-runtime")).unwrap(),
        );
    fs::write(target.join("Cargo.toml"), manifest).unwrap();
    fs::copy(fixture.join("src/lib.rs"), target.join("src/lib.rs")).unwrap();
    target
}

#[test]
fn generated_discovery_follows_new_provider_contracts_and_freezes_relative_parameters() {
    let root = tempfile::tempdir().unwrap();
    let chooser = copy_provider(root.path(), "flow-arguments-provider");
    let streaming = copy_provider(root.path(), "flow-argument-stream-provider");
    let bodies = root.path().join("bodies");
    fs::create_dir_all(&bodies).unwrap();
    let grandchild = bodies.join("increments.json");
    fs::write(
        &grandchild,
        serde_json::to_vec(&json!({
            "version":"2026-10-28", "dependencies":{},
            "nodes":[{"id":"step", "kind":stream_provider::SOURCE_KIND}],
            "outputs":[
                {"name":"increment", "node":"step", "port":"delta"},
                {"name":"count", "node":"step", "port":"count"},
                {"name":"subscribed", "node":"step", "port":"subscribed"}
            ]
        }))
        .unwrap(),
    )
    .unwrap();
    let child = bodies.join("body.json");
    fs::write(&child, serde_json::to_vec(&json!({
        "version":"2026-10-28",
        "dependencies":{
            "core":{"package":"mfn-core", "path":common::crates_dir().join("builtin-nodes/core")},
            "new_provider":{"package":"fixture-flow-argument-stream-provider", "path":streaming}
        },
        "nodes":[
            {"id":"leaf", "kind":"builtin.constant", "config":{"value":"resolved"}},
            {"id":"nested", "kind":stream_provider::KIND,
                "config":{"local":{"path":"increments.json"}}}
        ],
        "outputs":[{"name":"content", "node":"leaf", "port":"value"}]
    })).unwrap()).unwrap();
    let definition = json!({
        "version":"2026-10-28",
        // Neither the core nor the nested consumer's provider is declared by the root.
        "dependencies":{"chooser":{"package":"fixture-flow-arguments-provider", "path":chooser}},
        "nodes":[
            {"id":"params", "kind":"builtin.parameters", "config":{
                "parameters":{"select":{"type":"string", "default":"left", "secret":false}}
            }},
            {"id":"custom", "kind":choose::KIND, "config":{"left":{"path":"bodies/body.json"}},
                "inputs":{"select":{"from":{"node":"params", "port":"select"}}}}
        ],
        "outputs":[{"name":"content", "node":"custom", "port":"content"}]
    });
    let source = root.path().join("root.json");
    fs::write(&source, serde_json::to_vec(&definition).unwrap()).unwrap();

    let registry = NodeRegistry::from_inventory().unwrap();
    let resolved = mf_compiler::load_workflow_with_registry(&source, &registry).unwrap();
    assert!(
        resolved
            .sources
            .contains(&fs::canonicalize(&child).unwrap())
    );
    assert!(
        resolved
            .sources
            .contains(&fs::canonicalize(&grandchild).unwrap())
    );
    let frozen = serde_json::to_value(&resolved.definition).unwrap();
    assert!(frozen.get("flow_arguments").is_none());
    assert!(
        frozen
            .pointer("/nodes/1/config/left/nodes/1/config/local")
            .is_some()
    );
    let plan = mf_compiler::compile_resolved_definition(&resolved.definition, &registry).unwrap();
    let graph = mf_compiler::describe_compiled(&plan).unwrap();
    assert_eq!(graph.nodes.len(), 2);
    assert_eq!(graph.loop_bodies.len(), 2);
    assert_eq!(
        serde_json::to_value(
            mf_compiler::instantiate_compiled(&plan, &registry)
                .unwrap()
                .execute()
                .unwrap()
        )
        .unwrap(),
        json!({"content":"resolved"})
    );

    let output = root.path().join("runner");
    let build = root.path().join("build");
    let mut locked_bytes = None;
    for locked in [false, true] {
        compile_project(&CompileRequest {
            definition: &source,
            output: &output,
            locked,
            build_dir: Some(&build),
            support: &SupportPackages::Local {
                crates_dir: common::crates_dir(),
            },
        })
        .unwrap();
        let bytes = fs::read(source.with_extension("lock")).unwrap();
        if let Some(previous) = &locked_bytes {
            assert_eq!(
                &bytes, previous,
                "locked discovery must not rewrite the authoritative lock"
            );
        } else {
            locked_bytes = Some(bytes);
        }
    }
    fs::remove_file(&source).unwrap();
    fs::remove_dir_all(&bodies).unwrap();
    fs::remove_dir_all(&chooser).unwrap();
    fs::remove_dir_all(&streaming).unwrap();
    fs::remove_dir_all(&build).unwrap();
    for _ in 0..2 {
        let result = Command::new(&output)
            .current_dir(root.path())
            .env("PATH", "")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&result.stdout).unwrap(),
            json!({"content":"resolved"})
        );
    }
}
