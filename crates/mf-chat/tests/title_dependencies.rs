#[path = "../build/title.rs"]
pub mod title;

use serde_json::{Value, json};
use std::process::Command;

fn template() -> Value {
    serde_json::from_str(include_str!("../workflows/title.json")).unwrap()
}

fn manifest() -> Value {
    let dependencies: Vec<_> = [
        ("mfn-core", "^1.2", false, vec!["core-feature"]),
        ("mfn-code", "~2.3", true, vec![]),
        ("mfn-llm", "=3.4.5", true, vec!["llm-feature"]),
    ]
    .into_iter()
    .map(|(name, req, defaults, features)| {
        json!({
            "name":name, "req":req, "kind":null, "target":null, "optional":false,
            "source":null, "path":"/checkout/provider",
            "features":features, "uses_default_features":defaults
        })
    })
    .collect();
    json!({"name":"mf-chat", "version":"9.8.7", "dependencies":dependencies})
}

#[test]
fn title_dependencies_follow_provider_requirements_and_exclude_development_features() {
    let mut package = manifest();
    let mut development = package["dependencies"][2].clone();
    development["kind"] = json!("dev");
    development["features"] = json!(["test-support"]);
    package["dependencies"]
        .as_array_mut()
        .unwrap()
        .insert(0, development);
    let mut definition = template();
    title::bind_dependencies(&mut definition, &package).unwrap();
    let definition = mf_runtime::WorkflowDefinition::from_json(&definition.to_string()).unwrap();
    for (alias, version, defaults, features) in [
        ("core", "^1.2", false, vec!["core-feature"]),
        ("code", "~2.3", true, vec![]),
        ("llm", "=3.4.5", true, vec!["llm-feature"]),
    ] {
        let dependency = &definition.dependencies[alias];
        assert_eq!(dependency.version.as_deref(), Some(version));
        assert_eq!(dependency.default_features, defaults);
        assert_eq!(dependency.features, features);
        assert!(dependency.path.is_none());
    }
    package["dependencies"].as_array_mut().unwrap().remove(0);
    for provider in package["dependencies"].as_array_mut().unwrap() {
        provider["source"] = json!("registry+https://github.com/rust-lang/crates.io-index");
        provider.as_object_mut().unwrap().remove("path");
    }
    let mut published = template();
    title::bind_dependencies(&mut published, &package).unwrap();
    assert_eq!(
        mf_runtime::WorkflowDefinition::from_json(&published.to_string()).unwrap(),
        definition
    );
}

#[test]
fn title_dependencies_reject_missing_or_nonportable_providers() {
    for (field, value, message) in [
        ("kind", json!("dev"), "normal, unconditional"),
        ("target", json!("cfg(unix)"), "normal, unconditional"),
        ("optional", json!(true), "normal, unconditional"),
        ("req", json!("*"), "portable registry version"),
        (
            "source",
            json!("git+https://example.invalid/provider"),
            "portable registry version",
        ),
    ] {
        let mut package = manifest();
        package["dependencies"][0][field] = value;
        let error = title::bind_dependencies(&mut template(), &package).unwrap_err();
        assert!(error.to_string().contains("mfn-core"));
        assert!(error.to_string().contains(message));
    }
}

#[test]
fn registered_title_dependencies_match_the_chat_cargo_manifest() {
    let output = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .args(["read-manifest", "--manifest-path"])
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
        .output()
        .unwrap();
    assert!(output.status.success());
    let package = serde_json::from_slice(&output.stdout).unwrap();
    let mut expected = template();
    title::bind_dependencies(&mut expected, &package).unwrap();
    assert_eq!(
        mf_chat::flows::title_definition().unwrap(),
        mf_runtime::WorkflowDefinition::from_json(&expected.to_string()).unwrap()
    );
}
