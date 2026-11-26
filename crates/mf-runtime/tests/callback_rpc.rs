#![cfg(unix)]

use mf_runtime::{
    Outputs, StreamCancellation, WorkflowArguments,
    callback::*,
    callback_rpc::{Grant, connect, serve},
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
    let handlers = [
        ("lookup".into(), handler("lookup")),
        ("other".into(), handler("other")),
    ]
    .into();
    let (server, mut peer) = UnixStream::pair().unwrap();
    let worker = thread::spawn(move || serve(server, &handlers, &Default::default()));
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
            serve(
                server,
                &[(
                    "lookup".into(),
                    HostCallback::new(|_, _| panic!("unexpected dispatch")),
                )]
                .into(),
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
            },
            &[("lookup".into(), vec!["site".into()])].into(),
            &execution,
        )
        .unwrap();
        let call = || callbacks["lookup"].call(&WorkflowArguments::default(), &Default::default());
        let reply = call();
        assert_eq!(reply.is_ok(), valid);
        drop(reply);
        assert!(execution.failure().is_some());
        let Err(error) = call() else {
            panic!("poisoned session dispatched a second call")
        };
        assert!(error.to_string().contains("callback RPC previously failed"));
        server.join().unwrap();
    }
}

#[test]
fn cancellation_interrupts_idle_rpc_and_incomplete_writes_do_not_acknowledge() {
    let (server, _peer) = UnixStream::pair().unwrap();
    let cancel = StreamCancellation::default();
    let stop = cancel.clone();
    let worker = thread::spawn(move || serve(server, &Default::default(), &cancel));
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
    let worker = thread::spawn(move || serve(server, &handlers, &Default::default()));
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
