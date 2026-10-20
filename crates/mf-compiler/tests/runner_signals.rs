#![cfg(unix)]
mod common;

use mf_compiler::{CompileRequest, RunnerOptions, SupportPackages, compile_project_with_options};
use nix::{
    sys::signal::{Signal, kill},
    unistd::Pid,
};
use serde_json::json;
use std::{
    fs,
    path::Path,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

struct Running<'a> {
    child: Child,
    directory: &'a Path,
}

impl Drop for Running<'_> {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if !self.directory.join("cleaned").exists()
            && let Ok(pid) = fs::read_to_string(self.directory.join("child"))
            && let Ok(pid) = pid.parse()
        {
            let _ = kill(Pid::from_raw(pid), Signal::SIGKILL);
        }
    }
}

fn wait_for(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(Instant::now() < deadline, "runner did not respond in time");
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn generated_runners_cancel_cleanly_and_escalate_repeated_interrupts() {
    let root = tempfile::tempdir().unwrap();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/multi-nodes");
    for streaming in [false, true] {
        let definition = root.path().join("flow.json");
        let executable = root.path().join("runner");
        let mut flow = json!({
            "version": "2026-10-03",
            "dependencies": {"fixture": {"package": "fixture-multi-nodes", "path": fixture}},
            "nodes": [{"id": "wait", "kind": "fixture.wait_for_interrupt"}],
            "outputs": [{"name": "finished", "node": "wait", "port": "finished"}]
        });
        if streaming {
            flow["execution"] = json!({"mode": "stream", "limits": {"workers": 1}});
        }
        fs::write(&definition, flow.to_string()).unwrap();
        compile_project_with_options(
            &CompileRequest {
                definition: &definition,
                output: &executable,
                locked: false,
                build_dir: Some(&root.path().join("build")),
                support: &SupportPackages::Local {
                    crates_dir: common::crates_dir(),
                },
            },
            &RunnerOptions { telemetry: false },
        )
        .unwrap();
        for selected in [false, true] {
            for signal in [Signal::SIGINT, Signal::SIGTERM] {
                for block in [false, true] {
                    let directory = tempfile::tempdir_in(root.path()).unwrap();
                    let mut command = Command::new(&executable);
                    if selected {
                        command.args(["--node", "wait"]);
                    }
                    let mut runner = Running {
                        child: command
                            .args([
                                "--inputs",
                                &json!({
                                    "wait.directory": directory.path(),
                                    "wait.block": block
                                })
                                .to_string(),
                            ])
                            .stdin(Stdio::null())
                            .stdout(Stdio::null())
                            .spawn()
                            .unwrap(),
                        directory: directory.path(),
                    };
                    wait_for(|| directory.path().join("ready").exists());
                    let pid = Pid::from_raw(runner.child.id() as i32);
                    // Signal only the runner: its isolated child must be cleaned by the node.
                    kill(pid, signal).unwrap();
                    if block {
                        thread::sleep(Duration::from_millis(50));
                        assert!(runner.child.try_wait().unwrap().is_none());
                        kill(pid, signal).unwrap();
                    }
                    let mut status = None;
                    wait_for(|| {
                        status = runner.child.try_wait().unwrap();
                        status.is_some()
                    });
                    assert_eq!(
                        status.unwrap().code(),
                        Some(128 + signal as i32),
                        "streaming={streaming}, selected={selected}, block={block}"
                    );
                    if !block {
                        assert!(directory.path().join("cleaned").exists());
                        let child = fs::read_to_string(directory.path().join("child"))
                            .unwrap()
                            .parse()
                            .unwrap();
                        assert_eq!(
                            kill(Pid::from_raw(child), None),
                            Err(nix::errno::Errno::ESRCH)
                        );
                    }
                }
            }
        }
    }
}
