use mfn_lsp::*;
use serde_json::json;
use std::{fs, process::Command};

#[test]
#[ignore = "requires explicitly configured MF_LSP_SMOKE_SERVER from the pinned toolchain"]
fn pinned_server_analyzes_a_real_rust_workspace() {
    let server = std::env::var("MF_LSP_SMOKE_SERVER")
        .expect("configure the pinned language server explicitly");
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    fs::create_dir(root.join("src")).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"lsp-smoke\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[workspace]\n",
    )
    .unwrap();
    let source = "pub fn value() -> u32 { 1 }\n#[test] fn checks_value() { let _ = value(); }\n";
    fs::write(root.join("src/lib.rs"), source).unwrap();
    assert!(
        Command::new("cargo")
            .args(["generate-lockfile", "--offline"])
            .current_dir(root)
            .status()
            .unwrap()
            .success()
    );
    let config = ServerConfig {
        command: server,
        args: vec![],
        profile: Profile::RustAnalyzer,
        extensions: vec!["rs".into()],
        deadline_ms: 30_000,
        max_requests: 128,
        max_bytes: 1024 * 1024,
    };
    let request = AnalysisRequest {
        snapshots: vec![Snapshot {
            id: "fixture".into(),
            root: root.to_path_buf(),
            documents: vec![Document {
                path: "src/lib.rs".into(),
                ranges: vec![SourceRange {
                    start: 0,
                    end: source.len(),
                }],
            }],
        }],
        initialization_options: json!({"cargo":{"features":"all","sysroot":null,"buildScripts":{"enable":false}},
            "procMacro":{"enable":false},"checkOnSave":false,"cachePriming":{"enable":false}}),
    };
    let outcome = analyze(&config, request, Default::default()).unwrap();
    assert_eq!(outcome.status, Status::Analyzed, "{:?}", outcome.gaps);
    assert!(
        outcome.evidence[0].documents[0]
            .symbols
            .iter()
            .any(|symbol| symbol.name == "value" && !symbol.references.is_empty())
    );
}
