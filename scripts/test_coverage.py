import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


class CoverageTests(unittest.TestCase):
    def test_full_selected_empty_and_failed_test_reports(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            env = dict(os.environ)
            for name in [
                "CARGO", "CARGO_TARGET_DIR", "CARGO_ENCODED_RUSTFLAGS", "CARGO_BUILD_TARGET",
                "MF_TEST_SOURCE_CONFIG", "MF_TEST_TARGET_DIR", "RUSTFLAGS", "LLVM_PROFILE_FILE",
                "NEXTEST_PROFILE",
            ]:
                env.pop(name, None)
            env["MF_TEST_BUILD_CACHE_DIR"] = str(root / "target/fixture-cache")

            def execute(*args):
                return subprocess.run(args, cwd=root, env=env, capture_output=True, text=True)

            (root / "src").mkdir()
            (root / "scripts").mkdir()
            shutil.copyfile(Path(__file__).resolve().parent / "run-cov.sh", root / "scripts/run-cov.sh")
            (root / "Cargo.toml").write_text('''[package]
name = "coverage-fixture"
version = "0.1.0"
edition = "2024"

[workspace]
members = ["other"]
''')
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

            # A package outside the plan must not even compile, and its old
            # coverage mappings must not leak from the preceding full run.
            other_source.write_text('compile_error!("Unselected package must not compile");\n')
            plan = root / "plan.json"
            plan.write_text(json.dumps({"packages": ["coverage-fixture"]}))
            result = execute("bash", "scripts/run-cov.sh", "--affected-plan", str(plan))
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertNotIn("other/src/lib.rs", report.read_text())
            self.assertIn("src/alpha.rs", report.read_text())

            source = root / "src/alpha.rs"
            source.write_text(source.read_text().replace("    1", "    99"))
            result = execute("bash", "scripts/run-cov.sh", "--affected-plan", str(plan), "-E", "test(alpha::tests::value_matches)")
            self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertIn("alpha::tests::value_matches", result.stdout + result.stderr)
            self.assertFalse(report.exists())

            report.write_text("stale coverage")
            plan.write_text(json.dumps({"packages": []}))
            result = execute("bash", "scripts/run-cov.sh", "--affected-plan", str(plan))
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertIn("No affected Rust package tests", result.stdout)
            self.assertFalse(report.exists())

            plan.write_text("{}")
            result = execute("bash", "scripts/run-cov.sh", "--affected-plan", str(plan))
            self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertFalse(report.exists())


if __name__ == "__main__":
    unittest.main()
