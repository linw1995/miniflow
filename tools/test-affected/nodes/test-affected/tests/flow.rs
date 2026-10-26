#![cfg(unix)]
mod common;

extern crate mfn_code as _;
extern crate mfn_core as _;
extern crate mfn_test_affected as _;

use common::*;
use mf_compiler::{
    CompileRequest, NodeRegistry, RunnerOptions, SupportPackages, WorkflowDefinition,
    compile_definition, compile_project_with_options, instantiate_stream,
};
use mf_runtime::{FlowRuntime, StreamOptions, WorkflowArguments};
use mfn_test_affected::TestPlan;
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

/// Mirrors the plan-side platform-independent toolchain identity
/// (`rustc -vV` release and commit hash; the host line is irrelevant).
fn graph_toolchain_identity(verbose: &str) -> String {
    let release = verbose
        .lines()
        .find_map(|line| line.strip_prefix("release: "))
        .unwrap();
    let commit = verbose
        .lines()
        .find_map(|line| line.strip_prefix("commit-hash: "))
        .unwrap();
    format!("rustc {release} ({commit})")
}

fn definition() -> Value {
    let package = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut value: Value = serde_json::from_str(
        &fs::read_to_string(package.join("../../flows/test-affected.json")).unwrap(),
    )
    .unwrap();
    for dependency in value["dependencies"].as_object_mut().unwrap().values_mut() {
        dependency["path"] = json!(
            package
                .join("../../flows")
                .join(dependency["path"].as_str().unwrap())
                .canonicalize()
                .unwrap()
        );
    }
    value["nodes"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|node| node["kind"] == "process.run")
        .unwrap()["config"]["args"] = json!([
        package.join("../../run-tests.py").canonicalize().unwrap(),
        "-"
    ]);
    value
}

#[test]
fn generated_flow_matches_memory_drains_once_and_rejects_cross_domain_bypass() {
    let (directory, base) = fixture();
    let root = directory.path();
    write(root, "core/src/lib.rs", &SOURCE.replace("1, 1", "2, 2"));
    let head = commit(root);
    let value = definition();
    let registry = NodeRegistry::from_inventory().unwrap();
    let workflow: WorkflowDefinition = serde_json::from_value(value.clone()).unwrap();
    let compiled = compile_definition(&workflow, &registry).unwrap();
    let prepared = instantiate_stream(&compiled, &registry).unwrap();
    let arguments = json!({"params.repository":root,"params.mode":"snapshot","params.base":base,"params.head":head,
        // Pin offline: without an explicit argument this parameter binds
        // GITHUB_REPOSITORY from the environment and would reach the real API.
        "params.github_repository":""});
    let instance = FlowRuntime::default()
        .start_stream(
            prepared,
            StreamOptions {
                arguments: WorkflowArguments::try_from(arguments.clone()).unwrap(),
                ..Default::default()
            },
        )
        .unwrap();
    let output = instance.recv().unwrap().unwrap();
    let memory: TestPlan =
        serde_json::from_value(serde_json::to_value(&output.outputs["plan"]).unwrap()).unwrap();
    assert!(instance.recv().unwrap().is_none());
    let summary = instance.join().unwrap();
    // The planning path is Task-only: no stream producer emits messages.
    assert_eq!(summary.emitted_messages, 0);
    assert_eq!(memory.tests.len(), 1);
    assert_eq!(memory.tests[0].name, "tests::changed");
    assert!(memory.packages.is_empty());

    let flow = root.join("target/test.flow.json");
    fs::write(&flow, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
    let executable = root.join("target/test-flow");
    let build = root.join("target/build");
    let crates = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../../crates")
        .canonicalize()
        .unwrap();
    compile_project_with_options(
        &CompileRequest {
            definition: &flow,
            output: &executable,
            locked: false,
            build_dir: Some(&build),
            support: &SupportPackages::Local { crates_dir: crates },
        },
        &RunnerOptions { telemetry: false },
    )
    .unwrap();
    let output = Command::new(&executable)
        .current_dir(std::env::temp_dir())
        .env("GITHUB_ACTIONS", "true")
        .args(["--inputs", &arguments.to_string()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let records: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(records.len(), 1);
    let generated: TestPlan = serde_json::from_value(records[0]["plan"].clone()).unwrap();
    assert_eq!(generated.tests, memory.tests);
    assert_eq!(generated.packages, memory.packages);
    assert_eq!(generated.head, memory.head);
    assert!(records[0].get("execution").is_none());
    let mut execute = arguments.clone();
    execute["params.run"] = json!(true);
    let memory_execution = memory_result(&value, &execute).unwrap();
    assert_eq!(memory_execution["execution"]["selected_count"], json!(1));
    assert_eq!(memory_execution["execution"]["executed"], json!(true));
    for empty in [false, true] {
        let mut execute = execute.clone();
        if empty {
            execute["params.base"] = json!(head);
        }
        let output = Command::new(&executable)
            .current_dir(std::env::temp_dir())
            .args(["--inputs", &execute.to_string()])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["execution"]["executed"], json!(!empty));
        assert_eq!(
            report["execution"]["selected_count"],
            json!(if empty { 0 } else { 1 })
        );
    }

    write(root, "core/src/lib.rs", &SOURCE.replace("1, 1", "2, 3"));
    execute["params.head"] = json!(commit(root));
    let output = Command::new(&executable)
        .args(["--inputs", &execute.to_string()])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("FAIL"));

    let pidfile = root.join("target/running-test.pid");
    write(
        root,
        "core/src/lib.rs",
        &format!(
            "#[test] fn waits() {{ std::fs::write({:?}, std::process::id().to_string()).unwrap(); std::thread::sleep(std::time::Duration::from_secs(60)); }}\n",
            pidfile.to_str().unwrap()
        ),
    );
    execute["params.head"] = json!(commit(root));
    execute["params.precision"] = json!("packages");
    use std::os::unix::process::CommandExt;
    let mut child = Command::new(&executable)
        .args(["--inputs", &execute.to_string()])
        .process_group(0)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while !pidfile.exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGINT);
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut status = None;
    while std::time::Instant::now() < deadline {
        status = child.try_wait().unwrap();
        if status.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
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
            libc::kill(pid, libc::SIGKILL);
        }
    }
    assert!(status.is_some() && !completed.success());
    assert!(!survived, "selected test survived runner interruption");
}

struct GithubFixture {
    directory: tempfile::TempDir,
    child: std::process::Child,
    base: String,
}
impl GithubFixture {
    fn new(state: &Value) -> Self {
        use std::io::BufRead;
        let directory = tempfile::tempdir().unwrap();
        let config = directory.path().join("config.json");
        fs::write(&config, state.to_string()).unwrap();
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/common/github.py");
        let mut child = Command::new("python3")
            .arg(script)
            .arg(config)
            .arg(directory.path().join("requests"))
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut base = String::new();
        std::io::BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut base)
            .unwrap();
        assert!(base.starts_with("http://127.0.0.1:"));
        Self {
            directory,
            child,
            base: base.trim().into(),
        }
    }
    fn configure(&self, state: &Value) {
        fs::write(self.directory.path().join("config.json"), state.to_string()).unwrap();
        fs::write(self.directory.path().join("requests"), "").unwrap();
    }
    fn requests(&self) -> String {
        fs::read_to_string(self.directory.path().join("requests")).unwrap_or_default()
    }
}
impl Drop for GithubFixture {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
fn configure_github(nodes: &mut Value, base: &str) {
    for node in nodes.as_array_mut().unwrap() {
        if node["kind"] == "github.read" || node["kind"] == "github.download" {
            node["config"]["api_base"] = json!(base);
        }
        if let Some(body) = node.get_mut("loop").and_then(|v| v.get_mut("body")) {
            configure_github(&mut body["nodes"], base);
        }
    }
}
fn merge_commit(root: &Path, base: &str, head: &str) -> String {
    let tree =
        mfn_test_affected::git::command(root, "git", &["rev-parse", &format!("{head}^{{tree}}")])
            .unwrap();
    mfn_test_affected::git::command(
        root,
        "git",
        &[
            "commit-tree",
            tree.trim(),
            "-p",
            base,
            "-p",
            head,
            "-m",
            "test(ci): merge fixture",
        ],
    )
    .unwrap()
    .trim()
    .into()
}
fn memory_result(definition: &Value, arguments: &Value) -> Result<Value, String> {
    let registry = NodeRegistry::from_inventory().unwrap();
    let definition: WorkflowDefinition = serde_json::from_value(definition.clone()).unwrap();
    let compiled = compile_definition(&definition, &registry).unwrap();
    let prepared = instantiate_stream(&compiled, &registry).unwrap();
    let instance = FlowRuntime::default()
        .start_stream(
            prepared,
            StreamOptions {
                arguments: WorkflowArguments::try_from(arguments.clone()).unwrap(),
                ..Default::default()
            },
        )
        .unwrap();
    let result = instance.recv();
    let finished = instance.join();
    match (result, finished) {
        (Ok(Some(message)), Ok(summary)) => {
            // The planning path is Task-only: no stream producer emits messages.
            assert_eq!(summary.emitted_messages, 0);
            Ok(serde_json::to_value(message.outputs).unwrap())
        }
        (result, finished) => Err(format!("{result:?}; {finished:?}")),
    }
}

#[test]
fn unified_flow_replays_ci_evidence_and_keeps_local_execution_offline() {
    use mfn_test_affected::git;
    let (source, base) = fixture();
    let origin = source.path();
    git::command(origin, "git", &["switch", "-qc", "topic"]).unwrap();
    write(origin, "core/src/lib.rs", &SOURCE.replace("{ 1 }", "{ 2 }"));
    let prior = commit(origin);
    write(origin, "core/src/lib.rs", &SOURCE.replace("{ 1 }", "{ 3 }"));
    let nearest = commit(origin);
    write(origin, "app/src/lib.rs", &SOURCE.replace("1, 1", "4, 4"));
    let head = commit(origin);
    let older_merge = merge_commit(origin, &base, &prior);
    let newer_merge = merge_commit(origin, &base, &nearest);
    git::command(
        origin,
        "git",
        &["update-ref", "refs/heads/validation-one", &older_merge],
    )
    .unwrap();
    git::command(
        origin,
        "git",
        &["update-ref", "refs/heads/validation-two", &newer_merge],
    )
    .unwrap();
    let checkout = tempfile::tempdir().unwrap();
    let root = checkout.path();
    git::command(
        root,
        "git",
        &[
            "clone",
            "-q",
            "--no-local",
            "--single-branch",
            "--branch",
            "topic",
            origin.to_str().unwrap(),
            ".",
        ],
    )
    .unwrap();
    for (key, value) in [
        ("user.name", "Fixture"),
        ("user.email", "fixture@example.com"),
        ("commit.gpgsign", "false"),
        ("core.hooksPath", "/dev/null"),
    ] {
        git::command(root, "git", &["config", key, value]).unwrap();
    }
    let tested = merge_commit(root, &base, &head);
    git::command(root, "git", &["checkout", "--detach", "-q", &tested]).unwrap();
    assert!(git::revision(root, &newer_merge).is_err());
    let toolchain = git::command(root, "rustc", &["-vV"]).unwrap();
    let environment: serde_json::Map<_, _> = [
        "RUSTFLAGS",
        "CARGO_ENCODED_RUSTFLAGS",
        "CARGO_INCREMENTAL",
        "CARGO_PROFILE_DEV_DEBUG",
        "CARGO_PROFILE_TEST_DEBUG",
    ]
    .into_iter()
    .map(|key| (key.to_owned(), json!(std::env::var(key).ok())))
    .collect();
    let signature = json!({"policy":"flow-pr-v3","coverage_target":95.0,"toolchain":toolchain,"features":"all","profile":"ci","target":std::env::var("CARGO_BUILD_TARGET").ok(),"environment":environment});
    // The coverage-graph signature omits policy fields and records only the
    // environment variables that are set, matching scripts/run-cov.sh.
    let graph_signature = {
        let environment: serde_json::Map<_, _> = [
            "RUSTFLAGS",
            "CARGO_ENCODED_RUSTFLAGS",
            "CARGO_INCREMENTAL",
            "CARGO_PROFILE_DEV_DEBUG",
            "CARGO_PROFILE_TEST_DEBUG",
        ]
        .into_iter()
        .filter_map(|name| {
            std::env::var(name)
                .ok()
                .map(|value| (name.to_owned(), json!(value)))
        })
        .collect();
        json!({"environment":environment,"features":"all","profile":"ci","target":std::env::var("CARGO_BUILD_TARGET").ok(),"toolchain":graph_toolchain_identity(&toolchain)})
            .to_string()
    };
    let graphs_dir = root.join("target/graphs");
    let build_graph = |name: &str, commit: &str, run: u64, hits: &[(u32, u32)]| {
        use mfn_test_affected::graph::{CoverageManifest, CoveredTest, GraphWriter};
        let lcov_dir = graphs_dir.join(format!("lcov-{name}"));
        fs::create_dir_all(&lcov_dir).unwrap();
        let mut export = "SF:core/src/lib.rs\n".to_string();
        for (line, count) in hits {
            export.push_str(&format!("DA:{line},{count}\n"));
        }
        export.push_str("end_of_record\n");
        let lcov = lcov_dir.join("stable.lcov");
        fs::write(&lcov, export).unwrap();
        let manifest = CoverageManifest {
            commit_sha: commit.into(),
            run_id: run.to_string(),
            attempt: 1,
            signature: graph_signature.clone(),
            tests: vec![CoveredTest {
                package: "sample-core".into(),
                binary: "sample-core".into(),
                name: "tests::stable".into(),
                lcov,
            }],
        };
        let path = graphs_dir.join(format!("graph-{name}.sqlite"));
        let mut writer = GraphWriter::create(&path).unwrap();
        writer.index(&manifest, root).unwrap();
        path
    };
    let graph_metadata = |commit: &str, run: u64| json!({"commit_sha":commit,"run_id":run.to_string(),"attempt":1,"schema_version":1,"signature":graph_signature});
    let record = |id: u64, head: &str, merge: &str| {
        let tree =
            git::command(origin, "git", &["rev-parse", &format!("{merge}^{{tree}}")]).unwrap();
        json!({"version":1,"repository":"fixture/repo","pr":1,"workflow_id":7,"run_id":id,"run_attempt":1,"base":base,"head":head,"merge":merge,"tree":tree.trim(),"signature":signature,"comparison":{"base":base,"head":merge,"scope":"cumulative","reason":"fixture"},"coverage":{"base":base,"head":merge,"tree":tree.trim(),"covered":100,"total":100,"target":95.0,"status":"passed"},"packages":["sample-core","sample-app"],"title_check":{"head_sha":head,"conclusion":"success"}})
    };
    let current =
        json!({"id":99,"run_attempt":1,"workflow_id":7,"head_sha":head,"event":"pull_request"});
    let runs = json!([{"id":11,"run_attempt":1,"workflow_id":7,"head_sha":prior,"status":"completed","conclusion":"success"},{"id":17,"run_attempt":1,"workflow_id":7,"head_sha":nearest,"status":"completed","conclusion":"success"}]);
    let initial = json!({"current":current,"pull":{"head":{"sha":head},"merge_commit_sha":tested},"runs":runs,"records":{"11":record(11,&prior,&older_merge),"17":record(17,&nearest,&newer_merge)},
        "graph_runs":[{"id":23,"run_attempt":1,"head_sha":base,"status":"completed","conclusion":"success"},{"id":29,"run_attempt":1,"head_sha":prior,"status":"completed","conclusion":"success"}],
        // The default graphs cover only line 2 of core/src/lib.rs; the changed
        // line 1 stays uncovered and widens conservatively to package scope.
        "graphs":{"23":{"sqlite":build_graph("base",&base,23,&[(2,1)]),"metadata":graph_metadata(&base,23)},"29":{"sqlite":build_graph("prior",&prior,29,&[(2,1)]),"metadata":graph_metadata(&prior,29)}}});
    let server = GithubFixture::new(&initial);
    let mut flow = definition();
    configure_github(&mut flow["nodes"], &server.base);
    let args = json!({"params.ci":true,"params.repository":root,"params.github_repository":"fixture/repo","params.run_id":99,"params.attempt":1,"params.pr":1,"params.branch":"topic","params.event":"pull_request","params.expected_head":head,"params.expected_merge":null,"params.profile":"ci","params.toolchain":toolchain.trim_end(),"params.token":"fixture-token","params.cumulative":false});
    let path = root.join("target/ci.flow.json");
    write(root, "target/ci.flow.json", &flow.to_string());
    let executable = root.join("target/ci-flow");
    let build = root.join("target/ci-build");
    let crates = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../../crates")
        .canonicalize()
        .unwrap();
    compile_project_with_options(
        &CompileRequest {
            definition: &path,
            output: &executable,
            locked: false,
            build_dir: Some(&build),
            support: &SupportPackages::Local { crates_dir: crates },
        },
        &RunnerOptions { telemetry: false },
    )
    .unwrap();
    assert!(server.requests().is_empty());
    let refs = git::command(root, "git", &["show-ref"]).unwrap();
    let index = fs::read(root.join(".git/index")).unwrap();
    for case in [
        "incremental",
        "local",
        "older_success",
        "reset",
        "empty",
        "explicit",
        "manual",
        "invalid_current",
        "graph_absent",
        "graph_incompatible",
        "graph_foreign",
        "graph_narrows",
        "graph_full_page",
        "graph_overfull_page",
        "graph_lagging",
        "ci_execution",
        "invalid_record",
        "failed_run",
        "missing_artifact",
        "failed_coverage",
        "wrong_tree",
        "wrong_attempt",
        "wrong_pr",
        "wrong_head",
        "pending_run",
        "unrelated_head",
        "invalid_zero_coverage",
        "event_head_mismatch",
        "manual_mismatch",
    ] {
        let mut state = initial.clone();
        let mut args = args.clone();
        let (scope, packages, expected_base) = match case {
            "local" => {
                args.as_object_mut().unwrap().remove("params.ci");
                // Local planning stays offline unless a repository identity is
                // explicitly supplied for coverage graph acquisition. Pin the
                // identity explicitly: the parameter binds GITHUB_REPOSITORY
                // from the environment when the argument is absent, which
                // differs between runners.
                args["params.github_repository"] = json!("");
                args["params.mode"] = json!("snapshot");
                args["params.base"] = json!(base);
                args["params.head"] = json!(head);
                args["params.precision"] = json!("packages");
                (None, vec!["sample-app", "sample-core"], base.as_str())
            }
            "older_success" => {
                state["records"]["17"]["signature"]["policy"] = json!("old-policy");
                (
                    Some("incremental"),
                    vec!["sample-app", "sample-core"],
                    older_merge.as_str(),
                )
            }
            "reset" => {
                for id in ["11", "17"] {
                    state["records"][id]["signature"]["policy"] = json!("old-policy");
                }
                (
                    Some("cumulative"),
                    vec!["sample-app", "sample-core"],
                    base.as_str(),
                )
            }
            "empty" => {
                state["runs"] = json!([]);
                (
                    Some("cumulative"),
                    vec!["sample-app", "sample-core"],
                    base.as_str(),
                )
            }
            "explicit" => {
                args["params.cumulative"] = json!(true);
                (
                    Some("cumulative"),
                    vec!["sample-app", "sample-core"],
                    base.as_str(),
                )
            }
            "manual" => {
                args["params.event"] = json!("workflow_dispatch");
                args["params.expected_merge"] = json!(tested);
                state["current"]["event"] = json!("workflow_dispatch");
                (
                    Some("cumulative"),
                    vec!["sample-app", "sample-core"],
                    base.as_str(),
                )
            }
            "invalid_current" => {
                state["current"]["head_sha"] = json!("invalid");
                (None, vec![], "")
            }
            "invalid_record" => {
                state["records"]["17"] = json!({"invalid":true});
                (
                    Some("incremental"),
                    vec!["sample-app", "sample-core"],
                    older_merge.as_str(),
                )
            }
            "failed_run" => {
                state["runs"][1]["conclusion"] = json!("failure");
                (
                    Some("incremental"),
                    vec!["sample-app", "sample-core"],
                    older_merge.as_str(),
                )
            }
            "missing_artifact" => {
                state["records"].as_object_mut().unwrap().remove("17");
                (
                    Some("incremental"),
                    vec!["sample-app", "sample-core"],
                    older_merge.as_str(),
                )
            }
            "failed_coverage" => {
                state["records"]["17"]["coverage"]["covered"] = json!(90);
                (
                    Some("incremental"),
                    vec!["sample-app", "sample-core"],
                    older_merge.as_str(),
                )
            }
            "wrong_tree" => {
                state["records"]["17"]["tree"] = json!("0".repeat(40));
                state["records"]["17"]["coverage"]["tree"] = json!("0".repeat(40));
                (
                    Some("incremental"),
                    vec!["sample-app", "sample-core"],
                    older_merge.as_str(),
                )
            }
            "wrong_attempt" => {
                state["records"]["17"]["run_attempt"] = json!(2);
                (
                    Some("incremental"),
                    vec!["sample-app", "sample-core"],
                    older_merge.as_str(),
                )
            }
            "wrong_pr" => {
                state["records"]["17"]["pr"] = json!(2);
                (
                    Some("incremental"),
                    vec!["sample-app", "sample-core"],
                    older_merge.as_str(),
                )
            }
            "wrong_head" => {
                state["records"]["17"]["head"] = json!(prior);
                (
                    Some("incremental"),
                    vec!["sample-app", "sample-core"],
                    older_merge.as_str(),
                )
            }
            "pending_run" => {
                state["runs"][1]["status"] = json!("in_progress");
                (
                    Some("incremental"),
                    vec!["sample-app", "sample-core"],
                    older_merge.as_str(),
                )
            }
            "unrelated_head" => {
                state["runs"][1]["head_sha"] = json!("f".repeat(40));
                (
                    Some("incremental"),
                    vec!["sample-app", "sample-core"],
                    older_merge.as_str(),
                )
            }
            "invalid_zero_coverage" => {
                state["records"]["17"]["coverage"]["total"] = json!(0);
                state["records"]["17"]["coverage"]["covered"] = json!(1);
                state["records"]["17"]["coverage"]["status"] = json!("not_applicable");
                (
                    Some("incremental"),
                    vec!["sample-app", "sample-core"],
                    older_merge.as_str(),
                )
            }
            "graph_absent" => {
                state["graph_runs"] = json!([]);
                (Some("incremental"), vec![], newer_merge.as_str())
            }
            "graph_incompatible" => {
                for run in ["23", "29"] {
                    state["graphs"][run]["metadata"]["schema_version"] = json!(999);
                }
                (Some("incremental"), vec![], newer_merge.as_str())
            }
            "graph_foreign" => {
                // The nearest run's artifact binds a foreign commit; the
                // search advances to the older eligible ancestor.
                state["graphs"]["29"]["metadata"]["commit_sha"] = json!(base);
                (Some("incremental"), vec![], newer_merge.as_str())
            }
            "graph_overfull_page" => {
                state["graph_runs"] = json!(vec![state["graph_runs"][0].clone(); 101]);
                (Some("incremental"), vec![], newer_merge.as_str())
            }
            "graph_narrows" | "graph_full_page" => {
                args["params.cumulative"] = json!(true);
                // The graph at the comparison base covers the changed core
                // line, narrowing the package to the covering test; the app
                // test-only change narrows independently from source.
                state["graphs"]["23"]["sqlite"] = json!(build_graph(case, &base, 23, &[(1, 1)]));
                if case == "graph_full_page" {
                    // A full API page is a bounded candidate window, not a
                    // discovery failure. Only its last run is an ancestor.
                    let mut runs: Vec<Value> = (100..199)
                        .map(|id| {
                            json!({"id":id,"run_attempt":1,"head_sha":head,
                            "status":"completed","conclusion":"success"})
                        })
                        .collect();
                    runs.push(state["graph_runs"][0].clone());
                    state["graph_runs"] = json!(runs);
                }
                (Some("cumulative"), vec![], base.as_str())
            }
            "graph_lagging" => {
                // The nearest graph predates the comparison base, which
                // changed core/src/lib.rs since; the file widens to package
                // scope even though the graph covers that line.
                state["records"]["17"]["signature"]["policy"] = json!("old-policy");
                state["graphs"].as_object_mut().unwrap().remove("29");
                (
                    Some("incremental"),
                    vec!["sample-app", "sample-core"],
                    older_merge.as_str(),
                )
            }
            "ci_execution" => {
                args["params.run"] = json!(true);
                (None, vec![], "")
            }
            "event_head_mismatch" => {
                args["params.expected_head"] = json!("invalid");
                (None, vec![], "")
            }
            "manual_mismatch" => {
                args["params.event"] = json!("workflow_dispatch");
                args["params.expected_merge"] = json!("0".repeat(40));
                state["current"]["event"] = json!("workflow_dispatch");
                (None, vec![], "")
            }
            _ => (Some("incremental"), vec![], newer_merge.as_str()),
        };
        server.configure(&state);
        let memory = memory_result(&flow, &args);
        if matches!(
            case,
            "invalid_current" | "event_head_mismatch" | "manual_mismatch" | "ci_execution"
        ) {
            assert!(memory.is_err());
        } else {
            assert!(memory.is_ok(), "{case}: {memory:?}");
        }
        if case == "ci_execution" {
            assert!(server.requests().is_empty());
        }
        server.configure(&state);
        if case == "graph_full_page" {
            // Resolve the default relative repository before choosing the
            // artifact destination, just as the in-memory absolute input does.
            args["params.repository"] = json!(".");
        }
        let result = Command::new(&executable)
            .current_dir(root)
            .args(["--inputs", &args.to_string()])
            .output()
            .unwrap();
        if matches!(
            case,
            "invalid_current" | "event_head_mismatch" | "manual_mismatch" | "ci_execution"
        ) {
            assert!(!result.status.success() && result.stdout.is_empty());
            if case == "ci_execution" {
                assert!(server.requests().is_empty());
            }
            continue;
        }
        assert!(
            result.status.success(),
            "{case}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        let generated: Value = serde_json::from_slice(&result.stdout).unwrap();
        assert!(generated.get("execution").is_none(), "{case}");
        assert_eq!(generated["plan"]["packages"], json!(packages), "{case}");
        assert_eq!(generated["plan"]["base"], expected_base, "{case}");
        let graph = &generated["plan"]["coverage_graph"];
        match case {
            "local" => {
                assert_eq!(
                    graph["reason"],
                    "No GitHub repository identity supplied for coverage graph acquisition",
                    "{case}"
                );
                assert!(graph["commit"].is_null(), "{case}");
            }
            "graph_absent" => {
                assert_eq!(
                    graph["reason"], "No successful default-branch coverage run found",
                    "{case}"
                );
            }
            "graph_overfull_page" => {
                assert_eq!(
                    graph["reason"], "Coverage graph discovery limit reached",
                    "{case}"
                );
                assert!(graph["commit"].is_null(), "{case}");
            }
            "graph_incompatible" => {
                assert_eq!(
                    graph["reason"],
                    "No compatible coverage graph artifact on any eligible default-branch ancestor",
                    "{case}"
                );
            }
            _ => {
                let (graph_commit, graph_run) =
                    if matches!(case, "graph_foreign" | "graph_lagging") || expected_base == base {
                        (base.as_str(), "23")
                    } else {
                        (prior.as_str(), "29")
                    };
                assert_eq!(graph["commit"], graph_commit, "{case}");
                assert_eq!(graph["run_id"], graph_run, "{case}");
                assert_eq!(graph["attempt"], 1, "{case}");
                assert_eq!(graph["schema_version"], 1, "{case}");
                assert!(graph["reason"].is_null(), "{case}");
                // Test-only app changes do not query the graph. Production
                // core changes still record uncovered or lagging ranges.
                let widened = &graph["widened"];
                let expected: Vec<(&str, &str, &str)> = match case {
                    "graph_lagging" => vec![(
                        "core/src/lib.rs",
                        "sample-core",
                        "changed between the coverage graph commit",
                    )],
                    _ if expected_base == newer_merge
                        || matches!(case, "graph_narrows" | "graph_full_page") =>
                    {
                        vec![]
                    }
                    _ => vec![("core/src/lib.rs", "sample-core", "no test covers")],
                };
                assert_eq!(widened.as_array().unwrap().len(), expected.len(), "{case}");
                for (entry, (file, package, reason)) in
                    widened.as_array().unwrap().iter().zip(expected)
                {
                    assert_eq!(entry["file"], file, "{case}");
                    assert_eq!(entry["package"], package, "{case}");
                    assert!(entry["reason"].as_str().unwrap().contains(reason), "{case}");
                }
            }
        }
        if matches!(case, "graph_narrows" | "graph_full_page") {
            assert_eq!(
                generated["plan"]["tests"],
                json!([
                    {"package":"sample-app","binary":"sample-app","name":"tests::changed"},
                    {"package":"sample-core","binary":"sample-core","name":"tests::stable"}
                ]),
                "{case}"
            );
        } else if packages.is_empty() {
            assert_eq!(
                generated["plan"]["tests"],
                json!([{"package":"sample-app","binary":"sample-app","name":"tests::changed"}]),
                "{case}"
            );
        } else {
            assert!(
                generated["plan"]["tests"].as_array().unwrap().is_empty(),
                "{case}"
            );
        }
        if let Some(scope) = scope {
            assert_eq!(
                generated["validation"]["comparison"]["scope"], scope,
                "{case}"
            );
        } else {
            assert!(generated["validation"].is_null());
            assert!(server.requests().is_empty());
        }
        let memory = memory.unwrap();
        assert_eq!(generated["validation"], memory["validation"], "{case}");
        assert_eq!(
            generated["plan"]["packages"], memory["plan"]["packages"],
            "{case}"
        );
        assert_eq!(
            generated["plan"]["coverage_graph"], memory["plan"]["coverage_graph"],
            "{case}"
        );
        assert!(!String::from_utf8_lossy(&result.stdout).contains("fixture-token"));
        if case == "incremental" {
            assert!(!server.requests().contains("/runs/11/artifacts"));
        }
        if matches!(case, "incremental" | "reset" | "manual") {
            let handoff = root.join("target/handoff");
            fs::create_dir_all(&handoff).unwrap();
            fs::write(handoff.join("flow.json"), &result.stdout).unwrap();
            fs::write(
                handoff.join("event.json"),
                json!({"number":1,"inputs":{"pr":"1"}}).to_string(),
            )
            .unwrap();
            let accepted = Command::new("python3")
                .arg(
                    Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join("../../../../scripts/pr_validation.py"),
                )
                .args(["accept", "--coverage-target", "95", "--directory"])
                .arg(&handoff)
                .arg("--flow-output")
                .arg(handoff.join("flow.json"))
                .current_dir(root)
                .env("GITHUB_EVENT_PATH", handoff.join("event.json"))
                .env("GITHUB_EVENT_NAME", args["params.event"].as_str().unwrap())
                .env("GITHUB_REPOSITORY", "fixture/repo")
                .env("GITHUB_RUN_ID", "99")
                .env("GITHUB_RUN_ATTEMPT", "1")
                .env("NEXTEST_PROFILE", "ci")
                .env_remove("GITHUB_OUTPUT")
                .env_remove("GITHUB_STEP_SUMMARY")
                .output()
                .unwrap();
            assert!(
                accepted.status.success(),
                "{case}: {}",
                String::from_utf8_lossy(&accepted.stderr)
            );
            let accepted: Value =
                serde_json::from_slice(&fs::read(handoff.join("plan.json")).unwrap()).unwrap();
            assert_eq!(accepted, generated["plan"]);
        }
        if matches!(case, "failed_run" | "pending_run" | "unrelated_head") {
            assert!(!server.requests().contains("/runs/17/artifacts"));
        }
        if case == "explicit" || case == "manual" {
            // The baseline record channel stays idle; coverage graph
            // acquisition is independent of the baseline search policy.
            assert!(!server.requests().contains("branch=topic"), "{case}");
        }
    }
    assert_eq!(git::command(root, "git", &["show-ref"]).unwrap(), refs);
    assert_eq!(git::revision(root, "HEAD").unwrap(), tested);
    assert_eq!(fs::read(root.join(".git/index")).unwrap(), index);
}
