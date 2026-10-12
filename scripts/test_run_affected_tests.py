from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import run_affected_tests as affected


class AffectedTests(unittest.TestCase):
    def test_documentation_does_not_force_full_suite(self):
        self.assertFalse(affected.requires_full_suite([
            "AGENTS.md", "README.md", "docs/ci.md",
            "openspec/changes/archive/example/design.md",
            "openspec/changes/archive/example/.openspec.yaml",
            "crates/mf-tui/src/graph.rs",
        ]))

    def test_markdown_fixture_and_unknown_documentation_still_force_full_suite(self):
        for path in ["crates/mf-cli/tests/fixtures/input.md", "unknown/readme.md", "docs/input.json"]:
            with self.subTest(path=path):
                self.assertTrue(affected.requires_full_suite([path]))

    def test_rust_sources_and_tests_can_use_coverage(self):
        self.assertFalse(affected.requires_full_suite([
            "crates/mf-runtime/src/dispatch.rs", "crates/mf-cli/tests/compile.rs",
        ]))

    def test_build_inputs_and_proc_macros_run_full_suite(self):
        for path in [
            "Cargo.lock", "Cargo.toml", "rust-toolchain.toml", ".cargo/config.toml",
            "crates/new-node/build.rs", "crates/mf-runtime-derive/src/lib.rs",
            "scripts/nextest-build-cache.py", ".config/nextest.toml",
            ".github/workflows/CI.yaml",
        ]:
            with self.subTest(path=path):
                self.assertTrue(affected.requires_full_suite([path], {path}))
        for path in [
            "examples/stream.json", "crates/mf-tui/tests/snapshots/graph.snap", "new-fixture.data",
            "examples/new-project/src/main.rs",
        ]:
            with self.subTest(path=path):
                self.assertTrue(affected.requires_full_suite([path]))

    def test_only_exact_input_rules_can_exempt_external_inputs(self):
        with tempfile.TemporaryDirectory() as directory:
            manifest = Path(directory) / "Cargo.toml"
            manifest.write_text('''[workspace]
[[workspace.metadata.affected.rule]]
globs = ["examples/answer.json", "crates/demo/tests/fixtures/input.rs", "examples/*.json"]
filterset = "kind(test)"
''')
            inputs = affected.mapped_inputs(manifest)
            self.assertFalse(affected.requires_full_suite([
                "examples/answer.json", "crates/demo/tests/fixtures/input.rs",
            ], inputs))
            for path in ["examples/new.json", "examples/*.json", "crates/demo/tests/fixtures/new.rs", "examples/answer.json/child"]:
                with self.subTest(path=path):
                    self.assertTrue(affected.requires_full_suite([path], inputs))

    def test_missing_baseline_runs_full_suite(self):
        with tempfile.TemporaryDirectory() as directory:
            self.assertEqual(affected.full_suite_reason(Path(directory) / "baseline.sha"), "baseline.sha is missing")

    def test_missing_git_anchor_runs_full_suite(self):
        with tempfile.TemporaryDirectory() as directory:
            baseline = Path(directory) / "baseline.sha"
            baseline.write_text("missing\n")
            with patch.object(affected.subprocess, "run", return_value=subprocess.CompletedProcess([], 128)):
                self.assertEqual(affected.full_suite_reason(baseline), "baseline commit is unavailable")

    def test_deleted_non_rust_inputs_are_checked_against_collection_anchor(self):
        with tempfile.TemporaryDirectory() as directory:
            baseline = Path(directory) / "baseline.sha"
            baseline.write_text("collect-sha\n")
            result = subprocess.CompletedProcess([], 0, b"deleted fixture.json\0")
            with patch.object(affected.subprocess, "run", return_value=result) as git:
                self.assertEqual(affected.full_suite_reason(baseline), "changed inputs: deleted fixture.json")
                self.assertEqual(git.call_args.args[0], [
                    "git", "diff", "--name-only", "--no-renames", "-z", "collect-sha", "--",
                ])

if __name__ == "__main__":
    unittest.main()
