#![cfg(unix)]

use mf_runtime::{
    Outputs, StreamCancellation, WorkflowArguments,
    callback::*,
    callback_rpc::{Grant, HostSession, IoGrant, connect, serve_session},
};
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Write},
    net::Shutdown,
    os::unix::net::{UnixListener, UnixStream},
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

fn read(stream: &UnixStream) -> Value {
    let mut bytes = Vec::new();
    BufReader::new(stream)
        .read_until(b'\n', &mut bytes)
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[test]
fn rpc_has_only_requests_and_responses_and_delivers_once_after_writing() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let handler = |name: &'static str| {
        let events = events.clone();
        HostCallback::new(move |args, _| {
            assert!(args.0["key"].is_secret());
            let events = events.clone();
            Ok(
                CallbackReply::from(Outputs::from([("type".into(), args.0["key"].clone())]))
                    .on_delivery(move |ok| events.lock().unwrap().push((name, ok))),
            )
        })
    };
    let session = HostSession {
        callbacks: [
            ("lookup".into(), handler("lookup")),
            ("other".into(), handler("other")),
        ]
        .into(),
        ..Default::default()
    };
    let (server, mut peer) = UnixStream::pair().unwrap();
    let worker = thread::spawn(move || serve_session(server, session, &Default::default()));
    for (id, name) in [(1, "lookup"), (2, "other")] {
        writeln!(
            peer,
            "{}",
            json!({"id":id,"binding":name,"args":{"values":{"key":"call"},"private":[["key"]]}})
        )
        .unwrap();
        assert_eq!(
            read(&peer),
            json!({"id":id,"value":{"values":{"type":"call"},"private":[["type"]]}})
        );
    }
    peer.shutdown(Shutdown::Write).unwrap();
    worker.join().unwrap().unwrap();
    assert_eq!(*events.lock().unwrap(), [("lookup", true), ("other", true)]);
}

#[test]
fn hosted_events_and_pulled_inputs_do_not_consume_callback_ids() {
    use mf_runtime::{DiagnosticLevel, NodeValues};
    let (server, mut peer) = UnixStream::pair().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let host = HostSession {
        callbacks: [(
            "lookup".into(),
            HostCallback::new(|args, _| Ok(args.0.clone().into())),
        )]
        .into(),
        inputs: Some(WorkflowArguments(
            [(
                "params.token".into(),
                mf_runtime::ValueRef::from("token").into_secret(),
            )]
            .into(),
        )),
        output: Some(Box::new(move |record: NodeValues, _| {
            tx.send(record).unwrap();
            Ok(())
        })),
        diagnostics: Some(Box::new(|diagnostic, _| {
            assert_eq!(diagnostic.level, DiagnosticLevel::Warning);
            assert_eq!(diagnostic.message, "note");
            Ok(())
        })),
    };
    let worker = thread::spawn(move || serve_session(server, host, &Default::default()));
    writeln!(peer, "{}", json!({"get_inputs":{}})).unwrap();
    assert_eq!(
        read(&peer),
        json!({"inputs":{"values":{"params.token":"token"},"private":[["params.token"]]}})
    );
    let large = "z".repeat(MAX_CALLBACK_FRAME + 1);
    for record in [json!({"answer":"prefix"}), json!({"output":large})] {
        writeln!(peer, "{}", json!({"output":record})).unwrap();
        assert_eq!(
            serde_json::to_value(rx.recv_timeout(Duration::from_secs(5)).unwrap()).unwrap(),
            record
        );
    }
    writeln!(
        peer,
        "{}",
        json!({"diagnostic":{"level":"warning","message":"note"}})
    )
    .unwrap();
    writeln!(
        peer,
        "{}",
        json!({"id":1,"binding":"lookup","args":{"values":{"x":1}}})
    )
    .unwrap();
    assert_eq!(read(&peer), json!({"id":1,"value":{"values":{"x":1}}}));
    writeln!(peer, "{}", json!({"flush":{}})).unwrap();
    assert_eq!(read(&peer), json!({"flushed":true}));
    worker.join().unwrap().unwrap();
}

#[test]
fn preauthenticated_sessions_also_reject_oversized_startup_inputs() {
    let (server, peer) = UnixStream::pair().unwrap();
    drop(peer);
    let session = HostSession {
        inputs: Some(WorkflowArguments(
            [(
                "key".into(),
                "x".repeat(mf_runtime::MAX_WORKFLOW_INPUT_BYTES).into(),
            )]
            .into(),
        )),
        ..Default::default()
    };
    assert!(matches!(
        serve_session(server, session, &Default::default()),
        Err(CallbackError::Inputs { .. })
    ));
}

#[test]
fn ungranted_or_incomplete_io_fails_without_replaying_any_event() {
    fn output() -> HostSession {
        HostSession {
            output: Some(Box::new(|_, _| Ok(()))),
            ..Default::default()
        }
    }
    fn inputs() -> HostSession {
        HostSession {
            inputs: Some(Default::default()),
            ..Default::default()
        }
    }
    for (host, lines, diagnostic) in [
        (
            HostSession::default(),
            vec![json!({"get_inputs":{}})],
            "not granted",
        ),
        (
            HostSession::default(),
            vec![json!({"output":{}})],
            "output not granted",
        ),
        (
            HostSession::default(),
            vec![json!({"flush":{}})],
            "unexpected drain",
        ),
        (
            output(),
            vec![json!({"diagnostic":{"level":"error","message":"not allowed"}})],
            "diagnostics not granted",
        ),
        (
            inputs(),
            vec![json!({"get_inputs":{}}), json!({"get_inputs":{}})],
            "already consumed",
        ),
        (inputs(), vec![json!({"output":{}})], "requested first"),
        (
            output(),
            vec![json!({"output":{"x":1}})],
            "closed before completing",
        ),
        (
            output(),
            vec![
                json!({"id":1,"binding":"x","args":{"values":{"x":"z".repeat(MAX_CALLBACK_FRAME)}}}),
            ],
            "exceeds limit",
        ),
        (
            output(),
            vec![json!({"output":{},"binding":"x"})],
            "encoding",
        ),
    ] {
        let (server, mut peer) = UnixStream::pair().unwrap();
        let worker = thread::spawn(move || serve_session(server, host, &Default::default()));
        for line in lines {
            writeln!(peer, "{line}").unwrap();
        }
        peer.shutdown(Shutdown::Write).unwrap();
        let error = worker.join().unwrap().unwrap_err();
        assert!(error.to_string().contains(diagnostic), "{error}");
    }
}

#[test]
fn event_sinks_must_drain_before_exit_and_cannot_fall_back_after_disconnect() {
    for (reply, valid) in [
        ("", false),
        ("{\"flushed\":false}\n", false),
        ("{\"flushed\":true}\n", true),
    ] {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let path = dir.path().join("rpc");
        let listener = UnixListener::bind(&path).unwrap();
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(socket.try_clone().unwrap());
            for expected in [json!({"output":{"id":"business"}}), json!({"flush":{}})] {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                assert_eq!(serde_json::from_str::<Value>(&line).unwrap(), expected);
            }
            socket.write_all(reply.as_bytes()).unwrap();
        });
        let execution = StreamCancellation::default();
        let mut client = connect(
            Grant {
                parent: std::process::id(),
                socket: path,
                bindings: vec![],
                io: IoGrant {
                    output: true,
                    ..Default::default()
                },
            },
            &Default::default(),
            &execution,
        )
        .unwrap();
        assert!(client.take_inputs().unwrap().is_none());
        assert!(
            client
                .diagnostic(mf_runtime::RunnerDiagnostic {
                    level: mf_runtime::DiagnosticLevel::Error,
                    message: "unexpected".into()
                })
                .is_err()
        );
        client
            .output([("id".into(), "business".into())].into(), &execution)
            .unwrap();
        assert_eq!(client.finish().is_ok(), valid);
        assert_eq!(execution.failure().is_none(), valid);
        server.join().unwrap();
    }
}

#[test]
fn a_rejected_output_never_acknowledges_drain() {
    let (server, mut peer) = UnixStream::pair().unwrap();
    let worker = thread::spawn(move || {
        serve_session(
            server,
            HostSession {
                output: Some(Box::new(|_, _| {
                    ProtocolSnafu {
                        reason: "consumer stopped",
                    }
                    .fail()
                })),
                ..Default::default()
            },
            &Default::default(),
        )
    });
    peer.write_all(b"{\"output\":{}}\n{\"flush\":{}}\n")
        .unwrap();
    assert!(
        worker
            .join()
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("consumer stopped")
    );
    let mut bytes = Vec::new();
    assert_eq!(
        BufReader::new(peer).read_until(b'\n', &mut bytes).unwrap(),
        0
    );
}

#[test]
fn malformed_partials_unknown_bindings_and_reused_ids_do_not_dispatch() {
    for (input, expected) in [
        (String::from("{\"type\":\"init\"}\n"), "encoding"),
        (String::from("{\"id\":1,\"id\":1}\n"), "duplicate"),
        (String::from("{"), "incomplete RPC"),
        ("x".repeat(MAX_CALLBACK_FRAME), "exceeds limit"),
        (
            format!(
                "{}\n",
                json!({"id":2,"binding":"lookup","args":{"values":{}}})
            ),
            "out-of-order",
        ),
        (
            format!(
                "{}\n",
                json!({"id":1,"binding":"missing","args":{"values":{}}})
            ),
            "unbound callback",
        ),
    ] {
        let (server, mut peer) = UnixStream::pair().unwrap();
        let worker = thread::spawn(move || {
            serve_session(
                server,
                HostSession {
                    callbacks: [(
                        "lookup".into(),
                        HostCallback::new(|_, _| panic!("unexpected dispatch")),
                    )]
                    .into(),
                    ..Default::default()
                },
                &Default::default(),
            )
        });
        peer.write_all(input.as_bytes()).unwrap();
        peer.shutdown(Shutdown::Write).unwrap();
        let error = worker.join().unwrap().unwrap_err();
        assert!(error.to_string().contains(expected), "{error}");
    }
}

#[test]
fn oversized_replies_write_no_prefix_and_do_not_acknowledge_delivery() {
    let (server, mut peer) = UnixStream::pair().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let handler = HostCallback::new(move |_, _| {
        let tx = tx.clone();
        Ok(CallbackReply::from(Outputs::from([(
            "value".into(),
            "x".repeat(MAX_CALLBACK_FRAME).into(),
        )]))
        .on_delivery(move |ok| tx.send(ok).unwrap()))
    });
    let session = HostSession {
        callbacks: [("lookup".into(), handler)].into(),
        ..Default::default()
    };
    let worker = thread::spawn(move || serve_session(server, session, &Default::default()));
    writeln!(
        peer,
        "{}",
        json!({"id":1,"binding":"lookup","args":{"values":{}}})
    )
    .unwrap();
    assert!(
        matches!(worker.join().unwrap(), Err(CallbackError::Codec { source }) if source.to_string().contains("exceeds limit"))
    );
    assert!(!rx.recv().unwrap());
    assert_eq!(
        BufReader::new(peer)
            .read_until(b'\n', &mut Vec::new())
            .unwrap(),
        0
    );
}

#[test]
fn a_failed_or_abandoned_client_reply_permanently_poisons_the_grant() {
    for (response, valid) in [
        ("", false),
        ("{", false),
        ("bad\n", false),
        ("{\"id\":2,\"value\":{\"values\":{}}}\n", false),
        ("{\"id\":1,\"value\":{\"values\":{}}}\n", true),
    ] {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let path = dir.path().join("rpc");
        let listener = UnixListener::bind(&path).unwrap();
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            assert_eq!(
                read(&socket),
                json!({"id":1,"binding":"lookup","args":{"values":{}}})
            );
            socket.write_all(response.as_bytes()).unwrap();
        });
        let execution = StreamCancellation::default();
        let callbacks = connect(
            Grant {
                parent: std::process::id(),
                socket: path,
                bindings: vec!["lookup".into()],
                io: Default::default(),
            },
            &[("lookup".into(), vec!["site".into()])].into(),
            &execution,
        )
        .unwrap()
        .callbacks();
        let call = || callbacks["lookup"].call(&WorkflowArguments::default(), &Default::default());
        let reply = call();
        assert_eq!(reply.is_ok(), valid);
        drop(reply);
        assert!(execution.failure().is_some());
        let Err(error) = call() else {
            panic!("poisoned session dispatched a second call")
        };
        assert!(error.to_string().contains("runner RPC previously failed"));
        server.join().unwrap();
    }
}

#[test]
fn cancellation_interrupts_idle_rpc_and_incomplete_writes_do_not_acknowledge() {
    let (server, _peer) = UnixStream::pair().unwrap();
    let cancel = StreamCancellation::default();
    let stop = cancel.clone();
    let worker = thread::spawn(move || serve_session(server, HostSession::default(), &cancel));
    thread::sleep(Duration::from_millis(60));
    stop.cancel(
        mf_runtime::StreamExecutionSnafu {
            message: "test cancellation",
        }
        .build(),
    );
    assert!(matches!(
        worker.join().unwrap(),
        Err(CallbackError::Stream { .. })
    ));
    let (server, mut peer) = UnixStream::pair().unwrap();
    let (started, wait) = std::sync::mpsc::channel();
    let (sent, received) = std::sync::mpsc::channel();
    let handlers = [(
        "lookup".into(),
        HostCallback::new(move |_, _| {
            started.send(()).unwrap();
            let sent = sent.clone();
            Ok(CallbackReply::from(Outputs::from([(
                "large".into(),
                "x".repeat(1024 * 1024).into(),
            )]))
            .on_delivery(move |ok| sent.send(ok).unwrap()))
        }),
    )]
    .into();
    let worker = thread::spawn(move || {
        serve_session(
            server,
            HostSession {
                callbacks: handlers,
                ..Default::default()
            },
            &Default::default(),
        )
    });
    writeln!(
        peer,
        "{}",
        json!({"id":1,"binding":"lookup","args":{"values":{}}})
    )
    .unwrap();
    wait.recv().unwrap();
    drop(peer);
    assert!(worker.join().unwrap().is_err());
    assert!(!received.recv().unwrap());
}
