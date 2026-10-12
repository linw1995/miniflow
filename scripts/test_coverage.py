import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


class CoverageTests(unittest.TestCase):
    def test_incremental_reports_full_fallback_and_empty_selection(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            env = dict(os.environ, NEXTEST_PROFILE="ci")
            for name in [
                "CARGO", "CARGO_TARGET_DIR", "CARGO_ENCODED_RUSTFLAGS", "CARGO_BUILD_TARGET",
                "MF_TEST_SOURCE_CONFIG", "MF_TEST_TARGET_DIR", "RUSTFLAGS", "LLVM_PROFILE_FILE",
            ]:
                env.pop(name, None)
            env["MF_TEST_BUILD_CACHE_DIR"] = str(root / "target/coverage/build/nextest-build-cache/ci")

            def execute(*args):
                result = subprocess.run(args, cwd=root, env=env, capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

            def commit(message):
                execute("git", "add", ".")
                execute("git", "-c", "core.hooksPath=/dev/null", "-c", "commit.gpgsign=false",
                        "-c", "user.name=CI Fixture", "-c", "user.email=ci@example.invalid", "commit", "-qm", message)

            def report():
                return json.loads((root / "target/affected/report.json").read_text())

            for name in ["src", "tests", "scripts", ".config"]:
                (root / name).mkdir()
            source_scripts = Path(__file__).resolve().parent
            for name in ["run-cov.sh", "run_affected_tests.py"]:
                shutil.copyfile(source_scripts / name, root / "scripts" / name)
            (root / ".gitignore").write_text("/target\n")
            (root / "Cargo.toml").write_text('''[package]
name = "coverage-fixture"
version = "0.1.0"
edition = "2024"

[workspace]

[[workspace.metadata.affected.rule]]
globs = ["src/**/*.rs"]
filterset = "kind(test)"
''')
            (root / "src/lib.rs").write_text("pub mod alpha;\npub mod beta;\n")
            for module, value in [("alpha", 1), ("beta", 2)]:
                (root / "src" / f"{module}.rs").write_text(f'''pub fn value() -> u32 {{
    {value}
}}

#[cfg(test)]
mod tests {{
    #[test]
    fn value_matches() {{
        assert_eq!(super::value(), {value});
    }}
}}
''')
            (root / "tests/runner.rs").write_text('''#[test]
fn runner_input_rule() {
    assert_eq!(std::env::var("COVERAGE_SETUP").unwrap(), "1");
    assert_eq!(coverage_fixture::beta::value(), 2);
}
''')
            (root / "scripts/setup.py").write_text('''import os
from pathlib import Path
import subprocess

target = Path(os.environ["MF_TEST_BUILD_CACHE_DIR"]) / "target"
subprocess.run(["cargo", "clean", "--workspace", "--profile", "dev", "--target-dir", str(target)], check=True)
with open(os.environ["NEXTEST_ENV"], "a") as output:
    output.write("COVERAGE_SETUP=1\\n")
''')
            (root / ".config/nextest.toml").write_text('''experimental = ["setup-scripts"]

[profile.ci]
inherits = "default"

[scripts.setup.integration-env]
command = ["python3", "scripts/setup.py"]

[[profile.default.scripts]]
filter = "kind(test)"
setup = "integration-env"
''')
            execute("cargo", "generate-lockfile", "--offline")
            execute("git", "init", "-q")
            commit("test(ci): seed coverage fixture")
            execute("bash", "scripts/run-cov.sh", "--collect-affected")

            alpha = root / "src/alpha.rs"
            alpha.write_text(alpha.read_text().replace("    1", "    1 + 0"))
            commit("test(ci): change instrumented source")
            execute("bash", "scripts/run-cov.sh", "--affected")
            self.assertEqual(report()["cache"]["status"], "hit-with-divergence")
            self.assertEqual(report()["selection"]["summary"]["selected"], 2)
            self.assertEqual(report()["selection"]["summary"]["skipped"], 1)
            lcov = root / "target/coverage/result/lcov.info"
            self.assertGreater(lcov.stat().st_size, 0)

            execute("bash", "scripts/run-cov.sh", "--collect-affected")
            execute("bash", "scripts/run-cov.sh", "--affected")
            self.assertEqual(report()["selection"]["summary"]["selected"], 0)
            self.assertFalse(lcov.exists())

            (root / "fixture.json").write_text("{}\n")
            commit("test(ci): change external input")
            execute("bash", "scripts/run-cov.sh", "--affected")
            self.assertEqual(report()["cache"]["status"], "forced-all")
            self.assertGreater(lcov.stat().st_size, 0)
            execute("bash", "scripts/run-cov.sh")
            self.assertGreater(lcov.stat().st_size, 0)


if __name__ == "__main__":
    unittest.main()
