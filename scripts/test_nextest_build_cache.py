import importlib.util
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch


spec = importlib.util.spec_from_file_location(
    "nextest_build_cache", Path(__file__).with_name("nextest-build-cache.py")
)
cache = importlib.util.module_from_spec(spec)
spec.loader.exec_module(cache)


class BuildCacheTests(unittest.TestCase):
    def test_optimized_setup_isolates_targets_sources_and_session(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            env = {
                "NEXTEST_WORKSPACE_ROOT": str(root),
                "NEXTEST_ENV": str(root / "nextest.env"),
                "NEXTEST_RUN_ID": "run",
                "MF_TEST_BUILD_CACHE_DIR": str(root / "cache"),
                "MF_TEST_BUILD_CACHE_SESSION": "session",
            }

            def prepare(workspace, cargo, metadata, target, persistent):
                (target / "target").mkdir(parents=True)
                (target / "vendor").mkdir()
                (target / "source.toml").write_text("fixture")

            with (
                patch.dict(os.environ, env, clear=True),
                patch.object(cache.shutil, "which", return_value="/bin/cargo"),
                patch.object(cache.subprocess, "check_output", return_value=b"{}"),
                patch.object(cache, "prepare_cache", side_effect=prepare),
            ):
                cache.main(optimized=True)
                cache.main(optimized=True)
            exported = (root / "nextest.env").read_text()
            for name, value in {
                "MF_TEST_TARGET_DIR": root / "cache/optimized/target",
                "MF_TEST_VENDOR_DIR": root / "cache/optimized/vendor",
                "CARGO_PROFILE_RELEASE_OPT_LEVEL": "3",
                "CARGO_PROFILE_RELEASE_LTO": "true",
                "CARGO_PROFILE_RELEASE_CODEGEN_UNITS": "1",
            }.items():
                self.assertIn(f"{name}={value}\n", exported)
            self.assertFalse((root / "cache/prepared-session.json").exists())
            self.assertTrue((root / "cache/optimized/prepared-session.json").exists())

    def test_persistent_preparation_cleans_checkout_packages_in_both_targets(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            package_target = root / "cache/packaged-target"
            package_target.mkdir(parents=True)
            obsolete = root / "cache/target/debug"
            obsolete.mkdir(parents=True)
            (obsolete / "dependency.rlib").write_text("old profile")
            metadata = {"packages": [{
                "name": "mf-compiler",
                "dependencies": [{"name": "serde_json", "req": "^1", "kind": None}],
            }]}
            with patch.object(cache.subprocess, "run") as run:
                cache.prepare_cache(root, "/bin/cargo", metadata, root / "cache", True)
            clean = [call for call in run.call_args_list if call.args[0][1] == "clean"]
            self.assertEqual([
                (call.kwargs["env"]["CARGO_TARGET_DIR"], call.args[0][-1]) for call in clean
            ], [
                (str(root / "cache/target"), "release"),
                (str(package_target), "release"),
            ])
            self.assertTrue(all("--workspace" in call.args[0] for call in clean))
            self.assertFalse(obsolete.exists())

    def test_session_reuse_invalidation_and_failed_preparation(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            target = root / "cache"
            env_file = root / "nextest.env"
            env = {
                "NEXTEST_WORKSPACE_ROOT": str(root),
                "NEXTEST_ENV": str(env_file),
                "NEXTEST_RUN_ID": "first-run",
                "MF_TEST_BUILD_CACHE_DIR": str(target),
                "MF_TEST_BUILD_CACHE_SESSION": "first-session",
            }

            def prepare(*args):
                (target / "target").mkdir(parents=True, exist_ok=True)
                (target / "vendor").mkdir(exist_ok=True)
                (target / "source.toml").write_text("fixture")

            with (
                patch.dict(os.environ, env, clear=True),
                patch.object(cache.shutil, "which", return_value="/bin/cargo"),
                patch.object(cache.subprocess, "check_output", return_value=b"{}"),
                patch.object(cache, "prepare_cache", side_effect=prepare) as preparation,
            ):
                cache.main()
                first_env = env_file.read_text()
                self.assertIn("CARGO_PROFILE_RELEASE_OPT_LEVEL=0\n", first_env)
                env_file.write_text("")
                os.environ["NEXTEST_RUN_ID"] = "second-package"
                cache.main()
                self.assertEqual(preparation.call_count, 1)
                self.assertEqual(env_file.read_text(), first_env)

                os.environ["MF_TEST_BUILD_CACHE_SESSION"] = "second-session"
                cache.main()
                self.assertEqual(preparation.call_count, 2)
                os.environ["RUSTFLAGS"] = "-Cinstrument-coverage"
                cache.main()
                self.assertEqual(preparation.call_count, 3)
                (target / "source.toml").unlink()
                cache.main()
                self.assertEqual(preparation.call_count, 4)

                os.environ["MF_TEST_BUILD_CACHE_SESSION"] = "failed-session"
                preparation.side_effect = RuntimeError("build failed")
                with self.assertRaisesRegex(RuntimeError, "build failed"):
                    cache.main()
                self.assertFalse((target / "prepared-session.json").exists())
                preparation.side_effect = prepare
                cache.main()
                self.assertEqual(preparation.call_count, 6)
                self.assertEqual(
                    json.loads((target / "prepared-session.json").read_text())["session"],
                    "failed-session",
                )

                del os.environ["MF_TEST_BUILD_CACHE_SESSION"]
                cache.main()
                cache.main()
                self.assertEqual(preparation.call_count, 8)
                self.assertFalse((target / "prepared-session.json").exists())
                os.environ["CARGO_PROFILE_RELEASE_OPT_LEVEL"] = "3"
                env_file.write_text("")
                cache.main()
                self.assertIn("CARGO_PROFILE_RELEASE_OPT_LEVEL=3\n", env_file.read_text())


if __name__ == "__main__":
    unittest.main()
