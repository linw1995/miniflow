import argparse
import asyncio
from contextlib import redirect_stdout
import io
from pathlib import Path
import subprocess
import sys
import tempfile
import textwrap
import unittest
from unittest.mock import patch

import affected_tests as affected


def package(root, name, dependencies=(), targets=None):
    return {
        "name": name, "manifest_path": str(root / "crates" / name / "Cargo.toml"),
        "targets": [{"name": target} for target in (targets or [name.replace("-", "_")])],
        "dependencies": [{"path": str(root / "crates" / dep)} for dep in dependencies],
    }


def options(**values):
    defaults = dict(commit=None, base=None, head=None, worktree=False, target=None, rust_analyzer="rust-analyzer", timeout=5)
    return argparse.Namespace(**(defaults | values))


class SelectionTests(unittest.TestCase):
    def setUp(self):
        self.root = Path("/workspace")
        self.packages = {
            "core-lib": package(self.root, "core-lib"),
            "app": package(self.root, "app", ["core-lib"]),
            "unrelated": package(self.root, "unrelated"),
        }

    def test_reverse_closure_includes_consumers_but_not_dependencies_or_unrelated(self):
        graph = {"core-lib": set(), "app": {"core-lib"}, "host": {"app"}, "unrelated": set()}
        self.assertEqual(set(affected.select({"core-lib": ["src/lib.rs"]}, graph)), {"core-lib", "app", "host"})
        self.assertEqual(set(affected.select({"app": ["src/main.rs"]}, graph)), {"app", "host"})

    def test_dev_dependency_cycle_terminates(self):
        self.assertEqual(set(affected.select({"a": ["test.rs"]}, {"a": {"b"}, "b": {"a"}})), {"a", "b"})

    def test_graph_maps_custom_targets_and_disambiguates_duplicate_names(self):
        self.packages["app"]["targets"] = [{"name": "custom-bin"}, {"name": "common"}]
        self.packages["unrelated"]["targets"] = [{"name": "common"}]
        dot = '''digraph rust_analyzer_crate_graph {
    _1[label="core_lib"][shape="box"];
    _2[label="custom_bin"][shape="box"];
    _3[label="common"][shape="box"];
    _3 -> _1[label=""];
}'''
        graph, supplements = affected.package_graph(dot, self.packages)
        self.assertEqual(graph, {"core-lib": set(), "app": {"core-lib"}, "unrelated": set()})
        self.assertEqual(supplements, [])

    def test_missing_cfg_build_or_dev_edges_are_supplemented(self):
        dot = '''digraph rust_analyzer_crate_graph {
    _1[label="core_lib"][shape="box"];
    _2[label="app"][shape="box"];
    _3[label="unrelated"][shape="box"];
}'''
        graph, supplements = affected.package_graph(dot, self.packages)
        self.assertEqual(graph["app"], {"core-lib"})
        self.assertEqual(supplements, ["app -> core-lib"])

    def test_empty_partial_and_unrecognized_graphs_fail_closed(self):
        for dot in ["", "<svg/>", 'digraph rust_analyzer_crate_graph {\n}',
                    'digraph rust_analyzer_crate_graph {\n_1[label="unknown"][shape="box"];\n}']:
            with self.subTest(dot=dot), self.assertRaises(affected.AnalysisError):
                affected.package_graph(dot, self.packages)

    def test_crate_assets_and_deleted_files_have_owners(self):
        direct, ignored, fallback = affected.classify(self.root, [
            "crates/core-lib/src/deleted.rs", "crates/app/migrations/schema.sql",
            "crates/app/tests/fixtures/README.md", "docs/guide.md",
        ], self.packages)
        self.assertEqual(set(direct), {"core-lib", "app"})
        self.assertEqual(ignored, ["docs/guide.md"])
        self.assertEqual(fallback, [])

    def test_unknown_shared_and_dependency_inputs_select_full_suite(self):
        for path in ["Cargo.toml", "Cargo.lock", "crates/app/Cargo.toml", "flake.lock",
                     ".cargo/config.toml", "examples/example.json", "scripts/helper.py",
                     "crates/removed/src/lib.rs", ".config/nextest.toml"]:
            with self.subTest(path=path):
                self.assertTrue(affected.classify(self.root, [path], self.packages)[2])

    def test_nested_fixture_manifest_forces_full_suite(self):
        self.assertTrue(affected.classify(self.root, ["crates/app/tests/fixture/Cargo.toml"], self.packages)[2])

    def test_analyzer_failure_produces_full_plan_not_empty_selection(self):
        with patch.object(affected, "changes", return_value=("base", "head", ["crates/app/src/main.rs"])), \
                patch.object(affected, "workspace_packages", return_value=self.packages), \
                patch.object(affected, "analyzer_graph", side_effect=OSError("missing executable")):
            plan = affected.make_plan(self.root, options())
        self.assertEqual(plan["mode"], "all")
        self.assertEqual(plan["packages"], sorted(self.packages))
        self.assertIn("missing executable", plan["fallback_reasons"][0])
        self.assertNotIn("--workspace", plan["command"])

    def test_docs_only_plan_never_invokes_analyzer_or_returns_default_member_command(self):
        with patch.object(affected, "changes", return_value=("base", "head", ["docs/guide.md"])), \
                patch.object(affected, "workspace_packages", return_value=self.packages), \
                patch.object(affected, "analyzer_graph") as analyzer:
            plan = affected.make_plan(self.root, options())
        analyzer.assert_not_called()
        self.assertEqual(plan["mode"], "none")
        self.assertEqual(plan["command"], [])

    def test_run_skips_empty_selection_and_propagates_test_failure(self):
        for argv, expected in [([], 0), (["cargo", "nextest", "run", "-p", "app"], 42)]:
            with self.subTest(argv=argv), \
                    patch.object(sys, "argv", ["affected_tests.py", "--worktree", "--run"]), \
                    patch.object(affected, "command", return_value=str(self.root)), \
                    patch.object(affected, "make_plan", return_value={
                        "mode": "affected" if argv else "none", "packages": [], "command": argv,
                    }), \
                    patch.object(affected.subprocess, "run", return_value=subprocess.CompletedProcess(argv, 42)) as run, \
                    redirect_stdout(io.StringIO()):
                self.assertEqual(affected.main(), expected)
                if argv:
                    run.assert_called_once_with(argv, cwd=self.root)
                else:
                    run.assert_not_called()


class GitTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.git("init", "-b", "main")
        self.git("config", "user.email", "tests@example.com")
        self.git("config", "user.name", "Test")
        self.git("config", "commit.gpgsign", "false")
        self.git("config", "core.hooksPath", "/dev/null")
        self.write("original.rs", "original")
        self.commit()

    def git(self, *args):
        return affected.command(self.root, "git", *args).strip()

    def write(self, name, contents):
        file = self.root / name
        file.parent.mkdir(parents=True, exist_ok=True)
        file.write_text(contents)

    def commit(self):
        self.git("add", ".")
        self.git("commit", "-m", "test(ci): fixture")
        return self.git("rev-parse", "HEAD")

    def test_root_commit_lists_whole_tree(self):
        base, _, paths = affected.changes(self.root, options(commit="HEAD"))
        self.assertIsNone(base)
        self.assertEqual(paths, ["original.rs"])

    def test_rename_reports_both_paths_and_preserves_spaces_and_newlines(self):
        self.git("mv", "original.rs", "renamed file\nname.rs")
        self.commit()
        _, _, paths = affected.changes(self.root, options(commit="HEAD"))
        self.assertEqual(paths, ["original.rs", "renamed file\nname.rs"])

    def test_pr_uses_merge_base_not_base_tip(self):
        ancestor = self.git("rev-parse", "HEAD")
        self.git("checkout", "-b", "feature")
        self.write("feature.rs", "feature")
        head = self.commit()
        self.git("checkout", "main")
        self.write("main.rs", "main")
        self.commit()
        self.git("checkout", "feature")
        base, actual_head, paths = affected.changes(self.root, options(base="main", head="feature"))
        self.assertEqual((base, actual_head, paths), (ancestor, head, ["feature.rs"]))

    def test_worktree_includes_staged_unstaged_untracked_and_deleted_paths(self):
        self.write("staged.rs", "staged")
        self.git("add", "staged.rs")
        self.write("new.rs", "new")
        (self.root / "original.rs").unlink()
        _, _, paths = affected.changes(self.root, options(worktree=True))
        self.assertEqual(paths, ["new.rs", "original.rs", "staged.rs"])

    def test_wrong_checkout_dirty_tree_and_missing_base_fail(self):
        old = self.git("rev-parse", "HEAD")
        self.write("new.rs", "new")
        self.commit()
        with self.assertRaisesRegex(affected.AnalysisError, "Check out"):
            affected.changes(self.root, options(commit=old))
        with self.assertRaises(affected.AnalysisError):
            affected.changes(self.root, options(base="missing-ref"))
        self.write("new.rs", "modified")
        with self.assertRaisesRegex(affected.AnalysisError, "modified"):
            affected.changes(self.root, options(commit="HEAD"))

    def test_untracked_sources_cannot_contaminate_commit_graph(self):
        self.write("src/bin/untracked.rs", "fn main() {}")
        with self.assertRaisesRegex(affected.AnalysisError, "untracked"):
            affected.changes(self.root, options(commit="HEAD"))

    def test_merge_commit_uses_first_parent(self):
        self.git("checkout", "-b", "feature")
        self.write("feature.rs", "feature")
        self.commit()
        self.git("checkout", "main")
        self.write("main.rs", "main")
        parent = self.commit()
        self.git("merge", "--no-ff", "feature", "-m", "test(ci): merge fixture")
        base, _, paths = affected.changes(self.root, options(commit="HEAD"))
        self.assertEqual((base, paths), (parent, ["feature.rs"]))


class ProtocolTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()

    def server(self, body):
        path = self.root / "fake-analyzer"
        path.write_text(f"#!{sys.executable}\n" + textwrap.dedent(body))
        path.chmod(0o755)
        return str(path)

    def test_protocol_waits_for_ready_and_services_requests_before_reading_graph(self):
        server = self.server('''
            import json, sys
            def read():
                headers = {}
                while (line := sys.stdin.buffer.readline()) != b"\\r\\n":
                    key, value = line.split(b":", 1)
                    headers[key] = value.strip()
                return json.loads(sys.stdin.buffer.read(int(headers[b"Content-Length"])))
            def send(message):
                data = json.dumps({"jsonrpc": "2.0", **message}).encode()
                sys.stdout.buffer.write(f"Content-Length: {len(data)}\\r\\n\\r\\n".encode() + data)
                sys.stdout.buffer.flush()
            init = read()
            assert init["params"]["initializationOptions"]["cargo"]["features"] == "all"
            assert init["params"]["initializationOptions"]["cargo"]["target"] == "example-target"
            send({"id": init["id"], "result": {"capabilities": {}}})
            assert read()["method"] == "initialized"
            send({"method": "experimental/serverStatus", "params": {"health": "ok", "quiescent": False}})
            send({"id": 40, "method": "window/workDoneProgress/create", "params": {"token": "load"}})
            assert read()["id"] == 40
            send({"method": "experimental/serverStatus", "params": {"health": "ok", "quiescent": True}})
            graph = read()
            assert graph["method"] == "rust-analyzer/viewCrateGraph"
            send({"id": graph["id"], "result": "graph"})
            shutdown = read()
            assert shutdown["method"] == "shutdown"
            send({"id": shutdown["id"], "result": None})
            assert read()["method"] == "exit"
        ''')
        self.assertEqual(asyncio.run(affected.analyzer_graph(self.root, server, 5, "example-target")), "graph")

    def test_timeout_and_crashed_server_raise_instead_of_hanging(self):
        for body in ["import time; time.sleep(30)", "import sys; sys.stderr.write('load failed'); sys.exit(1)"]:
            with self.subTest(body=body):
                server = self.server(body)
                with self.assertRaises(affected.AnalysisError):
                    asyncio.run(affected.analyzer_graph(self.root, server, 0.1, None))


if __name__ == "__main__":
    unittest.main()
