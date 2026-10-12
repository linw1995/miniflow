from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import run_affected_tests as affected


class AffectedTests(unittest.TestCase):
    def test_rust_sources_and_tests_can_use_coverage(self):
        self.assertFalse(affected.requires_full_suite([
            "crates/mf-runtime/src/dispatch.rs", "crates/mf-cli/tests/compile.rs",
        ]))

    def test_build_inputs_and_proc_macros_run_full_suite(self):
        for path in [
            "Cargo.lock", "Cargo.toml", "rust-toolchain.toml", ".cargo/config.toml",
            "crates/new-node/build.rs", "crates/mf-runtime-derive/src/lib.rs",
            "scripts/nextest-build-cache.py", ".config/nextest.toml",
            "examples/stream.json", "crates/mf-tui/tests/snapshots/graph.snap", "new-fixture.data",
            "crates/mf-compiler/tests/fixtures/multi-nodes/src/typed_fixture.rs",
            "crates/mf-compiler/tests/fixtures/controlled_source.rs",
        ]:
            with self.subTest(path=path):
                self.assertTrue(affected.requires_full_suite([path]))

    def test_missing_baseline_runs_full_suite(self):
        with tempfile.TemporaryDirectory() as directory:
            self.assertTrue(affected.use_full_suite(Path(directory) / "baseline.sha"))

    def test_missing_git_anchor_runs_full_suite(self):
        with tempfile.TemporaryDirectory() as directory:
            baseline = Path(directory) / "baseline.sha"
            baseline.write_text("missing\n")
            with patch.object(affected.subprocess, "run", return_value=subprocess.CompletedProcess([], 128)):
                self.assertTrue(affected.use_full_suite(baseline))

    def test_deleted_non_rust_inputs_are_checked_against_collection_anchor(self):
        with tempfile.TemporaryDirectory() as directory:
            baseline = Path(directory) / "baseline.sha"
            baseline.write_text("collect-sha\n")
            result = subprocess.CompletedProcess([], 0, b"deleted fixture.json\0")
            with patch.object(affected.subprocess, "run", return_value=result) as git:
                self.assertTrue(affected.use_full_suite(baseline))
                self.assertEqual(git.call_args.args[0], [
                    "git", "diff", "--name-only", "--no-renames", "-z", "collect-sha", "--",
                ])


if __name__ == "__main__":
    unittest.main()
