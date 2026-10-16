import json
import os
import re
from pathlib import Path
import shutil
import subprocess
import tempfile
import tomllib
import unittest


class CoverageTests(unittest.TestCase):
    def test_workflows_cover_every_workspace_package_and_skip_unselected_steps(self):
        root = Path(__file__).resolve().parent.parent
        workspace = tomllib.loads((root / "Cargo.toml").read_text())["workspace"]
        packages = {
            tomllib.loads(manifest.read_text())["package"]["name"]
            for member in workspace["members"]
            for manifest in root.glob(f"{member}/Cargo.toml")
        }
        for filename in ["CI.yaml", "Nightly.yaml"]:
            with self.subTest(workflow=filename):
                workflow = (root / ".github/workflows" / filename).read_text()
                steps = re.findall(
                    r"      - name: Coverage tests \(([^)]+)\)\n(.*?)(?=      - name:)",
                    workflow, re.DOTALL,
                )
                self.assertEqual(sorted(name for name, _ in steps), sorted(packages))
                for name, body in steps:
                    self.assertIn(f"scripts/run-cov.sh --test-package {name}\n", body)
                    if filename == "CI.yaml":
                        self.assertIn(
                            "if: github.event_name != 'pull_request' || "
                            f"contains(fromJSON(steps.selection.outputs.packages || '[]'), '{name}')",
                            body,
                        )

    def test_full_selected_empty_and_failed_test_reports(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            env = dict(os.environ)
            for name in [
                "CARGO", "CARGO_TARGET_DIR", "CARGO_ENCODED_RUSTFLAGS", "CARGO_BUILD_TARGET",
                "MF_TEST_SOURCE_CONFIG", "MF_TEST_TARGET_DIR", "RUSTFLAGS", "LLVM_PROFILE_FILE",
                "NEXTEST_PROFILE", "MF_TEST_BUILD_CACHE_SESSION",
            ]:
                env.pop(name, None)
            env["MF_TEST_BUILD_CACHE_DIR"] = str(root / "target/fixture-cache")
            env["MF_TEST_BUILD_CACHE_SESSION"] = "inherited-session"

            def execute(*args):
                return subprocess.run(args, cwd=root, env=env, capture_output=True, text=True)

            (root / "src").mkdir()
            (root / "scripts").mkdir()
            (root / ".config").mkdir()
            (root / ".config/nextest.toml").write_text('''experimental = ["setup-scripts"]
[scripts.setup.check-session]
command = ["python3", "scripts/check-session.py"]
[[profile.default.scripts]]
filter = "all()"
setup = "check-session"
''')
            (root / "scripts/check-session.py").write_text('''import os
from pathlib import Path
session = os.environ["MF_TEST_BUILD_CACHE_SESSION"]
assert session != "inherited-session"
assert session == Path("target/coverage/build-cache-session").read_text().strip()
''')
            shutil.copyfile(Path(__file__).resolve().parent / "run-cov.sh", root / "scripts/run-cov.sh")
            (root / "Cargo.toml").write_text('''[package]
name = "coverage-fixture"
version = "0.1.0"
edition = "2024"

[workspace]
members = ["other", "empty"]
''')
            (root / "empty/src").mkdir(parents=True)
            (root / "empty/Cargo.toml").write_text('''[package]
name = "empty-fixture"
version = "0.1.0"
edition = "2024"
''')
            (root / "empty/src/lib.rs").write_text("pub fn empty_value() -> u32 { 0 }\n")
            (root / "other/src").mkdir(parents=True)
            (root / "other/Cargo.toml").write_text('''[package]
name = "unselected-fixture"
version = "0.1.0"
edition = "2024"
''')
            other_source = root / "other/src/lib.rs"
            other_source.write_text('''pub fn other_value() -> u32 { 3 }
#[test]
fn other_matches() { assert_eq!(other_value(), 3); }
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
            result = execute("cargo", "generate-lockfile", "--offline")
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            result = execute("bash", "scripts/run-cov.sh")
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            report = root / "target/coverage/result/lcov.info"
            blocks = report.read_text().split("end_of_record")
            for module in ["alpha", "beta"]:
                block = next(block for block in blocks if any(
                    line.startswith("SF:") and line.endswith(f"src/{module}.rs")
                    for line in block.splitlines()
                ))
                self.assertTrue(any(
                    line.startswith("DA:") and int(line.split(",")[1]) > 0
                    for line in block.splitlines()
                ), block)
            self.assertGreater((root / "target/coverage/result/cobertura.xml").stat().st_size, 0)
            self.assertIn("other/src/lib.rs", report.read_text())

            # Separate steps must accumulate profiles without cleaning earlier crates.
            result = execute("bash", "scripts/run-cov.sh", "--prepare")
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertFalse(report.exists())
            session = root / "target/coverage/build-cache-session"
            prepared_session = session.read_text()
            self.assertTrue(prepared_session.strip())
            # Package steps must reuse the shared build, never compile again.
            original_source = other_source.read_text()
            other_source.write_text('compile_error!("Package steps must reuse binaries");\n')
            for package in ["coverage-fixture", "unselected-fixture", "empty-fixture"]:
                result = execute("bash", "scripts/run-cov.sh", "--test-package", package)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertFalse(report.exists())
            self.assertEqual(session.read_text(), prepared_session)
            other_source.write_text(original_source)
            result = execute("bash", "scripts/run-cov.sh", "--report")
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            blocks = report.read_text().split("end_of_record")
            for filename in ["src/alpha.rs", "src/beta.rs", "other/src/lib.rs"]:
                block = next(block for block in blocks if any(
                    line.startswith("SF:") and line.endswith(filename)
                    for line in block.splitlines()
                ))
                self.assertTrue(any(
                    line.startswith("DA:") and int(line.split(",")[1]) > 0
                    for line in block.splitlines()
                ), block)

            # A package outside the plan must not even compile, and its old
            # coverage mappings must not leak from the preceding full run.
            other_source.write_text('compile_error!("Unselected package must not compile");\n')
            plan = root / "plan.json"
            plan.write_text(json.dumps({"packages": ["coverage-fixture"]}))
            result = execute("bash", "scripts/run-cov.sh", "--affected-plan", str(plan))
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertNotEqual(session.read_text(), prepared_session)
            self.assertNotIn("other/src/lib.rs", report.read_text())
            self.assertIn("src/alpha.rs", report.read_text())

            # Skipping the other package in the phased path must also omit its mappings.
            for args in [("--prepare", "--affected-plan", str(plan)), ("--test-package", "coverage-fixture"), ("--report",)]:
                result = execute("bash", "scripts/run-cov.sh", *args)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertNotIn("other/src/lib.rs", report.read_text())
            self.assertIn("src/alpha.rs", report.read_text())

            source = root / "src/alpha.rs"
            source.write_text(source.read_text().replace("    1", "    99"))
            result = execute("bash", "scripts/run-cov.sh", "--affected-plan", str(plan), "-E", "test(alpha::tests::value_matches)")
            self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertIn("alpha::tests::value_matches", result.stdout + result.stderr)
            self.assertFalse(report.exists())

            result = execute("bash", "scripts/run-cov.sh", "--prepare", "--affected-plan", str(plan))
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            result = execute("bash", "scripts/run-cov.sh", "--test-package", "coverage-fixture")
            self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertFalse(report.exists())

            plan.write_text(json.dumps({"packages": []}))
            for phase in [(), ("--prepare",)]:
                report.write_text("stale coverage")
                result = execute("bash", "scripts/run-cov.sh", *phase, "--affected-plan", str(plan))
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertIn("No affected Rust package tests", result.stdout)
                self.assertFalse(report.exists())
                self.assertFalse((root / "target/coverage/binaries.json").exists())

            plan.write_text("{}")
            result = execute("bash", "scripts/run-cov.sh", "--affected-plan", str(plan))
            self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertFalse(report.exists())


if __name__ == "__main__":
    unittest.main()
