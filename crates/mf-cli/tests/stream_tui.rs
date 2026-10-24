#![cfg(unix)]

use mf_compiler::{CompileRequest, RunnerOptions, SupportPackages, compile_project_with_options};
use nix::{
    pty::{Winsize, openpty},
    sys::termios::{LocalFlags, tcgetattr},
};
use serde_json::{Value, json};
use std::{
    ffi::OsString,
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::Path,
    process::{Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};

#[path = "support/terminal.rs"]
mod terminal;
use terminal::Screen;

fn run_tui(
    runner: &Path,
    arguments: &[OsString],
    interrupt: bool,
    inspect: bool,
) -> (ExitStatus, Vec<u8>, String) {
    let pty = openpty(
        Some(&Winsize {
            ws_row: 40,
            ws_col: 160,
            ws_xpixel: 0,
            ws_ypixel: 0,
        }),
        None,
    )
    .unwrap();
    let original = tcgetattr(&pty.slave).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_mf"))
        .args(["run", "--tui"])
        .arg(runner)
        .args(arguments)
        .stdin(Stdio::from(pty.slave.try_clone().unwrap()))
        .stderr(Stdio::from(pty.slave.try_clone().unwrap()))
        .stdout(Stdio::piped())
        .env("MF_CAPTURE_SNAPSHOTS", "1")
        .spawn()
        .unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let output = thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).unwrap();
        bytes
    });
    let mut master = fs::File::from(pty.master);
    let display = thread::spawn(move || {
        let mut screen = String::new();
        let mut terminal = Screen::new(160, 40);
        let mut buffer = [0; 8192];
        let mut interrupted = false;
        let mut details_requested = false;
        let mut stage = 0;
        loop {
            let size = match master.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(size) => size,
            };
            terminal.feed(&buffer[..size]);
            let text = terminal.text();
            screen.push_str(&text);
            screen.push('\n');
            if !details_requested && text.contains("Workflow:") {
                master.write_all(b"d").unwrap();
                details_requested = true;
                continue;
            }
            if interrupt
                && !interrupted
                && text.contains("Observed calls:")
                && !text.contains("Observed calls: 0")
                && !text.contains("Exited:")
            {
                master.write_all(b"\x03").unwrap();
                interrupted = true;
            }
            if stage == 0 && text.contains("Exited:") && text.contains("Details") {
                master
                    .write_all(if inspect { b"jv" } else { b"q" })
                    .unwrap();
                stage = if inspect { 1 } else { 3 };
            } else if stage == 1 && text.contains("OTEL Tracing") {
                master.write_all(b"\t").unwrap();
                stage = 2;
            } else if stage == 2 && text.contains("Invocation Data") {
                master.write_all(b"l\x1b[Hf\tg").unwrap();
                stage = 4;
            } else if stage == 4 && text.contains("Session LogRecords") {
                master.write_all(b"\t").unwrap();
                stage = 6;
            } else if stage == 6 && text.contains("Session Data") {
                master.write_all(b"\x1b[H\x1b[6~f").unwrap();
                stage = 7;
            } else if stage == 7 && text.contains("Following tail") {
                master.write_all(b"v").unwrap();
                stage = 5;
            } else if stage == 5 {
                master.write_all(b"q").unwrap();
                stage = 3;
            }
        }
        screen
    });
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut timed_out = false;
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            timed_out = true;
            child.kill().unwrap();
            break child.wait().unwrap();
        }
        thread::sleep(Duration::from_millis(20));
    };
    let restored = tcgetattr(&pty.slave).unwrap();
    drop(pty.slave);
    let screen = display.join().unwrap();
    assert!(!timed_out, "TUI timed out: {screen}");
    assert_eq!(
        restored.local_flags & (LocalFlags::ICANON | LocalFlags::ECHO),
        original.local_flags & (LocalFlags::ICANON | LocalFlags::ECHO),
        "{screen}"
    );
    (status, output.join().unwrap(), screen)
}

#[test]
fn generated_streams_run_in_tui_with_parameters_files_and_interrupts() {
    if TcpListener::bind(("127.0.0.1", 0)).is_err() {
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let source = root.join("source");
    let definition = root.join("workflow.json");
    let build = root.join("build");
    let data = root.join("lines.txt");
    fs::write(&data, "one\ntwo\nthree\nfour\nfive\nsix").unwrap();
    let support = SupportPackages::Local {
        crates_dir: crates.into(),
    };
    let compile = |value: Value, output: &Path| {
        fs::write(&definition, value.to_string()).unwrap();
        compile_project_with_options(
            &CompileRequest {
                definition: &definition,
                output,
                locked: false,
                build_dir: Some(&build),
                support: &support,
            },
            &RunnerOptions::default(),
        )
        .unwrap();
    };
    let graph = json!({"version":"2026-10-03", "execution":{},
        "dependencies":{"core":{"package":"mfn-core", "path":crates.join("builtin-nodes/core")}},
        "nodes":[{"id":"read", "kind":"builtin.readline"}, {"id":"consume", "kind":"builtin.identity"}],
        "edges":[{"from_node":"read", "from_output":"line", "to_node":"consume", "to_input":"input"}],
        "outputs":[{"name":"value", "node":"consume", "port":"value"}]});
    compile(graph.clone(), &source);
    fs::remove_file(&definition).unwrap();
    fs::remove_dir_all(&build).unwrap();

    for invalid in [
        "{}",
        r#"{"read.path":42}"#,
        r#"{"read.path":"x","read.path":"y"}"#,
    ] {
        let (status, output, screen) =
            run_tui(&source, &["--inputs".into(), invalid.into()], false, false);
        assert_eq!(status.code(), Some(1), "{screen}");
        assert!(output.is_empty());
        assert!(!screen.contains("Workflow: "));
    }
    let parameters = json!({"read.path":data}).to_string();
    let inputs_file = root.join("parameters.json");
    fs::write(&inputs_file, &parameters).unwrap();
    let (status, output, screen) = run_tui(
        &source,
        &[
            "-vvv".into(),
            "--inputs-file".into(),
            inputs_file.clone().into_os_string(),
        ],
        false,
        true,
    );
    assert_eq!(status.code(), Some(0), "{screen}");
    assert!(
        screen.contains("Complete") && screen.contains("Observed calls: 6"),
        "{screen}"
    );
    assert!(screen.contains("OTEL Tracing"));
    assert!(screen.contains("Invocation Data"));
    assert!(screen.contains("Session Data"));
    assert!(screen.contains("Data capture: Complete"));
    assert!(screen.contains("six"));
    assert!(screen.contains("Session LogRecords"));
    assert!(screen.contains("mf.workflow"), "{screen}");
    assert!(!screen.contains("No trace spans were received"), "{screen}");
    let expected: Vec<_> = ["one", "two", "three", "four", "five", "six"]
        .into_iter()
        .map(|value| json!({"value":value}))
        .collect();
    assert_eq!(
        output
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_slice::<Value>(line).unwrap())
            .collect::<Vec<_>>(),
        expected
    );

    fs::write(&data, b"one\n\xff\n").unwrap();
    let (status, _, screen) = run_tui(
        &source,
        &["--read.path".into(), data.clone().into_os_string()],
        false,
        false,
    );
    assert_eq!(status.code(), Some(1), "{screen}");
    assert!(screen.contains("Workflow failure (input)"), "{screen}");

    // Keep a regular-file stream active until the TUI sends its interrupt.
    fs::write(&data, "line\n".repeat(1_000_000)).unwrap();
    let (status, _, screen) = run_tui(
        &source,
        &["--inputs".into(), parameters.into()],
        true,
        false,
    );
    assert_eq!(status.code(), Some(130), "{screen}");
}
