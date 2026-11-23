//! Built-in Flow listing and description without a compiled executable.

use std::process::Command;

fn command() -> Command {
    Command::new(env!("CARGO_BIN_EXE_mf"))
}

#[test]
fn lists_linked_builtin_flows_in_sorted_order() {
    let output = command().arg("flows").output().unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    let stdout = String::from_utf8(output.stdout).unwrap();
    let names: Vec<_> = stdout.lines().collect();
    assert!(names.contains(&"mf-chat.chat-title"), "{stdout}");
    assert!(names.contains(&"mf-chat.conversation"), "{stdout}");
    assert!(names.contains(&"mfn-llm.agent"), "{stdout}");
    assert!(names.contains(&"mfn-llm.agent-turn"), "{stdout}");
    let mut sorted = names.clone();
    sorted.sort_unstable();
    assert_eq!(names, sorted, "{stdout}");
}

#[test]
fn unknown_builtin_flow_names_the_listing_command() {
    let output = command().args(["flows", "missing.flow"]).output().unwrap();
    assert!(!output.status.success(), "{output:?}");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("unknown built-in Flow `missing.flow`"),
        "{stderr}"
    );
    assert!(stderr.contains("mf flows"), "{stderr}");
}

#[test]
fn describes_a_builtin_flow_before_requiring_a_terminal() {
    // Piped stdio cannot present the graph, so reaching the terminal diagnostic
    // proves the linked definition resolved into a description.
    for name in [
        "mfn-llm.agent",
        "mfn-llm.agent-turn",
        "mf-chat.chat-title",
        "mf-chat.conversation",
    ] {
        let output = command().args(["flows", name]).output().unwrap();
        assert!(!output.status.success(), "{output:?}");
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(
            stderr.contains("TUI requires terminal stdin and stderr"),
            "{name}: {stderr}"
        );
    }
}
