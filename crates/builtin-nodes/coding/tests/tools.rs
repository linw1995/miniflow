#![cfg(any(target_os = "linux", target_os = "macos"))]

use mf_runtime::{
    ExecutionContext, NodeExecutionError, NodeRegistry, NodeResult, NodeValues, PreparedNode,
};
use mfn_coding::{BASH_KIND, CodingError, EDIT_KIND, READ_KIND, WRITE_KIND};
use rustix::{
    fs::{FlockOperation, flock},
    io::Errno,
    process::{Pid, test_kill_process},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    thread,
    time::{Duration, Instant},
};

fn prepare(kind: &str, config: Value) -> PreparedNode {
    NodeRegistry::from_inventory()
        .unwrap()
        .get(kind)
        .unwrap()
        .instantiate(config)
        .unwrap()
}

fn execute(
    node: &PreparedNode,
    input: Value,
    context: &mut ExecutionContext,
) -> Result<NodeResult, NodeExecutionError> {
    let inputs = input
        .as_object()
        .unwrap()
        .iter()
        .map(|(key, value)| (key.clone(), value.clone().into()))
        .collect();
    node.execution
        .as_task_node()
        .unwrap()
        .execute(inputs, context)
}

fn call(node: &PreparedNode, input: Value) -> Result<NodeValues, NodeExecutionError> {
    Ok(execute(node, input, &mut ExecutionContext::default())?.outputs)
}

fn business(
    result: Result<NodeValues, NodeExecutionError>,
) -> Box<dyn std::error::Error + Send + Sync> {
    match result.unwrap_err() {
        NodeExecutionError::BusinessFailed { source } => source,
        error => panic!("expected a recoverable tool error, got {error}"),
    }
}

#[test]
fn registers_coding_tools_by_their_public_flow_kinds() {
    let registry = NodeRegistry::from_inventory().unwrap();
    let kinds: BTreeSet<_> = registry
        .iter()
        .map(|node| node.kind)
        .filter(|kind| kind.starts_with("builtin.coding."))
        .collect();
    assert_eq!(
        kinds,
        BTreeSet::from([
            "builtin.coding.bash",
            "builtin.coding.edit",
            "builtin.coding.read",
            "builtin.coding.write",
        ])
    );
}

#[test]
fn read_pages_preserve_line_endings_and_do_not_echo_private_text_in_snapshots() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("input"), "one\r\nλ\nlast").unwrap();
    let read = prepare(READ_KIND, json!({"workspace_root": dir.path()}));
    let first = call(&read, json!({"path":"input", "limit":2})).unwrap();
    assert_eq!(first["content"].as_str(), Some("one\r\nλ\n"));
    assert!(first["content"].is_secret());
    assert_eq!(first["next_line"].as_u64(), Some(3));
    let last = execute(
        &read,
        json!({"path":"input", "start_line":3}),
        &mut ExecutionContext::default(),
    )
    .unwrap();
    assert_eq!(last.outputs["content"].as_str(), Some("last"));
    assert_eq!(last.skipped, ["next_line".into()].into());
    let empty = call(&read, json!({"path":"input", "start_line":4})).unwrap();
    assert_eq!(empty["content"].as_str(), Some(""));
    for (field, value) in [("start_line", 0), ("limit", 0), ("limit", 2001)] {
        let error = business(call(&read, json!({"path":"input", (field):value})));
        assert!(matches!(error.downcast_ref::<CodingError>(),
            Some(CodingError::InvalidInput { field: rejected, .. }) if *rejected == field));
    }
}

#[test]
fn read_bounds_bytes_without_splitting_a_line_and_rejects_non_utf8() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data");
    let long_line = "λ".repeat(25_600);
    fs::write(&path, format!("short\n{long_line}")).unwrap();
    let read = prepare(READ_KIND, json!({"workspace_root": dir.path()}));
    let first = call(&read, json!({"path":"data"})).unwrap();
    assert_eq!(first["content"].as_str(), Some("short\n"));
    assert_eq!(first["next_line"].as_u64(), Some(2));
    let last = call(&read, json!({"path":"data", "start_line":2})).unwrap();
    assert_eq!(last["content"].as_str(), Some(long_line.as_str()));

    fs::write(&path, format!("{long_line}!")).unwrap();
    assert!(matches!(
        business(call(&read, json!({"path":"data"}))).downcast_ref::<CodingError>(),
        Some(CodingError::LineTooLong { .. })
    ));
    fs::write(&path, [0xff]).unwrap();
    assert!(matches!(
        business(call(&read, json!({"path":"data"}))).downcast_ref::<CodingError>(),
        Some(CodingError::FileEncoding { .. })
    ));
}

#[test]
fn writes_are_create_only_and_edits_match_the_original_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let config = json!({"workspace_root": dir.path()});
    let write = prepare(WRITE_KIND, config.clone());
    let edit = prepare(EDIT_KIND, config);
    fs::create_dir(dir.path().join("nested")).unwrap();
    let path = dir.path().join("nested/script");
    let original = "begin α\r\nend β\n";
    let created = call(
        &write,
        json!({"path": "nested/script", "content": original}),
    )
    .unwrap();
    assert_eq!(
        created["bytes_written"].as_u64(),
        Some(original.len() as u64)
    );
    business(call(
        &write,
        json!({"path": "nested/script", "content": "clobber"}),
    ));
    assert_eq!(fs::read_to_string(&path).unwrap(), original);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o751)).unwrap();

    let result = call(
        &edit,
        json!({"path": "nested/script", "edits": [
            {"old_text":"end β", "new_text":"begin α"},
            {"old_text":"begin α", "new_text":"updated"}
        ]}),
    )
    .unwrap();
    assert_eq!(result["replacements"].as_u64(), Some(2));
    assert_eq!(fs::read_to_string(&path).unwrap(), "updated\r\nbegin α\n");
    assert_eq!(path.metadata().unwrap().permissions().mode() & 0o777, 0o751);
    assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
}

#[test]
fn missing_ambiguous_overlapping_or_oversized_edits_leave_the_file_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("file");
    let original = "abc aaa";
    fs::write(&path, original).unwrap();
    let config = json!({"workspace_root": dir.path(), "max_file_bytes": 8});
    let edit = prepare(EDIT_KIND, config.clone());
    let write = prepare(WRITE_KIND, config.clone());
    let read = prepare(READ_KIND, config);
    for edits in [
        json!([]),
        json!(vec![json!({"old_text":"abc", "new_text":"a"}); 129]),
        json!([{"old_text":"abc", "new_text":"abc"}]),
        json!([{"old_text":"abc", "new_text":"a"}, {"old_text":"missing", "new_text":"text"}]),
        json!([{"old_text":"aa", "new_text":"b"}]),
        json!([{"old_text":"ab", "new_text":"x"}, {"old_text":"bc", "new_text":"y"}]),
        json!([{"old_text":"abc", "new_text":"large replacement"}]),
        json!([{"old_text":"abc", "new_text":18}]),
    ] {
        business(call(&edit, json!({"path": "file", "edits": edits})));
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
    }
    business(call(&write, json!({"path":"new", "content":"λ".repeat(5)})));
    assert!(!dir.path().join("new").exists());
    fs::write(&path, "a".repeat(9)).unwrap();
    business(call(&read, json!({"path":"file"})));
}

#[test]
fn file_tools_reject_workspace_escapes_aliases_and_git_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().join("root");
    fs::create_dir(&workspace).unwrap();
    fs::create_dir(workspace.join(".git")).unwrap();
    fs::write(workspace.join(".git/config"), "repository config").unwrap();
    let outside = dir.path().join("outside");
    fs::write(&outside, "outside").unwrap();
    symlink(dir.path(), workspace.join("directory-link")).unwrap();
    symlink(&outside, workspace.join("file-link")).unwrap();
    fs::hard_link(&outside, workspace.join("hard-link")).unwrap();
    let read = prepare(READ_KIND, json!({"workspace_root":workspace}));
    let write = prepare(WRITE_KIND, json!({"workspace_root":workspace}));
    let edit = prepare(EDIT_KIND, json!({"workspace_root":workspace}));
    for path in [
        "../outside",
        "directory-link/outside",
        "file-link",
        "hard-link",
        ".git/config",
        "",
        outside.to_str().unwrap(),
    ] {
        business(call(&read, json!({"path": path})));
        business(call(
            &edit,
            json!({"path": path, "edits": [{"old_text":"outside", "new_text":"changed"}]}),
        ));
    }
    business(call(
        &write,
        json!({"path":"directory-link/new", "content":"clobber"}),
    ));
    business(call(
        &write,
        json!({"path":"file-link", "content":"clobber"}),
    ));
    for path in [".GiT/config", ".mfn-coding-private"] {
        assert!(matches!(
            business(call(&write, json!({"path":path, "content":"new"})))
                .downcast_ref::<CodingError>(),
            Some(CodingError::InvalidInput { field: "path", .. })
        ));
    }
    business(call(&write, json!({"path":"missing/new", "content":"new"})));
    assert!(!workspace.join("missing").exists());
    assert!(!dir.path().join("new").exists());
    assert_eq!(fs::read_to_string(outside).unwrap(), "outside");
}

#[test]
fn cooperating_writers_fail_without_waiting_for_a_busy_directory() {
    let dir = tempfile::tempdir().unwrap();
    let write = prepare(WRITE_KIND, json!({"workspace_root": dir.path()}));
    let lock = fs::File::open(dir.path()).unwrap();
    flock(&lock, FlockOperation::NonBlockingLockExclusive).unwrap();
    assert!(matches!(
        business(call(&write, json!({"path":"input", "content":"a"})))
            .downcast_ref::<CodingError>(),
        Some(CodingError::DirectoryBusy)
    ));
    assert!(!dir.path().join("input").exists());
    // A parallel shell spawn can inherit this open file description until exec.
    flock(&lock, FlockOperation::Unlock).unwrap();
    call(&write, json!({"path":"input", "content":"a"})).unwrap();
}

#[test]
fn bash_executes_supplied_shell_text_in_the_workspace_and_preserves_exit_status() {
    let dir = tempfile::tempdir().unwrap();
    let bash = prepare(BASH_KIND, json!({"workspace_root":dir.path()}));
    let result = call(&bash, json!({"command": "pwd -P; printf '%s' 'two words' | tr ' ' ':' > result && printf '%s %s' \"$(cat result)\" \"${HOME-unset}\"; printf 'test failed' >&2; exit 7"})).unwrap();
    let expected = format!(
        "{}\ntwo:words unset",
        dir.path().canonicalize().unwrap().display()
    );
    assert_eq!(result["stdout"].as_str(), Some(expected.as_str()));
    assert_eq!(result["stderr"].as_str(), Some("test failed"));
    assert!(result["stdout"].is_secret() && result["stderr"].is_secret());
    assert_eq!(result["exit_code"].as_i64(), Some(7));
    assert_eq!(result["timed_out"].as_bool(), Some(false));
    assert!(!result.contains_key("signal"));
    let unknown = call(&bash, json!({"command":"no-such-mfn-coding-test-command"})).unwrap();
    assert_eq!(unknown["exit_code"].as_i64(), Some(127));
    let signalled = execute(
        &bash,
        json!({"command":"kill -TERM $$"}),
        &mut ExecutionContext::default(),
    )
    .unwrap();
    assert_eq!(signalled.outputs["signal"], json!(15));
    assert_eq!(
        signalled.skipped,
        [
            "exit_code".into(),
            "stdout_path".into(),
            "stderr_path".into()
        ]
        .into()
    );
}

#[test]
fn bash_launches_the_chosen_shell_with_a_fresh_prefix_for_each_command() {
    let dir = tempfile::tempdir().unwrap();
    let shell = dir.path().join("fixture shell");
    fs::write(
        &shell,
        "#!/bin/sh\nprintf '%s\\n' \"$1\" >> marker\nexec /bin/sh \"$@\"\n",
    )
    .unwrap();
    fs::set_permissions(&shell, fs::Permissions::from_mode(0o700)).unwrap();
    let bash = prepare(
        BASH_KIND,
        json!({
            "workspace_root":dir.path(),
            "shell":{"path":shell, "command_prefix":"export LABEL=fixture\nprintf 'setup\\n' >> marker # prefix ends here"}
        }),
    );
    for _ in 0..2 {
        let result = call(&bash, json!({"command":"printf '%s:%s' \"$LABEL\" \"${PREVIOUS-unset}\"; export PREVIOUS=set; cd /"})).unwrap();
        assert_eq!(result["stdout"], json!("fixture:unset"));
        assert_eq!(result["exit_code"], json!(0));
    }
    assert_eq!(
        fs::read_to_string(dir.path().join("marker")).unwrap(),
        "-c\nsetup\n-c\nsetup\n"
    );
}

#[test]
fn bash_rejects_invalid_arguments_and_recipe_configuration() {
    let dir = tempfile::tempdir().unwrap();
    let registry = NodeRegistry::from_inventory().unwrap();
    let factory = registry.get(BASH_KIND).unwrap();
    for mut config in [
        json!({"commands":{"test":{"program":"/bin/sh"}}}),
        json!({"shell":{"path":"bash"}}),
        json!({"shell":{"path":"/bin/bash\0"}}),
        json!({"shell":{"command_prefix":"bad\0prefix"}}),
        json!({"timeout_ms":0}),
        json!({"max_output_bytes":0}),
    ] {
        config["workspace_root"] = json!(dir.path());
        assert!(factory.instantiate(config.clone()).is_err(), "{config}");
    }
    let bash = prepare(BASH_KIND, json!({"workspace_root":dir.path()}));
    for input in [
        json!({"command":"bad\0text"}),
        json!({"command":":", "timeout":-1}),
        json!({"command":":", "timeout":0}),
    ] {
        business(call(&bash, input));
    }
}

#[test]
fn bash_bounds_each_stream_independently() {
    let dir = tempfile::tempdir().unwrap();
    let bash = prepare(
        BASH_KIND,
        json!({"workspace_root":dir.path(), "max_output_bytes":5}),
    );
    let result = call(&bash, json!({"command":"i=0; while [ \"$i\" -lt 2000 ]; do printf '1234567890'; printf 'abcdef' >&2; i=$((i+1)); done"})).unwrap();
    assert_eq!(result["exit_code"].as_i64(), Some(0));
    assert_eq!(result["stdout"].as_str(), Some("67890"));
    assert_eq!(result["stderr"].as_str(), Some("bcdef"));
    for (field, expected) in [
        ("stdout_path", "1234567890".repeat(2000)),
        ("stderr_path", "abcdef".repeat(2000)),
    ] {
        let path = std::path::Path::new(result[field].as_str().unwrap());
        assert!(path.is_absolute() && !path.starts_with(dir.path()) && result[field].is_secret());
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(fs::read(path).unwrap(), expected.as_bytes());
        fs::remove_file(path).unwrap();
    }
    assert_eq!(result["stdout_truncated"].as_bool(), Some(true));
    assert_eq!(result["stderr_truncated"].as_bool(), Some(true));
}

#[test]
fn bash_streams_private_rolling_snapshots_without_truncating_the_terminal_result() {
    struct Viewer {
        resume: std::path::PathBuf,
        snapshots: Vec<String>,
    }
    impl mf_runtime::EmissionSink for Viewer {
        fn send(&mut self, emission: NodeResult) -> Result<(), mf_runtime::StreamError> {
            let progress = &emission.outputs["progress"];
            assert!(progress.is_secret());
            let text = progress.as_str().unwrap();
            if text.contains("opening stdout") && text.contains("opening stderr") {
                fs::write(&self.resume, "").unwrap();
            }
            self.snapshots.push(text.to_owned());
            Ok(())
        }
    }

    let dir = tempfile::tempdir().unwrap();
    let noise = "working\n".repeat(1500);
    fs::write(dir.path().join("bulk"), &noise).unwrap();
    let node = prepare(BASH_KIND, json!({"workspace_root":dir.path()}));
    let mut viewer = Viewer {
        resume: dir.path().join("resume"),
        snapshots: Vec::new(),
    };
    let command = "echo 'opening stdout'; echo 'opening stderr' >&2; while [ ! -f resume ]; do sleep 0.02; done; cat bulk; echo 'final stdout'; echo 'final stderr' >&2; exit 13";
    let result = node
        .execution
        .as_task_node()
        .unwrap()
        .execute_with_sink(
            [
                ("command".into(), command.into()),
                ("timeout".into(), json!(10.0).into()),
            ]
            .into(),
            &mut ExecutionContext::default(),
            &mut viewer,
        )
        .unwrap();
    assert_eq!(result.outputs["exit_code"].as_i64(), Some(13));
    assert_eq!(
        result.outputs["stdout"].as_str().unwrap(),
        format!("opening stdout\n{noise}final stdout\n")
    );
    assert!(!result.outputs.contains_key("stdout_path"));
    let last = viewer.snapshots.last().unwrap();
    assert!(viewer.snapshots.len() >= 2 && !last.contains("opening stdout"));
    assert!(last.contains("stdout:\n") && last.contains("final stdout\n"));
    assert!(last.contains("stderr:\nopening stderr\nfinal stderr\n"));
    assert!(viewer.snapshots.iter().all(|frame| frame.len() < 4096 + 32));
}

#[test]
fn bash_spills_on_the_line_limit_without_losing_binary_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let bash = prepare(BASH_KIND, json!({"workspace_root": dir.path()}));
    let line = [0xff, 0, b'\n'];
    for count in [2000, 2001] {
        let raw = line.repeat(count);
        fs::write(dir.path().join("bytes"), &raw).unwrap();
        let result = call(&bash, json!({"command":"cat bytes >&2"})).unwrap();
        assert_eq!(
            result["stderr"].as_str().unwrap(),
            "\u{fffd}\0\n".repeat(2000)
        );
        assert_eq!(result["stderr_truncated"].as_bool(), Some(count > 2000));
        assert!(!result.contains_key("stdout_path"));
        if count > 2000 {
            let path = result["stderr_path"].as_str().unwrap();
            assert_eq!(fs::read(path).unwrap(), raw);
            fs::remove_file(path).unwrap();
        } else {
            assert!(!result.contains_key("stderr_path"));
        }
    }
}

#[test]
fn bash_cleans_up_its_group_even_if_the_leader_exits_first() {
    let dir = tempfile::tempdir().unwrap();
    let bash = prepare(BASH_KIND, json!({"workspace_root":dir.path()}));
    let result = call(&bash, json!({"command":"( : > started; sleep 1; printf late > late ) & while [ ! -f started ]; do :; done; exit 12", "timeout":5})).unwrap();
    assert_eq!(result["exit_code"].as_i64(), Some(12));
    assert!(dir.path().join("started").is_file());
    thread::sleep(Duration::from_millis(1100));
    assert!(!dir.path().join("late").exists());
}

#[test]
fn bash_honors_the_shorter_of_the_call_timeout_and_the_workflow_limit() {
    let dir = tempfile::tempdir().unwrap();
    for (limit, timeout) in [(None, Some(0.05)), (Some(50), Some(10.0)), (Some(50), None)] {
        let bash = prepare(
            BASH_KIND,
            json!({"workspace_root":dir.path(), "timeout_ms":limit}),
        );
        let result = execute(
            &bash,
            json!({"command":"sleep 2", "timeout":timeout}),
            &mut ExecutionContext::default(),
        )
        .unwrap();
        assert_eq!(result.outputs["timed_out"].as_bool(), Some(true));
        assert_eq!(
            result.skipped,
            [
                "exit_code".into(),
                "signal".into(),
                "stdout_path".into(),
                "stderr_path".into()
            ]
            .into()
        );
    }
}

#[test]
fn bash_cancellation_is_not_a_tool_result_and_reaps_the_child() {
    let dir = tempfile::tempdir().unwrap();
    let bash = prepare(BASH_KIND, json!({"workspace_root":dir.path()}));
    let mut context = ExecutionContext::default();
    let cancellation = mf_runtime::StreamCancellation::default();
    context.set_cancellation(cancellation.clone());
    let worker = thread::spawn(move || {
        execute(
            &bash,
            json!({"command":"printf '%s\\n' \"$$\" > started; while :; do :; done", "timeout":8}),
            &mut context,
        )
    });
    let marker = dir.path().join("started");
    let deadline = Instant::now() + Duration::from_secs(5);
    let pid = loop {
        if let Ok(text) = fs::read_to_string(&marker)
            && let Ok(raw) = text.trim().parse()
        {
            break Pid::from_raw(raw).unwrap();
        }
        assert!(Instant::now() < deadline, "test command did not start");
        thread::sleep(Duration::from_millis(5));
    };
    cancellation.cancel(
        mf_runtime::StreamExecutionSnafu {
            message: "cancel test command",
        }
        .build(),
    );
    match worker.join().unwrap().unwrap_err() {
        NodeExecutionError::PluginFailed { source } => {
            assert!(matches!(
                source.downcast_ref::<mf_runtime::StreamError>(),
                Some(mf_runtime::StreamError::Execution { message }) if message == "cancel test command"
            ));
        }
        error => panic!("unexpected cancellation error: {error}"),
    }
    assert_eq!(test_kill_process(pid), Err(Errno::SRCH));
}

#[test]
fn preparation_validates_syntax_without_opening_a_workspace_or_command() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing");
    prepare(READ_KIND, json!({"workspace_root": &missing}));
    prepare(
        BASH_KIND,
        json!({"workspace_root": &missing, "shell":{"path":missing.join("bash")}}),
    );
    let registry = NodeRegistry::from_inventory().unwrap();
    let factory = registry.get(READ_KIND).unwrap();
    assert!(factory.instantiate(json!({"workspace_root": "."})).is_err());
    for invalid in [0, 16 * 1024 * 1024 + 1] {
        assert!(
            factory
                .instantiate(json!({"workspace_root": &missing, "max_file_bytes":invalid}))
                .is_err()
        );
    }
    let read = prepare(READ_KIND, json!({"workspace_root":{"$ref":"params.root"}}));
    assert_eq!(
        read.metadata.context_references,
        [mf_runtime::ContextReference::new(
            "params.root",
            "workspace_root"
        )]
    );
    assert!(
        !read
            .metadata
            .ports
            .inputs
            .iter()
            .any(|port| port.name == "workspace_root")
    );
}
