import json
import os
import re
from pathlib import Path
import shutil
import subprocess
import tempfile
import tomllib
import unittest

import pr_validation


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
                self.assertLess(workflow.index("name: Generate coverage reports"),
                                workflow.index("name: Prune coverage build cache"))
                self.assertLess(workflow.index("name: Prune coverage build cache"),
                                workflow.index("name: Upload coverage artifact"))
                for name, body in steps:
                    suffix = " --run-ignored all" if name == "mfn-lsp" else ""
                    self.assertIn(f"scripts/run-cov.sh --test-package {name}{suffix}\n", body)
                    if name == "mfn-lsp":
                        self.assertIn("env MF_LSP_SMOKE_SERVER=rust-analyzer", body)
                    if filename == "CI.yaml":
                        self.assertIn(
                            "if: steps.selection.outputs.scoped != 'true' || "
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
            for script in ["run-cov.sh", "prune-test-cache.py", "cargo_cache.py"]:
                shutil.copyfile(Path(__file__).resolve().parent / script, root / "scripts" / script)
            (root / "Cargo.toml").write_text('''[package]
name = "coverage-fixture"
version = "0.1.0"
edition = "2024"

[workspace]
members = ["other", "empty"]
exclude = ["dependency"]

[dependencies]
coverage-dependency = { path = "dependency" }
''')
            (root / "dependency/src").mkdir(parents=True)
            (root / "dependency/Cargo.toml").write_text('''[package]
name = "coverage-dependency"
version = "0.1.0"
edition = "2024"
''')
            (root / "dependency/src/lib.rs").write_text("pub fn value() -> u32 { 1 }\n")
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
    #[test]
    #[ignore]
    fn explicit_check() {{
        assert_eq!(super::value(), {value});
    }}
}}
''')
            result = execute("cargo", "generate-lockfile", "--offline")
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            def git(*args):
                result = execute("git", *args)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                return result.stdout.strip()

            git("init", "-q", "-b", "main")
            for key, value in [("user.name", "Fixture"), ("user.email", "fixture@example.com"),
                               ("commit.gpgsign", "false"), ("core.hooksPath", "/dev/null")]:
                git("config", key, value)
            (root / ".gitignore").write_text("/target/\n/plan.json\n")
            git("add", ".")
            git("commit", "-qm", "test(ci): coverage base")
            base = git("rev-parse", "HEAD")
            alpha = root / "src/alpha.rs"
            alpha.write_text(alpha.read_text().replace("    1\n", "    1 + 0\n"))
            git("add", ".")
            git("commit", "-qm", "test(ci): executable increment")
            head = git("rev-parse", "HEAD")
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
            provenance = json.loads(report.with_name("source.json").read_text())
            context = {"merge": head, "tree": git("rev-parse", "HEAD^{tree}"),
                       "signature": {key: provenance[key] for key in ("toolchain", "target", "profile")}}
            context["signature"]["coverage_target"] = 95
            context["signature"]["environment"] = provenance["input_environment"]
            comparison = {"base": base, "head": head}
            plan = comparison | {"comparison_mode": "snapshot", "packages": ["coverage-fixture"], "ignored_files": []}
            coverage = pr_validation.evaluate_coverage(root, context, comparison, plan, report)
            self.assertEqual(coverage["status"], "passed")
            self.assertGreater(coverage["total"], 0)
            self.assertEqual(coverage["covered"], coverage["total"])

            dependencies = list((root / "target/coverage/build/debug/deps").glob("libcoverage_dependency-*.rlib"))
            self.assertTrue(dependencies)
            stale_binary = root / "target/stale-binary"
            binary = next(path for path in (root / "target/coverage/build/debug/deps").glob("coverage_fixture-*")
                          if path.is_file() and not path.suffix)
            shutil.copyfile(binary, stale_binary)
            before_pruning = report.read_bytes()
            result = execute("bash", "scripts/run-cov.sh", "--prune-cache")
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertEqual(report.read_bytes(), before_pruning)
            self.assertTrue(all(path.is_file() for path in dependencies))
            self.assertFalse(list((root / "target/coverage/build/debug/deps").glob("libcoverage_fixture-*.rlib")))

            # A compiler-cache backup is not a current published Cargo artifact.
            backup = root / "target/coverage/build/debug/deps/.kache-prestage-fixture"
            shutil.copyfile(stale_binary, backup)
            alpha.write_text("// Shift current executable lines.\n" * 20 + alpha.read_text())
            old_profile = root / "default_123_0_456.profraw"
            old_profile.write_text("Profiles from an earlier session must not be consumed.")
            # Separate steps must accumulate profiles without cleaning earlier crates.
            result = execute("bash", "scripts/run-cov.sh", "--prepare")
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertFalse(report.exists())
            result = execute("bash", "scripts/run-cov.sh", "--prune-cache")
            self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
            session = root / "target/coverage/build-cache-session"
            prepared_session = session.read_text()
            self.assertTrue(prepared_session.strip())
            isolated_env = dict(env)
            isolated_env.pop("LLVM_PROFILE_FILE", None)
            result = subprocess.run([str(binary), "--exact", "alpha::tests::value_matches"],
                                    cwd=root, env=isolated_env, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertGreater(len(list(root.glob("default*.profraw"))), 1)
            # Package steps must reuse the shared build, never compile again.
            original_source = other_source.read_text()
            other_source.write_text('compile_error!("Package steps must reuse binaries");\n')
            for package in ["coverage-fixture", "unselected-fixture", "empty-fixture"]:
                result = execute("bash", "scripts/run-cov.sh", "--test-package", package)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertFalse(report.exists())
            result = execute("bash", "scripts/run-cov.sh", "--test-package", "coverage-fixture", "--run-ignored", "only")
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertIn("explicit_check", result.stdout + result.stderr)
            self.assertIn("2 passed", result.stdout + result.stderr)
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

            self.assertTrue(backup.exists())
            alpha_block = next(block for block in blocks if any(
                line.startswith("SF:") and line.endswith("src/alpha.rs") for line in block.splitlines()))
            self.assertTrue(all(int(line[3:].split(",")[0]) > 20
                                for line in alpha_block.splitlines() if line.startswith("DA:")))
            self.assertFalse((root / "target/coverage/report-binaries").exists())
            self.assertEqual(list(root.glob("default*.profraw")), [old_profile])
            self.assertTrue(list((root / "target/coverage/data").glob("subprocess-*.profraw")))
            old_profile.unlink()

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
