#![cfg(any(target_os = "linux", target_os = "macos"))]

use mf_runtime::{
    ExecutionContext, NodeExecutionError, NodeRegistry, NodeResult, NodeValues, PreparedNode,
};
use mfn_coding::{
    BASH_KIND, CodingError, EDIT_KIND, FIND_KIND, GREP_KIND, LS_KIND, MKDIR_KIND, READ_KIND,
    WRITE_KIND,
};
use rustix::{
    fs::{FlockOperation, flock},
    io::Errno,
    process::{Pid, test_kill_process},
};
use serde_json::{Value, json};
use std::{
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
    for kind in [
        "builtin.coding.mkdir",
        "builtin.coding.ls",
        "builtin.coding.find",
        "builtin.coding.grep",
        "builtin.coding.read",
        "builtin.coding.edit",
        "builtin.coding.write",
        "builtin.coding.bash",
    ] {
        assert!(registry.get(kind).is_some(), "missing node kind {kind}");
    }
    assert!(registry.get("builtin.coding.run").is_none());
}

#[test]
fn ls_lists_one_level_including_dotfiles_without_following_or_exposing_aliases() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("src/sub")).unwrap();
    fs::write(root.join(".gitignore"), "ignored\n").unwrap();
    fs::write(root.join("src/sub/.gitignore"), [0xff]).unwrap();
    for name in [".hidden", "a", "ignored"] {
        fs::write(root.join("src").join(name), "ok").unwrap();
    }
    symlink(root, root.join("src/link")).unwrap();
    let node = prepare(LS_KIND, json!({"workspace_root":root}));
    let list = call(&node, json!({"path":"src"})).unwrap();
    assert!(list["entries"].is_secret());
    assert_eq!(
        list["entries"],
        json!([{"path":"src/.hidden","kind":"file"}, {"path":"src/a","kind":"file"}, {"path":"src/sub","kind":"directory"}])
    );
    assert_eq!(list["truncated"], json!(false));
    assert_eq!(
        call(&node, json!({"limit": 1})).unwrap()["truncated"],
        json!(true)
    );
    for path in ["src/.hidden", "src/link", "src/../", "src/sub"] {
        business(call(&node, json!({"path":path})));
    }
}

#[test]
fn find_matches_relative_globs_basenames_kind_and_depth_without_reading_files() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("src/sub/mod.rs")).unwrap();
    fs::write(root.join(".gitignore"), "ignored.rs\n").unwrap();
    for name in ["a.rs", "src/b.rs", "src/ignored.rs", "src/sub/c.rs"] {
        fs::write(root.join(name), [0xff]).unwrap();
    }
    symlink(root.join("src"), root.join("alias")).unwrap();
    let node = prepare(FIND_KIND, json!({"workspace_root":root}));
    let search = |input| call(&node, input).unwrap();
    let result = search(json!({"pattern":"*.rs", "kind":"file"}));
    assert!(result["entries"].is_secret());
    assert_eq!(
        result["entries"],
        json!([{"path":"a.rs","kind":"file"}, {"path":"src/b.rs","kind":"file"}, {"path":"src/sub/c.rs","kind":"file"}])
    );
    let partial = search(json!({"path":"src", "pattern":"sub/*.rs", "limit":1}));
    assert_eq!(
        partial["entries"],
        json!([{"path":"src/sub/c.rs","kind":"file"}])
    );
    assert_eq!(partial["truncated"], json!(true));
    let shallow = search(json!({"path":"src", "pattern":"**/*.rs", "max_depth":1}));
    assert_eq!(
        shallow["entries"],
        json!([{"path":"src/b.rs","kind":"file"}])
    );
    assert_eq!(shallow["truncated"], json!(false));
    assert_eq!(
        search(json!({"pattern":"src/**/*.rs", "kind":"directory"}))["entries"],
        json!([{"path":"src/sub/mod.rs","kind":"directory"}])
    );
    for input in [
        json!({"pattern":"["}),
        json!({"pattern":"/a"}),
        json!({"pattern":"*","kind":"link"}),
        json!({"pattern":"*","max_depth":0}),
        json!({"pattern":"*","max_depth":65}),
        json!({"pattern":"*","path":"a.rs"}),
        json!({"pattern":"*","path":"alias"}),
    ] {
        business(call(&node, input));
    }
}

#[test]
fn grep_observes_nested_ignores_and_returns_private_one_based_lines() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("src/build")).unwrap();
    fs::write(root.join(".gitignore"), "*.log\nbuild/\n/root-only\n").unwrap();
    fs::write(root.join("src/.gitignore"), "!kept.log\n").unwrap();
    for path in [
        "root-only",
        "src/ignored.log",
        "src/build/a",
        "src/root-only",
        "src/kept.log",
    ] {
        fs::write(root.join(path), "first\r\nNeedle[1]\r\nNeedleX\n").unwrap();
    }
    let grep = prepare(GREP_KIND, json!({"workspace_root":root}));
    let result = call(
        &grep,
        json!({"pattern":"needle[1]", "ignore_case":true, "literal":true}),
    )
    .unwrap();
    assert!(result["matches"].is_secret());
    assert_eq!(
        result["matches"],
        json!([
            {"path":"src/kept.log", "line":2, "text":"Needle[1]"},
            {"path":"src/root-only", "line":2, "text":"Needle[1]"}
        ])
    );
    assert_eq!(result["truncated"], json!(false));
    let limited = call(
        &grep,
        json!({"path":"src", "pattern":"Needle.$", "limit":1}),
    )
    .unwrap();
    assert_eq!(
        limited["matches"],
        json!([{"path":"src/kept.log", "line":3, "text":"NeedleX"}])
    );
    assert_eq!(limited["truncated"], json!(true));
    let exact = call(
        &grep,
        json!({"path":"src/kept.log", "pattern":"Needle.$", "limit":1}),
    )
    .unwrap();
    assert_eq!(exact["truncated"], json!(false));
    for path in ["src/build", "src/ignored.log"] {
        assert_eq!(
            call(&grep, json!({"path":path, "pattern":"Needle"})).unwrap()["matches"],
            json!([])
        );
    }
}

#[test]
fn grep_skips_binary_large_and_unsafe_entries_without_reading_external_rules() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("workspace");
    fs::create_dir_all(root.join(".git")).unwrap();
    let outside = dir.path().join("outside");
    fs::write(&outside, "*\nsecret").unwrap();
    symlink(&outside, root.join(".gitignore")).unwrap();
    symlink(dir.path(), root.join("alias")).unwrap();
    fs::hard_link(&outside, root.join("hard")).unwrap();
    for path in [".git/config", ".mfn-coding-private", "ok"] {
        fs::write(root.join(path), "secret").unwrap();
    }
    fs::write(root.join("binary"), b"secret\n\x00").unwrap();
    fs::write(root.join("invalid"), b"secret\n\xff").unwrap();
    fs::write(
        root.join("large"),
        format!("secret{}", "x".repeat(1024 * 1024)),
    )
    .unwrap();
    let grep = prepare(GREP_KIND, json!({"workspace_root":root}));
    assert_eq!(
        call(&grep, json!({"pattern":"secret"})).unwrap()["matches"],
        json!([{"path":"ok", "line":1, "text":"secret"}])
    );
    for path in ["..", ".git", "alias", "hard", "alias/outside", "/"] {
        business(call(&grep, json!({"path":path, "pattern":"."})));
    }
    for input in [
        json!({"pattern":"["}),
        json!({"pattern":""}),
        json!({"pattern":"x", "limit":0}),
        json!({"pattern":"x", "limit":1001}),
    ] {
        business(call(&grep, input));
    }
    fs::write(root.join("ok"), "\t".repeat(30_000)).unwrap();
    let escaped = call(&grep, json!({"path":"ok", "pattern":"."})).unwrap();
    assert_eq!(escaped["matches"], json!([]));
    assert_eq!(escaped["truncated"], json!(true));
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
    assert!(!dir.path().join("new").exists());
    assert_eq!(fs::read_to_string(outside).unwrap(), "outside");
}

#[test]
fn mkdir_creates_only_the_leaf_under_a_locked_verified_parent() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("workspace");
    fs::create_dir(&root).unwrap();
    let node = prepare(MKDIR_KIND, json!({"workspace_root":root}));
    assert_eq!(
        call(&node, json!({"path":"notes"})).unwrap()["created"],
        json!(true)
    );
    assert_eq!(
        root.join("notes").metadata().unwrap().permissions().mode() & 0o077,
        0
    );
    symlink(dir.path(), root.join("alias")).unwrap();
    fs::write(root.join("file"), "original").unwrap();
    for path in [
        "notes",
        "file",
        "file/sub",
        "missing/nested",
        "alias/new",
        "alias",
        "../new",
        ".GiT",
        "notes/.mfn-coding-hidden",
        ".",
    ] {
        business(call(&node, json!({"path":path})));
    }
    let lock = fs::File::open(root.join("notes")).unwrap();
    flock(&lock, FlockOperation::NonBlockingLockExclusive).unwrap();
    assert!(matches!(
        business(call(&node, json!({"path":"notes/new"}))).downcast_ref::<CodingError>(),
        Some(CodingError::DirectoryBusy)
    ));
    drop(lock);
    assert!(!root.join("notes/new").exists());
    call(&node, json!({"path":"notes/new"})).unwrap();
    assert!(root.join("notes/new").is_dir());
    assert!(!root.join("missing").exists());
    assert!(!dir.path().join("new").exists());
    assert_eq!(fs::read_to_string(root.join("file")).unwrap(), "original");
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
    drop(lock);
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
    assert_eq!(signalled.skipped, ["exit_code".into()].into());
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
    assert_eq!(result["stdout"].as_str(), Some("12345"));
    assert_eq!(result["stderr"].as_str(), Some("abcde"));
    assert_eq!(result["stdout_truncated"].as_bool(), Some(true));
    assert_eq!(result["stderr_truncated"].as_bool(), Some(true));
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
        assert_eq!(result.skipped, ["exit_code".into(), "signal".into()].into());
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
                source.downcast_ref::<CodingError>(),
                Some(CodingError::Cancelled { .. })
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
    assert!(
        registry
            .get(READ_KIND)
            .unwrap()
            .instantiate(json!({"workspace_root": "."}))
            .is_err()
    );
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
