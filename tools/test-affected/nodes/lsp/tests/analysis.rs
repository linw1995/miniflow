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
fn no_requests_never_start_a_server_and_still_emit_once() {
    let (directory, mut config, mut request) = fixture("normal");
    config.command = "/missing-server".into();
    request.snapshots[0].documents.clear();
    let outcome = analyze(&config, request, StreamCancellation::default()).unwrap();
    assert_eq!(outcome.status, Status::NotRequested);
    assert!(!directory.path().join("requests.pid").exists());
}

#[test]
fn deadline_and_protocol_failure_are_incomplete_and_reap_children() {
    for mode in ["idle", "malformed"] {
        let (directory, mut config, request) = fixture(mode);
        config.deadline_ms = 500;
        let start = Instant::now();
        let outcome = analyze(&config, request, StreamCancellation::default()).unwrap();
        assert_eq!(outcome.status, Status::Incomplete);
        assert!(start.elapsed() < Duration::from_secs(3));
        reaped(directory.path());
    }
}

#[test]
fn cancellation_interrupts_idle_reads_without_emitting_fallback() {
    let (directory, config, request) = fixture("idle");
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
    let result = analyze(&config, request, cancellation);
    canceller.join().unwrap();
    assert!(matches!(result, Err(Error::Cancelled { .. })));
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
    assert!(byte_offset(source, &json!({"line":0,"character":1}), "utf-16").is_err());
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
