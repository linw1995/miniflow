#![cfg(unix)]
extern crate mfn_test_affected as _;

use mf_runtime::{
    ExecutionContext, Inputs, NodeExecutionError, NodeRegistry, NodeResult, ValueRef,
};
use serde_json::{Value, json};
use std::{
    fs, thread,
    time::{Duration, Instant},
};

fn run(
    config: Value,
    inputs: Inputs,
    context: &mut ExecutionContext,
) -> Result<NodeResult, NodeExecutionError> {
    NodeRegistry::from_inventory()
        .unwrap()
        .get("process.run")
        .unwrap()
        .instantiate(config)
        .unwrap()
        .execution
        .as_task_node()
        .unwrap()
        .execute(inputs, context)
}
fn python(code: &str) -> Value {
    json!({"program":"python3","args":["-c",code]})
}

#[test]
fn exchanges_json_without_shell_interpolation_or_losing_sensitivity() {
    let directory = tempfile::Builder::new()
        .prefix("process fixture ")
        .tempdir()
        .unwrap();
    let literal = "; touch unexpected";
    let result = run(json!({"program":"python3","args":["-c",
        "import json, os, sys; print('fixture diagnostic', file=sys.stderr); print(json.dumps({'input':json.load(sys.stdin),'argument':sys.argv[1],'cwd':os.getcwd()}))",literal],"output":"json"}),
        Inputs::from([
            ("cwd".into(), ValueRef::from(directory.path().to_str().unwrap())),
            ("stdin_json".into(), ValueRef::from(json!({"secret":"fixture"})).into_secret()),
        ]), &mut ExecutionContext::default()).unwrap();
    assert!(result.outputs["result"].is_secret());
    assert_eq!(
        result.outputs["result"]["input"],
        json!({"secret":"fixture"})
    );
    assert_eq!(result.outputs["result"]["argument"], json!(literal));
    assert_eq!(
        result.outputs["result"]["cwd"],
        json!(directory.path().canonicalize().unwrap())
    );
    assert!(!directory.path().join("unexpected").exists());
}

#[test]
fn failed_invalid_and_excessive_process_results_never_succeed() {
    for (code, extra) in [
        ("print('{}'); raise SystemExit(7)", json!({"output":"json"})),
        ("print('not-json')", json!({"output":"json"})),
        ("print('{} {}')", json!({"output":"json"})),
        ("import os; os.write(1,b'\\xff')", json!({})),
        ("print('x'*4096)", json!({"max_stdout_bytes":8})),
        ("import time; time.sleep(60)", json!({"timeout_ms":20})),
    ] {
        let mut config = python(code);
        config
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        assert!(
            run(config, Inputs::new(), &mut ExecutionContext::default()).is_err(),
            "{code}"
        );
    }
    let directory = tempfile::tempdir().unwrap();
    let mut config = python("import pathlib; pathlib.Path('started').touch()");
    let input = Inputs::from([
        ("cwd".into(), directory.path().to_str().unwrap().into()),
        ("stdin_json".into(), ValueRef::from("x".repeat(1024 * 1024))),
    ]);
    assert!(run(config.clone(), input, &mut ExecutionContext::default()).is_err());
    let mut context = ExecutionContext::default();
    context
        .cancellation()
        .cancel(mf_runtime::StreamError::Execution {
            message: "cancelled before start".into(),
        });
    assert!(
        run(
            config.clone(),
            Inputs::from([("cwd".into(), directory.path().to_str().unwrap().into())]),
            &mut context
        )
        .is_err()
    );
    assert!(!directory.path().join("started").exists());
    config["max_stdout_bytes"] = json!(0);
    assert!(
        NodeRegistry::from_inventory()
            .unwrap()
            .get("process.run")
            .unwrap()
            .instantiate(config)
            .is_err()
    );
}

#[test]
fn cancellation_and_normal_completion_clean_same_group_descendants() {
    for cancel in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        fs::write(root.join("child.py"), r#"import json, pathlib, signal, subprocess, sys, time
ready = pathlib.Path('ready')
if sys.argv[1] == 'true': signal.signal(signal.SIGTERM, signal.SIG_IGN)
child = subprocess.Popen([sys.executable, '-c', "import pathlib, signal, time; signal.signal(signal.SIGTERM, signal.SIG_IGN); pathlib.Path('ready').touch(); time.sleep(60)"])
while not ready.exists(): time.sleep(0.01)
pathlib.Path('pid').write_text(str(child.pid))
if sys.argv[1] == 'true': time.sleep(60)
print('done')
"#).unwrap();
        let mut context = ExecutionContext::default();
        let signal = context.cancellation();
        let pidfile = root.join("pid");
        let watched = pidfile.clone();
        let watcher = thread::spawn(move || {
            if cancel {
                let deadline = Instant::now() + Duration::from_secs(5);
                while !watched.exists() && Instant::now() < deadline {
                    thread::sleep(Duration::from_millis(10));
                }
                signal.cancel(mf_runtime::StreamError::Execution {
                    message: "cancel process fixture".into(),
                });
            }
        });
        let result = run(
            json!({"program":"python3","args":["child.py",cancel.to_string()],"timeout_ms":10000}),
            Inputs::from([("cwd".into(), root.to_str().unwrap().into())]),
            &mut context,
        );
        watcher.join().unwrap();
        if cancel {
            assert!(result.is_err());
        } else {
            assert_eq!(result.unwrap().outputs["result"], json!("done\n"));
        }
        let pid: i32 = fs::read_to_string(pidfile).unwrap().parse().unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while unsafe { libc::kill(pid, 0) } == 0 && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            unsafe { libc::kill(pid, 0) },
            -1,
            "descendant {pid} survived"
        );
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
    }
}

#[test]
fn native_finite_and_selected_tasks_forward_interrupts_to_owned_processes() {
    use mf_compiler::{
        CompileRequest, RunnerOptions, SupportPackages, compile_project_with_options,
    };
    use std::{
        os::unix::process::CommandExt,
        path::Path,
        process::{Command, Stdio},
    };
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let package = Path::new(env!("CARGO_MANIFEST_DIR"));
    let flow = root.join("flow.json");
    fs::write(&flow, json!({"version":"2026-10-19","dependencies":{"process":{"package":"mfn-test-affected","path":package}},
        "nodes":[{"id":"run","kind":"process.run","config":{"program":"python3","args":["-c",
        "import os,pathlib,time; pathlib.Path('pid.tmp').write_text(str(os.getpid())); pathlib.Path('pid.tmp').replace('pid'); time.sleep(60)"]}}],
        "outputs":[{"name":"result","node":"run","port":"result"}]}).to_string()).unwrap();
    let executable = root.join("runner");
    compile_project_with_options(
        &CompileRequest {
            definition: &flow,
            output: &executable,
            locked: false,
            build_dir: Some(&root.join("build")),
            support: &SupportPackages::Local {
                crates_dir: package.join("../../../../crates").canonicalize().unwrap(),
            },
        },
        &RunnerOptions { telemetry: false },
    )
    .unwrap();
    for selected in [false, true] {
        let pidfile = root.join("pid");
        let _ = fs::remove_file(&pidfile);
        let mut command = Command::new(&executable);
        if selected {
            command.args(["--node", "run"]);
        }
        let mut child = command
            .arg("--run.cwd")
            .arg(root)
            .process_group(0)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !pidfile.exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        unsafe {
            libc::kill(
                -(child.id() as i32),
                if selected {
                    libc::SIGTERM
                } else {
                    libc::SIGINT
                },
            );
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut status = None;
        while Instant::now() < deadline {
            status = child.try_wait().unwrap();
            if status.is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        if status.is_none() {
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
        }
        let completed = child.wait().unwrap();
        let pid: i32 = fs::read_to_string(pidfile).unwrap().parse().unwrap();
        let survived = unsafe { libc::kill(pid, 0) } == 0;
        if survived {
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
        assert!(status.is_some() && !completed.success());
        assert!(!survived, "owned process survived native interruption");
    }
}
