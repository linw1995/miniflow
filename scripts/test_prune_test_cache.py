import importlib.util
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch


spec = importlib.util.spec_from_file_location(
    "prune_test_cache", Path(__file__).with_name("prune-test-cache.py")
)
prune = importlib.util.module_from_spec(spec)
spec.loader.exec_module(prune)


class PruneTests(unittest.TestCase):
    def test_keeps_dependencies_and_build_scripts_but_removes_temporary_runners(self):
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory)
            keep = [
                "release/deps/libdependency.rlib", "release/deps/libmacro.so",
                "release/build/dependency/build-script-build", "debug/deps/libmacro.dylib",
                "release/.cargo-lock", "release/deps/metadata",
            ]
            remove = ["release/mf-generated-workflow", "release/deps/runner-hash", "debug/deps/test.exe"]
            for name in keep + remove:
                path = target / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(b"fixture")
                path.chmod(0o644 if name.endswith("metadata") else 0o755)
            with patch.object(prune, "clean_workspace") as run:
                prune.prune_target(target)
            self.assertEqual(run.call_args.args[-1], ["dev", "release"])
            self.assertTrue(all(path.is_file() for path in (target / name for name in keep)))
            self.assertFalse(any(path.exists() for path in (target / name for name in remove)))
            with patch.object(prune, "clean_workspace") as run:
                prune.prune_target(target / "missing")
            run.assert_not_called()

    def test_clean_failure_is_propagated_before_removing_executables(self):
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory)
            runner = target / "release/runner"
            runner.parent.mkdir()
            runner.write_bytes(b"fixture")
            runner.chmod(0o755)
            with patch.object(prune, "clean_workspace", side_effect=subprocess.CalledProcessError(1, "cargo")):
                with self.assertRaises(subprocess.CalledProcessError):
                    prune.prune_target(target)
            self.assertTrue(runner.is_file())


if __name__ == "__main__":
    unittest.main()
