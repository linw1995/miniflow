#![cfg(unix)]

use mf_compiler::{CompileRequest, RunnerOptions, SupportPackages, compile_project_with_options};
use nix::{
    pty::{Winsize, openpty},
    sys::{
        stat::Mode,
        termios::{LocalFlags, tcgetattr},
    },
    unistd::mkfifo,
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

// Frame diffs can reuse characters from earlier frames, so assertions inspect rendered cells.
struct Screen {
    cells: Vec<Vec<char>>,
    row: usize,
    column: usize,
    pending: Vec<u8>,
}

impl Screen {
    fn new() -> Self {
        Self {
            cells: vec![vec![' '; 160]; 40],
            row: 0,
            column: 0,
            pending: Vec::new(),
        }
    }

    fn feed(&mut self, bytes: &[u8]) {
        self.pending.extend_from_slice(bytes);
        let mut index = 0;
        while index < self.pending.len() {
            let byte = self.pending[index];
            if byte == 27 {
                if index + 1 == self.pending.len() {
                    break;
                }
                if self.pending[index + 1] != b'[' {
                    index += 2;
                    continue;
                }
                let Some(end) = (index + 2..self.pending.len())
                    .find(|&i| (0x40..=0x7e).contains(&self.pending[i]))
                else {
                    break;
                };
                if self.pending[end] == b'H' {
                    let parameters = String::from_utf8_lossy(&self.pending[index + 2..end]);
                    let mut coordinates = parameters
                        .split(';')
                        .map(|value| value.parse::<usize>().unwrap_or(1).max(1));
                    self.row = coordinates.next().unwrap_or(1).min(40) - 1;
                    self.column = coordinates.next().unwrap_or(1).min(160) - 1;
                }
                index = end + 1;
                continue;
            }
            match byte {
                b'\r' => self.column = 0,
                b'\n' => self.row = (self.row + 1).min(39),
                0..=31 => {}
                _ => {
                    let length = match byte {
                        0..=127 => 1,
                        128..=223 => 2,
                        224..=239 => 3,
                        _ => 4,
                    };
                    if index + length > self.pending.len() {
                        break;
                    }
                    let value = std::str::from_utf8(&self.pending[index..index + length])
                        .unwrap()
                        .chars()
                        .next()
                        .unwrap();
                    if self.column == 160 {
                        self.column = 0;
                        self.row = (self.row + 1).min(39);
                    }
                    self.cells[self.row][self.column] = value;
                    self.column += 1;
                    index += length;
                    continue;
                }
            }
            index += 1;
        }
        self.pending.drain(..index);
    }

    fn text(&self) -> String {
        self.cells
            .iter()
            .map(|row| row.iter().collect::<String>().trim_end().to_owned())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

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
        let mut terminal = Screen::new();
        let mut buffer = [0; 8192];
        let mut interrupted = false;
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
            if interrupt && !interrupted && text.contains("Observed calls: 1") {
                master.write_all(b"\x03").unwrap();
                interrupted = true;
            }
            if stage == 0 && text.contains("Exited:") {
                master
                    .write_all(if inspect { b"jv" } else { b"q" })
                    .unwrap();
                stage = if inspect { 1 } else { 3 };
            } else if stage == 1 && text.contains("Data history is unavailable") {
                master.write_all(b"v").unwrap();
                stage = 2;
            } else if stage == 2 {
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
    let graph = json!({"version":"2026-10-03", "execution":{"mode":"stream"},
        "dependencies":{"core":{"package":"mfn-core", "path":crates.join("builtin-nodes/core")}},
        "nodes":[{"id":"read", "kind":"builtin.readline"}, {"id":"consume", "kind":"builtin.identity"}],
        "edges":[{"from_node":"read", "from_output":"line", "to_node":"consume", "to_input":"input"}],
        "outputs":[{"name":"value", "node":"consume", "port":"value"}]});
    compile(graph.clone(), &source);
    fs::remove_file(&definition).unwrap();
    fs::remove_dir_all(&build).unwrap();

    for invalid in [
        "{}",
        r#"{"read":{"path":42}}"#,
        r#"{"read":{"path":"x","path":"y"}}"#,
    ] {
        let (status, output, screen) =
            run_tui(&source, &["--inputs".into(), invalid.into()], false, false);
        assert_eq!(status.code(), Some(1), "{screen}");
        assert!(output.is_empty());
        assert!(!screen.contains("Workflow: "));
    }
    let parameters = json!({"read":{"path":data}}).to_string();
    let inputs_file = root.join("parameters.json");
    fs::write(&inputs_file, &parameters).unwrap();
    let (status, output, screen) = run_tui(
        &source,
        &["--inputs-file".into(), inputs_file.clone().into_os_string()],
        false,
        true,
    );
    assert_eq!(status.code(), Some(0), "{screen}");
    assert!(
        screen.contains("Complete") && screen.contains("Observed calls: 6"),
        "{screen}"
    );
    assert!(screen.contains("Data history is unavailable"));
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

    let jsonl = root.join("data.jsonl");
    fs::write(&jsonl, "one\r\ntwo\nthree\nfour\nfive\nsix").unwrap();
    let (status, captured, screen) = run_tui(
        &source,
        &["--stream-input".into(), jsonl.clone().into_os_string()],
        false,
        true,
    );
    assert_eq!(status.code(), Some(0), "{screen}");
    assert_eq!(captured, output);
    for arguments in [
        vec![],
        vec!["--stream-input".into(), "-".into()],
        vec![
            "--stream-input".into(),
            root.join("absent").into_os_string(),
        ],
    ] {
        let (status, output, screen) = run_tui(&source, &arguments, false, false);
        assert_eq!(status.code(), Some(1), "{screen}");
        assert!(output.is_empty());
    }
    fs::write(&jsonl, b"one\n\xff\n").unwrap();
    let (status, _, screen) = run_tui(
        &source,
        &["--stream-input".into(), jsonl.into_os_string()],
        false,
        false,
    );
    assert_eq!(status.code(), Some(1), "{screen}");
    assert!(screen.contains("Workflow failure (input)"), "{screen}");

    let fifo = root.join("interrupt.jsonl");
    mkfifo(&fifo, Mode::S_IRUSR | Mode::S_IWUSR).unwrap();
    let interrupt_parameters = json!({"read":{"path":fifo}}).to_string();
    let writer_path = fifo.clone();
    let writer = thread::spawn(move || -> std::io::Result<()> {
        let mut writer = fs::OpenOptions::new().write(true).open(writer_path)?;
        for _ in 0..100 {
            if let Err(error) = writer.write_all(b"line\n") {
                if error.kind() == std::io::ErrorKind::BrokenPipe {
                    return Ok(());
                }
                return Err(error);
            }
            thread::sleep(Duration::from_millis(250));
        }
        Ok(())
    });
    let (status, _, screen) = run_tui(
        &source,
        &["--inputs".into(), interrupt_parameters.into()],
        true,
        false,
    );
    assert_eq!(status.code(), Some(130), "{screen}");
    writer.join().unwrap().unwrap();
}
