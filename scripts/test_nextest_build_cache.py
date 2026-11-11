import importlib.util
import json
import os
from pathlib import Path
import re
import tempfile
import tomllib
import unittest
from unittest.mock import patch


spec = importlib.util.spec_from_file_location(
    "nextest_build_cache", Path(__file__).with_name("nextest-build-cache.py")
)
cache = importlib.util.module_from_spec(spec)
spec.loader.exec_module(cache)


class NextestConfigTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.root = Path(__file__).resolve().parent.parent
        cls.config = tomllib.loads((cls.root / ".config/nextest.toml").read_text())
        profile = cls.config["profile"]["default"]
        cls.setup, = [rule for rule in profile["scripts"]
                      if rule.get("setup") == "build-cache"]
        cls.group, = [rule for rule in profile["overrides"]
                      if rule.get("test-group") == "shared-cargo"]

    def test_cache_setup_and_serialization_select_the_same_tests(self):
        self.assertEqual(self.setup["filter"], self.group["filter"])
        self.assertEqual(self.setup["platform"], self.group["platform"])
        self.assertEqual(self.config["test-groups"]["shared-cargo"]["max-threads"], 1)
        self.assertEqual(self.config["profile"]["ci"]["inherits"], "default")

    def test_build_oriented_targets_cache_new_and_renamed_cases(self):
        for binary in [
            "mf-compiler::custom_flow_runner",
            "mf-compiler::flow_argument_stream_codegen",
            "mf-compiler::flow_parameter_discovery",
            "mf-compiler::flow_value_runner",
            "mf-compiler::deepseek_workflows",
            "mf-compiler::llm_workflows",
        ]:
            with self.subTest(binary=binary):
                self.assertRegex(
                    self.setup["filter"], rf"(?m)^binary_id\(={re.escape(binary)}\)\s*\|"
                )

    def test_provider_probes_and_cli_validation_failures_use_the_cache(self):
        self.assertIn("(binary_id(=mf-compiler) & test(pipeline::tests::))",
                      self.setup["filter"])
        cli = self.setup["filter"].split("(binary_id(=mf-cli::compile) & (", 1)[1]
        cli = cli.split("))", 1)[0]
        for name in [
            "cyclic_workflow_reports_the_cycle_path",
            "rejects_redirected_cache_directories_without_mutating_inputs",
        ]:
            with self.subTest(test=name):
                self.assertIn(f"test(={name})", cli)

    def test_exact_cache_selectors_have_matching_source_functions(self):
        workspace = tomllib.loads((self.root / "Cargo.toml").read_text())["workspace"]
        packages = {}
        for pattern in workspace["members"]:
            for directory in self.root.glob(pattern):
                manifest = tomllib.loads((directory / "Cargo.toml").read_text())
                packages[manifest["package"]["name"]] = directory
        # Cheap source-level rename guard, not test discovery or a filter parser.
        # A stale clause silently disables cache reuse when a test is renamed.
        clauses = re.findall(r"binary_id\(=([^)]+)\)(.*?)(?=binary_id\(|\Z)",
                             self.setup["filter"], re.DOTALL)
        for binary, clause in clauses:
            package, _, target = binary.partition("::")
            directory = packages[package]
            if target:
                source = (directory / "tests" / f"{target}.rs").read_text()
            else:
                source = "\n".join(path.read_text() for path in (directory / "src").rglob("*.rs"))
            for name in re.findall(r"test\(=([^)]+)\)", clause):
                with self.subTest(binary=binary, test=name):
                    function = re.escape(name.rsplit("::", 1)[-1])
                    self.assertRegex(source, rf"\bfn\s+{function}\s*\(")


class BuildCacheTests(unittest.TestCase):
    def test_isolated_setup_prepares_sources_without_serial_compilation(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            metadata = {"packages": [{
                "name": "mf-compiler",
                "dependencies": [{"name": "serde_json", "req": "^1", "kind": None}],
            }]}
            with patch.object(cache.subprocess, "run") as run:
                cache.prepare_cache(root, "/bin/cargo", metadata, root / "cache", True,
                                    prewarm=False)
            commands = [call.args[0][1] for call in run.call_args_list]
            self.assertIn("vendor", commands)
            self.assertIn("clean", commands)
            self.assertNotIn("build", commands)
            self.assertTrue((root / "cache/target/CACHEDIR.TAG").is_file())
            self.assertTrue((root / "cache/source.toml").is_file())

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

            def prepare(workspace, cargo, metadata, target, persistent, prewarm=True):
                self.assertFalse(prewarm)
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

            def prepare(*args, **kwargs):
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
