#![cfg(unix)]

use mf_runtime::{ExecutionContext, StreamCancellation, TypedStreamNode, ValueRef};
use mfn_lsp::*;
use serde_json::json;
use std::{
    fs,
    path::Path,
    thread,
    time::{Duration, Instant},
};

fn fixture(mode: &str) -> (tempfile::TempDir, ServerConfig, AnalysisRequest) {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("source.txt"), "\u{1f600} chosen\n").unwrap();
    let config = ServerConfig {
        command: "python3".into(),
        args: vec![
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/server.py")
                .to_str()
                .unwrap()
                .into(),
            mode.into(),
            directory.path().join("requests").to_str().unwrap().into(),
        ],
        profile: Profile::Standard,
        extensions: vec![],
        deadline_ms: 2000,
        max_requests: 32,
        max_bytes: 64 * 1024,
    };
    let request = AnalysisRequest {
        snapshots: vec![Snapshot {
            id: "head".into(),
            root: directory.path().to_path_buf(),
            documents: vec![Document {
                path: "source.txt".into(),
                ranges: vec![SourceRange { start: 5, end: 11 }],
            }],
        }],
        initialization_options: json!({"fixture":true}),
    };
    (directory, config, request)
}

fn reaped(path: &Path) {
    let pid: i32 = fs::read_to_string(path.join("requests.pid"))
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
}

#[test]
fn negotiates_positions_collects_partial_results_and_closes_before_emission() {
    let (directory, config, request) = fixture("normal");
    let original = ValueRef::from(json!({"baseline":"opaque"})).into_secret();
    let mut node = AnalyzeNode::new(config).unwrap();
    let mut emissions = 0;
    node.execute(
        AnalyzeInput {
            request: ValueRef::from(serde_json::to_value(&request).unwrap()).into_secret(),
            context: original.clone(),
        },
        &mut ExecutionContext::default(),
        &mut |result| {
            emissions += 1;
            reaped(directory.path());
            assert!(result.outputs.outcome.is_secret());
            let outcome: AnalysisOutcome =
                serde_json::from_value(serde_json::to_value(&result.outputs.outcome).unwrap())
                    .unwrap();
            assert_eq!(outcome.status, Status::Analyzed);
            assert_eq!(outcome.request, request);
            assert!(result.outputs.context.ptr_eq(&original));
            assert!(result.outputs.context.is_secret());
            let symbol = &outcome.evidence[0].documents[0].symbols[0];
            assert_eq!(symbol.range, SourceRange { start: 5, end: 11 });
            assert_eq!(symbol.references.len(), 2);
            assert_eq!(symbol.incoming_calls.len(), 1);
            assert!(outcome.evidence[0].progress_notifications >= 2);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(emissions, 1);
    let requests = fs::read_to_string(directory.path().join("requests")).unwrap();
    assert!(!requests.contains("rust-analyzer/"));
    assert_eq!(
        requests
            .lines()
            .filter(|line| line.contains("\"method\": \"initialize\""))
            .count(),
        1
    );
}

#[test]
fn unsupported_capabilities_are_gaps_not_independence_claims() {
    let (directory, config, request) = fixture("limited");
    let result = analyze(&config, request, StreamCancellation::default()).unwrap();
    assert_eq!(result.status, Status::Incomplete);
    assert_eq!(result.gaps.len(), 2);
    assert!(result.gaps.iter().all(|g| g.kind == GapKind::Unsupported));
    reaped(directory.path());
}

#[test]
fn deadline_and_protocol_failure_are_incomplete_and_reap_children() {
    for mode in ["idle", "malformed", "duplicate_length"] {
        let (directory, mut config, request) = fixture(mode);
        config.deadline_ms = 500;
        let start = Instant::now();
        let outcome = analyze(&config, request, StreamCancellation::default()).unwrap();
        assert_eq!(outcome.status, Status::Incomplete, "{mode}");
        assert!(start.elapsed() < Duration::from_secs(3));
        reaped(directory.path());
    }
}

#[test]
fn cancellation_interrupts_idle_reads_without_emitting_fallback() {
    let (directory, mut config, request) = fixture("idle");
    config.deadline_ms = 10_000;
    let cancellation = StreamCancellation::default();
    let signal = cancellation.clone();
    let pid_path = directory.path().join("requests.pid");
    let canceller = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(2);
        while !pid_path.exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        signal.cancel(mf_runtime::StreamError::Execution {
            message: "cancel fixture".into(),
        });
    });
    let started = Instant::now();
    let result = analyze(&config, request, cancellation);
    canceller.join().unwrap();
    assert!(matches!(result, Err(Error::Cancelled { .. })));
    assert!(started.elapsed() < Duration::from_secs(3));
    reaped(directory.path());
}

#[test]
fn invalid_paths_and_emitter_failures_do_not_become_analysis_recovery() {
    let (_directory, config, mut request) = fixture("normal");
    request.snapshots[0].documents[0].path = "../escape".into();
    assert!(matches!(
        analyze(&config, request.clone(), StreamCancellation::default()),
        Err(Error::Invalid { .. })
    ));
    request.snapshots[0].documents.clear();
    let mut node = AnalyzeNode::new(config).unwrap();
    let mut emitted = 0;
    let result = node.execute(
        AnalyzeInput {
            request: serde_json::to_value(request).unwrap().into(),
            context: json!(null).into(),
        },
        &mut ExecutionContext::default(),
        &mut |_| {
            emitted += 1;
            mf_runtime::NodeExecutionFailedSnafu {
                message: "downstream closed",
            }
            .fail()
        },
    );
    assert!(result.is_err());
    assert_eq!(emitted, 1);
}

#[test]
fn position_conversion_rejects_split_unicode_and_handles_all_encodings() {
    let source = "\u{1f600} chosen\r\nnext";
    for (encoding, column) in [("utf-8", 5), ("utf-16", 3), ("utf-32", 2)] {
        assert_eq!(
            byte_offset(source, &json!({"line":0,"character":column}), encoding).unwrap(),
            5
        );
    }
    for (position, encoding) in [
        (json!({"line":0,"character":1}), "utf-16"),
        (json!({"line":0,"character":0}), "unknown"),
        (json!({}), "utf-16"),
        (json!({"line":5,"character":0}), "utf-16"),
        (json!({"line":0,"character":100}), "utf-16"),
    ] {
        assert!(byte_offset(source, &position, encoding).is_err());
    }
    assert_eq!(
        byte_offset(source, &json!({"line":1,"character":0}), "utf-16").unwrap(),
        13
    );
}

#[test]
fn reuses_one_session_per_snapshot_and_never_mixes_base_and_head() {
    let (directory, config, mut request) = fixture("normal");
    fs::write(directory.path().join("other.txt"), "\u{1f600} chosen\n").unwrap();
    request.snapshots[0].documents.push(Document {
        path: "other.txt".into(),
        ranges: vec![SourceRange { start: 5, end: 11 }],
    });
    let mut base = request.snapshots[0].clone();
    base.id = "base".into();
    request.snapshots.insert(0, base);
    let result = analyze(&config, request, StreamCancellation::default()).unwrap();
    assert_eq!(result.status, Status::Analyzed);
    assert_eq!(
        result
            .evidence
            .iter()
            .map(|s| s.snapshot.as_str())
            .collect::<Vec<_>>(),
        ["base", "head"]
    );
    assert!(result.evidence.iter().all(|s| s.documents.len() == 2));
    let requests = fs::read_to_string(directory.path().join("requests")).unwrap();
    assert_eq!(
        requests
            .lines()
            .filter(|line| line.contains("\"method\": \"initialize\""))
            .count(),
        2
    );
    reaped(directory.path());
}

#[test]
fn request_budget_and_missing_servers_preserve_incomplete_status() {
    let (directory, mut config, request) = fixture("normal");
    config.max_requests = 4;
    let outcome = analyze(&config, request.clone(), StreamCancellation::default()).unwrap();
    assert_eq!(outcome.status, Status::Incomplete);
    assert!(outcome.gaps.iter().any(|g| g.kind == GapKind::Limit));
    reaped(directory.path());
    config.command = "/missing-lsp-executable".into();
    let outcome = analyze(&config, request, StreamCancellation::default()).unwrap();
    assert_eq!(outcome.status, Status::Incomplete);
    assert!(
        outcome
            .gaps
            .iter()
            .any(|g| g.kind == GapKind::AnalyzerFailure)
    );
}

#[test]
fn invalid_configuration_and_inputs_preserve_typed_failures_without_starting_servers() {
    use std::error::Error as _;
    let (directory, config, request) = fixture("normal");
    let registration = inventory::iter::<mf_runtime::NodeRegistration>
        .into_iter()
        .find(|node| node.kind == KIND)
        .unwrap();
    for (field, value) in [
        ("command", json!("")),
        ("deadline_ms", json!(0)),
        ("max_requests", json!(3)),
        ("max_bytes", json!(0)),
    ] {
        let mut value_config = serde_json::to_value(&config).unwrap();
        value_config[field] = value;
        let error = registration.instantiate(value_config).err().unwrap();
        assert!(error.source().unwrap().downcast_ref::<Error>().is_some());
    }
    for case in [
        "identity",
        "relative_root",
        "duplicate",
        "reversed",
        "request_limit",
    ] {
        let mut invalid = request.clone();
        match case {
            "identity" => invalid.snapshots[0].id.clear(),
            "relative_root" => invalid.snapshots[0].root = "relative".into(),
            "duplicate" => invalid.snapshots.push(invalid.snapshots[0].clone()),
            "reversed" => {
                invalid.snapshots[0].documents[0].ranges[0] = SourceRange { start: 9, end: 1 }
            }
            _ => invalid.initialization_options = json!("x".repeat(config.max_bytes)),
        }
        assert!(
            matches!(
                analyze(&config, invalid, Default::default()),
                Err(Error::Invalid { .. })
            ),
            "{case}"
        );
    }
    let mut node = AnalyzeNode::new(config).unwrap();
    let error = node
        .execute(
            AnalyzeInput {
                request: json!(null).into(),
                context: json!(null).into(),
            },
            &mut ExecutionContext::default(),
            &mut |_| panic!("invalid input must not emit"),
        )
        .unwrap_err();
    assert!(error.source().unwrap().downcast_ref::<Error>().is_some());
    assert!(!directory.path().join("requests.pid").exists());
}

#[test]
fn document_contracts_reject_escaping_or_invalid_sources_and_close_sessions() {
    for case in ["escape", "directory", "range", "utf8", "document_limit"] {
        let (directory, mut config, mut request) = fixture("normal");
        let outside = tempfile::NamedTempFile::new().unwrap();
        let source = directory.path().join("source.txt");
        match case {
            "escape" => {
                fs::remove_file(&source).unwrap();
                std::os::unix::fs::symlink(outside.path(), &source).unwrap();
            }
            "directory" => {
                fs::remove_file(&source).unwrap();
                fs::create_dir(&source).unwrap();
            }
            "range" => request.snapshots[0].documents[0].ranges[0].end = 100,
            "utf8" => fs::write(&source, [0xff]).unwrap(),
            _ => {
                config.max_bytes = 1024;
                fs::write(&source, "x".repeat(2048)).unwrap();
            }
        }
        let result = analyze(&config, request, Default::default());
        if matches!(case, "utf8" | "document_limit") {
            let outcome = result.unwrap();
            assert_eq!(outcome.status, Status::Incomplete);
            assert_eq!(
                outcome.gaps[0].kind,
                if case == "utf8" {
                    GapKind::AnalyzerFailure
                } else {
                    GapKind::Limit
                }
            );
        } else {
            assert!(matches!(result, Err(Error::Invalid { .. })), "{case}");
        }
        reaped(directory.path());
    }
}

#[test]
fn accepts_standard_symbol_shapes_and_services_read_only_client_requests() {
    for mode in [
        "flat",
        "children",
        "null_symbols",
        "null_references",
        "client_requests",
        "no_symbols",
    ] {
        let (directory, config, mut request) = fixture(mode);
        request.initialization_options = json!({"fixture":{"enabled":true}});
        let outcome = analyze(&config, request, Default::default()).unwrap();
        if mode == "no_symbols" {
            assert_eq!(outcome.status, Status::Incomplete);
            assert_eq!(outcome.gaps[0].operation, "textDocument/documentSymbol");
            assert_eq!(outcome.gaps[0].kind, GapKind::Unsupported);
        } else {
            assert_eq!(
                outcome.status,
                Status::Analyzed,
                "{mode}: {:?}",
                outcome.gaps
            );
            let symbols = &outcome.evidence[0].documents[0].symbols;
            assert_eq!(
                symbols.len(),
                match mode {
                    "null_symbols" => 0,
                    "children" => 2,
                    _ => 1,
                }
            );
            if mode == "children" {
                assert_eq!(
                    symbols.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
                    ["chosen", "child"]
                );
            }
            if mode == "null_references" {
                assert!(symbols[0].references.is_empty());
            }
        }
        reaped(directory.path());
    }
}

#[test]
fn malformed_queries_and_exhausted_result_budgets_are_explicit_gaps() {
    for mode in [
        "eof",
        "invalid_json",
        "bad_capabilities",
        "bad_encoding",
        "unhealthy",
        "bad_symbols",
        "missing_name",
        "bad_reference",
        "bad_call_item",
        "bad_incoming",
        "bad_partial_final",
        "missing_range",
        "reversed_range",
        "rpc_error",
        "missing_result",
        "bad_array",
        "large_header",
        "unterminated_header",
        "large_message",
        "partial_limit",
        "document_limit",
        "snapshot_limit",
        "aggregate_limit",
        "write_blocked",
    ] {
        let (directory, mut config, mut request) = fixture(mode);
        if mode == "unhealthy" {
            config.profile = Profile::RustAnalyzer;
        }
        if mode == "snapshot_limit" {
            fs::copy(
                directory.path().join("source.txt"),
                directory.path().join("other.txt"),
            )
            .unwrap();
            let mut document = request.snapshots[0].documents[0].clone();
            document.path = "other.txt".into();
            request.snapshots[0].documents.push(document);
        }
        if mode == "aggregate_limit" {
            config.max_bytes = 1500;
            request.initialization_options = json!({"padding":"x".repeat(700)});
        }
        if mode == "write_blocked" {
            config.max_bytes = 1024 * 1024;
            config.deadline_ms = 500;
            request.initialization_options = json!({"padding":"x".repeat(700_000)});
        }
        let outcome = analyze(&config, request, Default::default()).unwrap();
        assert_eq!(outcome.status, Status::Incomplete, "{mode}");
        assert!(
            outcome.gaps.iter().any(|gap| gap.kind
                == if mode.starts_with("large_")
                    || mode.ends_with("_limit")
                    || matches!(mode, "unterminated_header" | "write_blocked")
                {
                    GapKind::Limit
                } else {
                    GapKind::AnalyzerFailure
                }),
            "{mode}: {:?}",
            outcome.gaps
        );
        reaped(directory.path());
    }
}
