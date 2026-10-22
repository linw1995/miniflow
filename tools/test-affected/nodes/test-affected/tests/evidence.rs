extern crate mfn_test_affected as _;

mod common;

use mf_runtime::{ExecutionContext, Inputs, NodeExecutionError, NodeRegistry, NodeResult};
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Cursor, Write},
    net::TcpListener,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

struct Server {
    base: String,
    stop: Arc<AtomicBool>,
    entered: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}
impl Server {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = format!("http://{}/", listener.local_addr().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let entered = Arc::new(AtomicBool::new(false));
        let done = stop.clone();
        let active = entered.clone();
        let handle = thread::spawn(move || {
            while !done.load(Ordering::SeqCst) {
                let Ok((mut stream, _)) = listener.accept() else {
                    thread::sleep(Duration::from_millis(2));
                    continue;
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(1)))
                    .unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() || line.is_empty() {
                    continue;
                }
                let path = line.split_whitespace().nth(1).unwrap().to_owned();
                loop {
                    let mut header = String::new();
                    if reader.read_line(&mut header).is_err()
                        || header == "\r\n"
                        || header.is_empty()
                    {
                        break;
                    }
                }
                active.store(true, Ordering::SeqCst);
                if path == "/idle" {
                    while !done.load(Ordering::SeqCst) {
                        thread::sleep(Duration::from_millis(2));
                    }
                    break;
                }
                let (status, body) = match path.as_str() {
                    "/missing" => (404, b"missing".to_vec()),
                    "/invalid" => (200, b"not-json".to_vec()),
                    "/wide" => (200, json!(vec![0; 5000]).to_string().into_bytes()),
                    "/deep" => {
                        let value = (0..20).fold(json!(0), |inner, _| json!({"nested":inner}));
                        (200, value.to_string().into_bytes())
                    }
                    "/large" => (200, vec![b'x'; 4096]),
                    "/streamed" => {
                        let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n");
                        let _ = stream.write_all(&vec![b'x'; 4096]);
                        continue;
                    }
                    "/artifact" | "/large-member" => {
                        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
                        zip.start_file("validation.json", zip::write::SimpleFileOptions::default())
                            .unwrap();
                        let record = if path == "/large-member" {
                            json!({"padding":"x".repeat(4096)})
                        } else {
                            json!({"run_id":42,"head":"fixture"})
                        };
                        zip.write_all(record.to_string().as_bytes()).unwrap();
                        (200, zip.finish().unwrap().into_inner())
                    }
                    "/download" | "/download-incomplete" | "/download-large" => {
                        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
                        zip.start_file("graph.sqlite", zip::write::SimpleFileOptions::default())
                            .unwrap();
                        let sqlite = if path == "/download-large" {
                            vec![b'x'; 4096]
                        } else {
                            b"sqlite-fixture".to_vec()
                        };
                        zip.write_all(&sqlite).unwrap();
                        if path != "/download-incomplete" {
                            zip.start_file(
                                "graph.metadata.json",
                                zip::write::SimpleFileOptions::default(),
                            )
                            .unwrap();
                            zip.write_all(
                                json!({"commit_sha":"abc","schema_version":1})
                                    .to_string()
                                    .as_bytes(),
                            )
                            .unwrap();
                        }
                        (200, zip.finish().unwrap().into_inner())
                    }
                    _ => {
                        assert!(path.starts_with("/runs?branch=topic%2Fa%26b"), "{path}");
                        (
                            200,
                            json!({"runs":[{"id":42,"head":{"sha":"fixture"},"unused":"omit"}]})
                                .to_string()
                                .into_bytes(),
                        )
                    }
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} Response\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(&body);
            }
        });
        Self {
            base,
            stop,
            entered,
            thread: Some(handle),
        }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let joined = self.thread.take().unwrap().join();
        if !thread::panicking() {
            joined.unwrap();
        }
    }
}

fn read(
    server: &Server,
    config: Value,
    input: Value,
    context: &mut ExecutionContext,
) -> Result<NodeResult, NodeExecutionError> {
    let mut config = config;
    config["api_base"] = json!(server.base);
    invoke("github.read", config, input, context)
}

fn invoke(
    kind: &str,
    config: Value,
    input: Value,
    context: &mut ExecutionContext,
) -> Result<NodeResult, NodeExecutionError> {
    let prepared = NodeRegistry::from_inventory()
        .unwrap()
        .get(kind)
        .unwrap()
        .instantiate(config)
        .unwrap();
    let inputs: Inputs = input
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), v.clone().into()))
        .collect();
    prepared
        .execution
        .as_task_node()
        .unwrap()
        .execute(inputs, context)
}

#[test]
fn reads_bounded_projected_evidence_and_distinguishes_history_gaps() {
    let server = Server::new();
    let result = read(
        &server,
        json!({"pointer":"/runs","fields":{"id":"/id","head":"/head/sha"}}),
        json!({"path":"/runs","query":{"branch":"topic/a&b"},"token":"fixture-token"}),
        &mut ExecutionContext::default(),
    )
    .unwrap();
    assert_eq!(
        result.outputs["result"]["value"]["data"],
        json!([{"id":42,"head":"fixture"}])
    );
    assert!(
        !serde_json::to_string(&result.outputs)
            .unwrap()
            .contains("fixture-token")
    );
    let record = read(
        &server,
        json!({"member":"validation.json"}),
        json!({"path":"/artifact","token":null}),
        &mut ExecutionContext::default(),
    )
    .unwrap();
    assert_eq!(
        record.outputs["result"]["value"]["data"]["run_id"],
        json!(42)
    );
    assert_eq!(
        record.outputs["result"]["value"]["members"],
        json!(["validation.json"])
    );
    for (path, config, reason) in [
        ("/missing", json!({}), "response"),
        ("/invalid", json!({}), "invalid_data"),
        ("/deep", json!({}), "limit"),
        ("/wide", json!({}), "limit"),
        ("/large", json!({"max_bytes":1024}), "limit"),
        ("/streamed", json!({"max_bytes":1024}), "limit"),
        ("/wide", json!({"max_value_bytes":1024}), "limit"),
        (
            "/large-member",
            json!({"member":"validation.json","max_value_bytes":1024}),
            "limit",
        ),
        (
            "/artifact",
            json!({"member":"missing.json"}),
            "invalid_data",
        ),
    ] {
        let mut config = config;
        config["required"] = json!(false);
        let result = read(
            &server,
            config.clone(),
            json!({"path":path}),
            &mut ExecutionContext::default(),
        )
        .unwrap();
        assert_eq!(result.outputs["result"]["available"], json!(false));
        assert_eq!(result.outputs["result"]["reason"], json!(reason), "{path}");
        config["required"] = json!(true);
        assert!(
            read(
                &server,
                config,
                json!({"path":path}),
                &mut ExecutionContext::default()
            )
            .is_err()
        );
    }
    assert!(
        read(
            &server,
            json!({"required":false}),
            json!({"path":"//other-host/secret"}),
            &mut ExecutionContext::default()
        )
        .is_err()
    );
    for query in [json!([]), json!({"branch":["topic"]})] {
        assert!(
            read(
                &server,
                json!({"required":false}),
                json!({"path":"/runs","query":query}),
                &mut ExecutionContext::default()
            )
            .is_err()
        );
    }
}

#[test]
fn invalid_evidence_configuration_is_not_a_history_gap() {
    let registry = NodeRegistry::from_inventory().unwrap();
    for (kind, config) in [
        (
            "github.read",
            json!({"api_base":"https://user:secret@example.com/"}),
        ),
        ("github.read", json!({"pointer":"runs"})),
        ("github.read", json!({"max_bytes":0})),
        ("github.read", json!({"timeout_ms":0})),
        ("git.inspect", json!({"max_revisions":101})),
        ("git.fetch", json!({"remote":"--upload-pack=other"})),
    ] {
        assert!(
            registry.get(kind).unwrap().instantiate(config).is_err(),
            "{kind}"
        );
    }
    let (repository, head) = common::fixture();
    for (kind, config, input) in [
        (
            "git.fetch",
            json!({}),
            json!({"repository":repository.path(),"commit":"HEAD"}),
        ),
        (
            "git.inspect",
            json!({"required":false,"max_revisions":1}),
            json!({"repository":repository.path(),"revisions":[head,head]}),
        ),
    ] {
        assert!(invoke(kind, config, input, &mut ExecutionContext::default()).is_err());
    }
}

#[test]
fn downloads_artifact_members_to_disk_with_bounded_metadata() {
    let server = Server::new();
    let directory = tempfile::tempdir().unwrap();
    let target = || directory.path().join("graph").to_str().unwrap().to_owned();
    let config = || json!({"api_base":server.base,"required":false,"members":["graph.sqlite","graph.metadata.json"],"metadata_member":"graph.metadata.json"});
    let result = invoke(
        "github.download",
        config(),
        json!({"path":"/download","directory":target(),"token":"fixture-token"}),
        &mut ExecutionContext::default(),
    )
    .unwrap();
    assert_eq!(result.outputs["result"]["available"], json!(true));
    assert_eq!(
        result.outputs["result"]["value"]["members"],
        json!(["graph.sqlite", "graph.metadata.json"])
    );
    assert_eq!(
        result.outputs["result"]["value"]["metadata"]["commit_sha"],
        json!("abc")
    );
    assert_eq!(
        std::fs::read(directory.path().join("graph/graph.sqlite")).unwrap(),
        b"sqlite-fixture"
    );
    assert!(
        !serde_json::to_string(&result.outputs)
            .unwrap()
            .contains("fixture-token")
    );

    // Missing members and oversized members are degradations, not failures.
    for (path, config, reason) in [
        ("/download-incomplete", config(), "invalid_data"),
        (
            "/download-large",
            json!({"api_base":server.base,"required":false,"members":["graph.sqlite"],"max_member_bytes":1024}),
            "limit",
        ),
        ("/missing", config(), "response"),
    ] {
        let result = invoke(
            "github.download",
            config,
            json!({"path":path,"directory":target()}),
            &mut ExecutionContext::default(),
        )
        .unwrap();
        assert_eq!(
            result.outputs["result"]["available"],
            json!(false),
            "{path}"
        );
        assert_eq!(result.outputs["result"]["reason"], json!(reason), "{path}");
    }

    // Invalid paths and directories are configuration errors, not degradations.
    for input in [
        json!({"path":"//other-host/secret","directory":target()}),
        json!({"path":"/download","directory":"relative/target"}),
    ] {
        assert!(
            invoke(
                "github.download",
                config(),
                input,
                &mut ExecutionContext::default()
            )
            .is_err()
        );
    }

    // Member names must stay plain archive file names.
    let registry = NodeRegistry::from_inventory().unwrap();
    for members in [
        json!([]),
        json!(["../escape"]),
        json!(["nested/graph.sqlite"]),
    ] {
        assert!(
            registry
                .get("github.download")
                .unwrap()
                .instantiate(json!({"members":members}))
                .is_err(),
            "{members}"
        );
    }
    assert!(
        registry
            .get("github.download")
            .unwrap()
            .instantiate(json!({"members":["graph.sqlite"],"metadata_member":"other.json"}))
            .is_err()
    );
}

#[test]
fn cancellation_is_fatal_and_deadlines_are_preserved_across_reads() {
    let server = Server::new();
    let mut context = ExecutionContext::default();
    let signal = context.cancellation();
    let entered = server.entered.clone();
    let cancel = thread::spawn(move || {
        let end = Instant::now() + Duration::from_secs(2);
        while !entered.load(Ordering::SeqCst) && Instant::now() < end {
            thread::sleep(Duration::from_millis(2));
        }
        signal.cancel(mf_runtime::StreamError::Execution {
            message: "cancel evidence".into(),
        });
    });
    let started = Instant::now();
    assert!(
        read(
            &server,
            json!({"required":false}),
            json!({"path":"/idle"}),
            &mut context
        )
        .is_err()
    );
    cancel.join().unwrap();
    assert!(started.elapsed() < Duration::from_secs(2));
    let expired = read(
        &server,
        json!({"required":false}),
        json!({"path":"/missing","deadline":1}),
        &mut ExecutionContext::default(),
    )
    .unwrap();
    assert_eq!(expired.outputs["deadline"], json!(1));
    assert_eq!(expired.outputs["result"]["remaining_ms"], json!(0));
    assert_eq!(expired.outputs["result"]["reason"], json!("limit"));
    let timeout = read(
        &server,
        json!({"required":false,"timeout_ms":20}),
        json!({"path":"/idle"}),
        &mut ExecutionContext::default(),
    )
    .unwrap();
    assert_eq!(timeout.outputs["result"]["reason"], json!("limit"));
}

#[test]
fn git_inspection_and_explicit_fetch_preserve_caller_state() {
    use common::*;
    use mfn_test_affected::git;
    use std::fs;
    let (source, base) = fixture();
    let checkout = tempfile::tempdir().unwrap();
    let root = checkout.path();
    git::command(
        root,
        "git",
        &[
            "clone",
            "--quiet",
            "--no-local",
            source.path().to_str().unwrap(),
            ".",
        ],
    )
    .unwrap();
    write(source.path(), "core/src/lib.rs", "pub fn newer() {}\n");
    let head = commit(source.path());
    write(root, "core/src/lib.rs", "pub fn local() {}\n");
    git::command(root, "git", &["add", "core/src/lib.rs"]).unwrap();
    write(root, "notes.txt", "untracked\n");
    let index = fs::read(root.join(".git/index")).unwrap();
    let refs = git::command(root, "git", &["show-ref"]).unwrap();
    let fetch_head = fs::read(root.join(".git/FETCH_HEAD")).ok();
    let before = invoke(
        "git.inspect",
        json!({}),
        json!({"repository":root,"revisions":[base,head],"ancestor_of":base}),
        &mut ExecutionContext::default(),
    )
    .unwrap();
    let value = &before.outputs["result"]["value"];
    assert_eq!(value["current"]["commit"], json!(base));
    assert_eq!(value["current"]["clean"], json!(false));
    assert_eq!(value["revisions"][0]["distance"], json!(0));
    assert_eq!(value["revisions"][1]["available"], json!(false));
    let fetched = invoke(
        "git.fetch",
        json!({}),
        json!({"repository":root,"commit":head}),
        &mut ExecutionContext::default(),
    )
    .unwrap();
    assert_eq!(fetched.outputs["result"]["available"], json!(true));
    let after = invoke(
        "git.inspect",
        json!({}),
        json!({"repository":root,"revisions":[base,head],"ancestor_of":head}),
        &mut ExecutionContext::default(),
    )
    .unwrap();
    let value = &after.outputs["result"]["value"];
    assert_eq!(value["current"]["commit"], json!(base));
    assert_eq!(value["revisions"][0]["distance"], json!(1));
    assert_eq!(value["revisions"][1]["parents"], json!([base]));
    let missing = invoke(
        "git.fetch",
        json!({}),
        json!({"repository":root,"commit":"0".repeat(40)}),
        &mut ExecutionContext::default(),
    )
    .unwrap();
    assert_eq!(missing.outputs["result"]["available"], json!(false));
    assert_eq!(fs::read(root.join(".git/index")).unwrap(), index);
    assert_eq!(git::command(root, "git", &["show-ref"]).unwrap(), refs);
    assert_eq!(fs::read(root.join(".git/FETCH_HEAD")).ok(), fetch_head);
    assert_eq!(
        fs::read_to_string(root.join("notes.txt")).unwrap(),
        "untracked\n"
    );
    assert_eq!(
        fs::read_to_string(root.join("core/src/lib.rs")).unwrap(),
        "pub fn local() {}\n"
    );
}

#[test]
#[cfg(unix)]
fn blocked_git_fetch_is_bounded_and_reaps_its_process_group() {
    use common::*;
    use mfn_test_affected::git;
    use std::{fs, os::unix::fs::PermissionsExt};
    for cancel in [false, true] {
        let (source, _) = fixture();
        let checkout = tempfile::tempdir().unwrap();
        let root = checkout.path();
        git::command(
            root,
            "git",
            &[
                "clone",
                "--quiet",
                "--no-local",
                source.path().to_str().unwrap(),
                ".",
            ],
        )
        .unwrap();
        let script = root.join("target/upload-pack");
        let pid_file = root.join("target/upload.pid");
        write(
            root,
            "target/upload-pack",
            &format!(
                "#!/bin/sh\nprintf '%s' \"$$\" > '{}'\nexec sleep 60\n",
                pid_file.display()
            ),
        );
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        git::command(
            root,
            "git",
            &[
                "config",
                "remote.origin.uploadpack",
                script.to_str().unwrap(),
            ],
        )
        .unwrap();
        let mut context = ExecutionContext::default();
        let signal = context.cancellation();
        let marker = pid_file.clone();
        let canceller = cancel.then(|| {
            thread::spawn(move || {
                let end = Instant::now() + Duration::from_secs(2);
                while !marker.exists() && Instant::now() < end {
                    thread::sleep(Duration::from_millis(2));
                }
                signal.cancel(mf_runtime::StreamError::Execution {
                    message: "cancel Git evidence".into(),
                });
            })
        });
        let start = Instant::now();
        let result = invoke(
            "git.fetch",
            json!({"timeout_ms":if cancel {5000}else{500}}),
            json!({"repository":root,"commit":"0".repeat(40)}),
            &mut context,
        );
        if let Some(canceller) = canceller {
            canceller.join().unwrap();
        }
        assert!(start.elapsed() < Duration::from_secs(3));
        if cancel {
            assert!(result.is_err());
        } else {
            assert_eq!(result.unwrap().outputs["result"]["reason"], json!("limit"));
        }
        let pid: i32 = fs::read_to_string(pid_file).unwrap().parse().unwrap();
        let end = Instant::now() + Duration::from_secs(2);
        while unsafe { libc::kill(pid, 0) } == 0 && Instant::now() < end {
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
    }
}
