#![cfg(unix)]

use mf_telemetry::{
    description::{NodeDescription, WorkflowDescription, WorkflowDescriptionVersion},
    identity::WorkflowId,
};
use nix::{
    pty::{Winsize, openpty},
    sys::termios::{LocalFlags, tcgetattr},
};
use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    os::unix::fs::PermissionsExt,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

#[test]
fn tui_preserves_stdout_and_restores_terminal_after_missing_telemetry() {
    if !loopback_available() {
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let runner = directory.path().join("fake-runner");
    let json = description_json();
    fs::write(&runner, format!(
        "#!/bin/sh\nif [ \"$1\" = --describe ]; then\n  printf '%s\\n' '{json}'\n  exit 0\nfi\n[ -z \"${{OTEL_EXPORTER_OTLP_HEADERS+x}}\" ] || exit 7\ncase \"$OTEL_EXPORTER_OTLP_ENDPOINT\" in http://127.0.0.1:*) ;; *) exit 8 ;; esac\n[ -n \"$MF_RUN_ID\" ] || exit 9\nprintf '\\001\\000\\377'\ndd if=/dev/zero bs=65536 count=2 2>/dev/null\nprintf 'diagnostic\\n' >&2\n"
    )).unwrap();
    fs::set_permissions(&runner, fs::Permissions::from_mode(0o700)).unwrap();

    let pty = openpty(
        Some(&Winsize {
            ws_row: 30,
            ws_col: 100,
            ws_xpixel: 0,
            ws_ypixel: 0,
        }),
        None,
    )
    .unwrap();
    let original = tcgetattr(&pty.slave).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_mf"))
        .arg("run")
        .arg(&runner)
        .arg("--tui")
        .stdin(Stdio::from(pty.slave.try_clone().unwrap()))
        .stderr(Stdio::from(pty.slave.try_clone().unwrap()))
        .stdout(Stdio::piped())
        .env("OTEL_EXPORTER_OTLP_ENDPOINT", "https://remote.invalid")
        .env("OTEL_EXPORTER_OTLP_HEADERS", "Authorization=secret")
        .spawn()
        .unwrap();
    let mut captured_stdout = child.stdout.take().unwrap();
    let stdout_reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        captured_stdout.read_to_end(&mut bytes).unwrap();
        bytes
    });
    let mut master = fs::File::from(pty.master);
    let reader = thread::spawn(move || {
        let mut output = Vec::new();
        let mut bytes = [0; 4096];
        let mut sent = false;
        loop {
            match master.read(&mut bytes) {
                Ok(0) | Err(_) => break,
                Ok(len) => {
                    output.extend_from_slice(&bytes[..len]);
                    if !sent
                        && output
                            .windows(b"Exited:".len())
                            .any(|part| part == b"Exited:")
                    {
                        master.write_all(b"q").unwrap();
                        sent = true;
                    }
                }
            }
        }
        output
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            panic!("TUI did not exit after its completed view received q");
        }
        thread::sleep(Duration::from_millis(20));
    };
    let stdout = stdout_reader.join().unwrap();
    let restored = tcgetattr(&pty.slave).unwrap();
    assert_eq!(
        restored.local_flags & (LocalFlags::ICANON | LocalFlags::ECHO),
        original.local_flags & (LocalFlags::ICANON | LocalFlags::ECHO)
    );
    drop(pty.slave);
    let screen = reader.join().unwrap();
    let screen = String::from_utf8_lossy(&screen);
    assert!(screen.contains("Unverif") && screen.contains("edTail"));
    assert!(screen.contains("Unknown"));
    assert_eq!(&stdout[..3], [1, 0, 255]);
    assert_eq!(stdout.len(), 3 + 131_072);
    assert!(stdout[3..].iter().all(|byte| *byte == 0));
    assert_eq!(status.code(), Some(0));
}

#[test]
fn tui_rejects_missing_terminal_before_launching_runner() {
    let output = Command::new(env!("CARGO_BIN_EXE_mf"))
        .arg("run")
        .arg("/no-such-runner")
        .arg("--tui")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("requires terminal stdin and stderr"));
}

#[test]
fn tui_restores_terminal_when_execution_spawn_fails_after_preflight() {
    if !loopback_available() {
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let runner = directory.path().join("vanishing-runner");
    let json = description_json();
    fs::write(&runner, format!(
        "#!/bin/sh\nif [ \"$1\" = --describe ]; then\n  printf '%s\\n' '{json}'\n  rm \"$0\"\n  exit 0\nfi\nexit 99\n"
    )).unwrap();
    fs::set_permissions(&runner, fs::Permissions::from_mode(0o700)).unwrap();

    let pty = openpty(
        Some(&Winsize {
            ws_row: 30,
            ws_col: 100,
            ws_xpixel: 0,
            ws_ypixel: 0,
        }),
        None,
    )
    .unwrap();
    let original = tcgetattr(&pty.slave).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_mf"))
        .arg("run")
        .arg(&runner)
        .arg("--tui")
        .stdin(Stdio::from(pty.slave.try_clone().unwrap()))
        .stderr(Stdio::from(pty.slave.try_clone().unwrap()))
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let mut master = fs::File::from(pty.master);
    let reader = thread::spawn(move || {
        let mut output = Vec::new();
        let mut bytes = [0; 4096];
        loop {
            match master.read(&mut bytes) {
                Ok(0) | Err(_) => break,
                Ok(len) => output.extend_from_slice(&bytes[..len]),
            }
        }
        output
    });
    let deadline = Instant::now() + Duration::from_secs(8);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            panic!("TUI did not exit after execution spawn failed");
        }
        thread::sleep(Duration::from_millis(20));
    };
    let restored = tcgetattr(&pty.slave).unwrap();
    assert_eq!(
        restored.local_flags & (LocalFlags::ICANON | LocalFlags::ECHO),
        original.local_flags & (LocalFlags::ICANON | LocalFlags::ECHO)
    );
    drop(pty.slave);
    let screen = reader.join().unwrap();
    assert!(String::from_utf8_lossy(&screen).contains("could not launch workflow"));
    assert_eq!(status.code(), Some(1));
}

#[test]
fn tui_escalates_ignored_interrupt_and_restores_terminal() {
    if !loopback_available() {
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let runner = directory.path().join("stubborn-runner");
    let json = description_json();
    fs::write(&runner, format!(
        "#!/bin/sh\nif [ \"$1\" = --describe ]; then\n  printf '%s\\n' '{json}'\n  exit 0\nfi\ntrap '' INT\nfor i in 1 2 3 4 5; do sleep 1; done\nexit 99\n"
    )).unwrap();
    fs::set_permissions(&runner, fs::Permissions::from_mode(0o700)).unwrap();

    let pty = openpty(
        Some(&Winsize {
            ws_row: 30,
            ws_col: 100,
            ws_xpixel: 0,
            ws_ypixel: 0,
        }),
        None,
    )
    .unwrap();
    let original = tcgetattr(&pty.slave).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_mf"))
        .arg("run")
        .arg(&runner)
        .arg("--tui")
        .stdin(Stdio::from(pty.slave.try_clone().unwrap()))
        .stderr(Stdio::from(pty.slave.try_clone().unwrap()))
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let mut master = fs::File::from(pty.master);
    let reader = thread::spawn(move || {
        let mut output = Vec::new();
        let mut bytes = [0; 4096];
        let mut sent = false;
        let mut closed = false;
        loop {
            match master.read(&mut bytes) {
                Ok(0) | Err(_) => break,
                Ok(len) => {
                    output.extend_from_slice(&bytes[..len]);
                    if !sent
                        && output
                            .windows(b"Running".len())
                            .any(|part| part == b"Running")
                    {
                        master.write_all(b"q\x03").unwrap();
                        sent = true;
                    }
                    if sent
                        && !closed
                        && output
                            .windows(b"Exited:".len())
                            .any(|part| part == b"Exited:")
                    {
                        master.write_all(b"q").unwrap();
                        closed = true;
                    }
                }
            }
        }
        output
    });
    let deadline = Instant::now() + Duration::from_secs(8);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            drop(pty.slave);
            let screen = reader.join().unwrap();
            panic!(
                "TUI did not enforce the interrupt deadline: {}",
                String::from_utf8_lossy(&screen)
            );
        }
        thread::sleep(Duration::from_millis(20));
    };
    let restored = tcgetattr(&pty.slave).unwrap();
    assert_eq!(
        restored.local_flags & (LocalFlags::ICANON | LocalFlags::ECHO),
        original.local_flags & (LocalFlags::ICANON | LocalFlags::ECHO)
    );
    drop(pty.slave);
    let screen = reader.join().unwrap();
    assert!(String::from_utf8_lossy(&screen).contains("Running"));
    assert_eq!(status.code(), Some(130));
}

#[test]
fn tui_marks_stdout_incomplete_when_descendant_keeps_pipe_open() {
    if !loopback_available() {
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let runner = directory.path().join("pipe-holding-runner");
    let json = description_json();
    fs::write(&runner, format!(
        "#!/bin/sh\nif [ \"$1\" = --describe ]; then\n  printf '%s\\n' '{json}'\n  exit 0\nfi\nsleep 5 &\nprintf 'prefix'\n"
    )).unwrap();
    fs::set_permissions(&runner, fs::Permissions::from_mode(0o700)).unwrap();

    let pty = openpty(
        Some(&Winsize {
            ws_row: 30,
            ws_col: 100,
            ws_xpixel: 0,
            ws_ypixel: 0,
        }),
        None,
    )
    .unwrap();
    let original = tcgetattr(&pty.slave).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_mf"))
        .arg("run")
        .arg(&runner)
        .arg("--tui")
        .stdin(Stdio::from(pty.slave.try_clone().unwrap()))
        .stderr(Stdio::from(pty.slave.try_clone().unwrap()))
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut master = fs::File::from(pty.master);
    let reader = thread::spawn(move || {
        let mut output = Vec::new();
        let mut bytes = [0; 4096];
        let mut sent = false;
        loop {
            match master.read(&mut bytes) {
                Ok(0) | Err(_) => break,
                Ok(len) => {
                    output.extend_from_slice(&bytes[..len]);
                    if !sent
                        && output
                            .windows(b"Exited:".len())
                            .any(|part| part == b"Exited:")
                    {
                        master.write_all(b"q").unwrap();
                        sent = true;
                    }
                }
            }
        }
        output
    });
    let deadline = Instant::now() + Duration::from_secs(8);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            panic!("TUI waited indefinitely for a descendant-held pipe");
        }
        thread::sleep(Duration::from_millis(20));
    };
    let stdout = {
        let mut bytes = Vec::new();
        child
            .stdout
            .take()
            .unwrap()
            .read_to_end(&mut bytes)
            .unwrap();
        bytes
    };
    let restored = tcgetattr(&pty.slave).unwrap();
    assert_eq!(
        restored.local_flags & (LocalFlags::ICANON | LocalFlags::ECHO),
        original.local_flags & (LocalFlags::ICANON | LocalFlags::ECHO)
    );
    drop(pty.slave);
    let screen = reader.join().unwrap();
    assert!(
        String::from_utf8_lossy(&screen).contains("incomplete"),
        "{}",
        String::from_utf8_lossy(&screen)
    );
    assert_eq!(stdout, b"prefix");
    assert_eq!(status.code(), Some(1));
}

#[test]
fn failed_stdout_delivery_preserves_a_nonzero_child_result() {
    if !loopback_available() {
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let runner = directory.path().join("failing-runner");
    let json = description_json();
    fs::write(&runner, format!(
        "#!/bin/sh\nif [ \"$1\" = --describe ]; then\n  printf '%s\\n' '{json}'\n  exit 0\nfi\nprintf 'result'\nexit 23\n"
    )).unwrap();
    fs::set_permissions(&runner, fs::Permissions::from_mode(0o700)).unwrap();

    let pty = openpty(
        Some(&Winsize {
            ws_row: 30,
            ws_col: 100,
            ws_xpixel: 0,
            ws_ypixel: 0,
        }),
        None,
    )
    .unwrap();
    let original = tcgetattr(&pty.slave).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_mf"))
        .arg("run")
        .arg(&runner)
        .arg("--tui")
        .stdin(Stdio::from(pty.slave.try_clone().unwrap()))
        .stderr(Stdio::from(pty.slave.try_clone().unwrap()))
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdout.take());
    let mut master = fs::File::from(pty.master);
    let reader = thread::spawn(move || {
        let mut output = Vec::new();
        let mut bytes = [0; 4096];
        let mut sent = false;
        loop {
            match master.read(&mut bytes) {
                Ok(0) | Err(_) => break,
                Ok(len) => {
                    output.extend_from_slice(&bytes[..len]);
                    if !sent
                        && output
                            .windows(b"Exited:".len())
                            .any(|part| part == b"Exited:")
                    {
                        master.write_all(b"q").unwrap();
                        sent = true;
                    }
                }
            }
        }
        output
    });
    let deadline = Instant::now() + Duration::from_secs(8);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            panic!("TUI did not close after stdout delivery failed");
        }
        thread::sleep(Duration::from_millis(20));
    };
    let restored = tcgetattr(&pty.slave).unwrap();
    assert_eq!(
        restored.local_flags & (LocalFlags::ICANON | LocalFlags::ECHO),
        original.local_flags & (LocalFlags::ICANON | LocalFlags::ECHO)
    );
    drop(pty.slave);
    let screen = reader.join().unwrap();
    assert!(
        String::from_utf8_lossy(&screen).contains("could not deliver captured workflow stdout")
    );
    assert_eq!(status.code(), Some(23));
}

fn description_json() -> String {
    let description = WorkflowDescription {
        version: WorkflowDescriptionVersion::CURRENT,
        workflow_id: WorkflowId::try_from(format!("sha256:{}", "a".repeat(64))).unwrap(),
        nodes: vec![NodeDescription {
            id: "step".into(),
            kind: "test.step".into(),
        }],
        data_edges: vec![],
        control_edges: vec![],
        execution_order: vec!["step".into()],
    };
    String::from_utf8(description.to_json().unwrap()).unwrap()
}

fn loopback_available() -> bool {
    match TcpListener::bind("127.0.0.1:0") {
        Ok(listener) => {
            drop(listener);
            true
        }
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => false,
        Err(error) => panic!("could not probe loopback availability: {error}"),
    }
}
