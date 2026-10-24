import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
import unittest.mock

import coverage_graph


def inventory():
    return {"rust-suites": {
        "mf-runtime": {"package-name": "mf-runtime", "binary-id": "mf-runtime",
                       "binary-path": "/bin/mf-runtime", "status": "listed",
                       "testcases": {
                           "node::tests::depth": {"filter-match": {"status": "matches"}},
                           "node::tests::skipped": {"filter-match": {"status": "mismatch"}},
                       }},
        "mfn-test-affected::flow": {"package-name": "mfn-test-affected",
                                    "binary-id": "mfn-test-affected::flow",
                                    "binary-path": "/bin/flow", "status": "listed",
                                    "testcases": {
                                        "plans::run": {"filter-match": {"status": "matches"}},
                                    }},
        "mf-cli::bin/mf": {"package-name": "mf-cli", "binary-id": "mf-cli::bin/mf",
                           "binary-path": "/bin/mf", "status": "listed",
                           "testcases": {
                               "tests::parses_run_options": {"filter-match": {"status": "matches"}},
                           }},
    }}


class ProfileGrouping(unittest.TestCase):
    def test_groups_pid_suffixed_profiles_by_test_key(self):
        with tempfile.TemporaryDirectory() as directory:
            profiles = Path(directory)
            for name in ["mf-runtime__node::tests::depth-100.profraw",
                         "mf-runtime__node::tests::depth-101.profraw",
                         "mfn-test-affected::flow__plans::run-102.profraw"]:
                (profiles / name).write_text("")
            groups = coverage_graph.group_profiles(profiles)
            self.assertEqual(sorted(groups), ["mf-runtime__node::tests::depth",
                                              "mfn-test-affected::flow__plans::run"])
            self.assertEqual(len(groups["mf-runtime__node::tests::depth"]), 2)

    def test_rejects_unrecognized_profile_names(self):
        with tempfile.TemporaryDirectory() as directory:
            (Path(directory) / "stray.profraw").write_text("")
            with self.assertRaises(ValueError):
                coverage_graph.group_profiles(Path(directory))

    def test_rejects_profiles_nested_outside_the_flat_namespace(self):
        with tempfile.TemporaryDirectory() as directory:
            nested = Path(directory) / "mf-cli::bin"
            nested.mkdir()
            (nested / "mf__tests::parses_compile_defaults-1.profraw").write_text("")
            with self.assertRaises(ValueError):
                coverage_graph.group_profiles(Path(directory))


class ExportPlanning(unittest.TestCase):
    def test_maps_profiles_to_inventory_identities(self):
        groups = {"mf-runtime__node::tests::depth": [Path("a.profraw")],
                  "mfn-test-affected::flow__plans::run": [Path("b.profraw")],
                  "mf-cli::bin@mf__tests::parses_run_options": [Path("c.profraw")]}
        with unittest.mock.patch.object(Path, "is_file", return_value=True):
            exports = coverage_graph.plan_exports(inventory(), groups)
        self.assertEqual([(e["package"], e["binary"], e["name"]) for e in exports],
                         [("mf-cli", "mf-cli::bin/mf", "tests::parses_run_options"),
                          ("mf-runtime", "mf-runtime", "node::tests::depth"),
                          ("mfn-test-affected", "mfn-test-affected::flow", "plans::run")])

    def test_rejects_profiles_outside_the_inventory(self):
        groups = {"mf-runtime__UNATTRIBUTED": [Path("a.profraw")]}
        with self.assertRaises(ValueError):
            coverage_graph.plan_exports(inventory(), groups)


class ExportExecution(unittest.TestCase):
    def test_compact_export_preserves_workspace_lines_without_dependency_records(self):
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory).resolve()
            root = base / "workspace [coverage]"
            root.mkdir()
            dependency = base / "external.rs"
            dependency.write_text("pub fn value() -> u32 { 7 }\n")
            library = base / "libexternal.rlib"
            source = root / "main.rs"
            source.write_text('''fn main() {
    assert_eq!(external::value(), 7);
    assert_eq!(value(true), 1);
}
fn value(flag: bool) -> u32 {
    if flag { 1 } else { 2 }
}
''')
            binary = root / "fixture"
            common = ["rustc", "--edition=2024", "-Cinstrument-coverage", "-Cdebuginfo=0"]
            subprocess.run(common + [str(dependency), "--crate-name", "external",
                                    "--crate-type=rlib", "-o", str(library)], check=True)
            subprocess.run(common + [source.name, "--extern", f"external={library}",
                                    "-o", str(binary)], cwd=root, check=True)
            profile = base / "fixture.profraw"
            subprocess.run([str(binary)], check=True,
                           env=os.environ | {"LLVM_PROFILE_FILE": str(profile)})
            work = base / "export"
            for name in ["profdata", "lcov"]:
                (work / name).mkdir(parents=True)
            export = {"key": "fixture", "profiles": [profile], "binary_path": binary}
            tools = coverage_graph.llvm_tools()
            coverage_graph.export_test(tools, work, export, root)
            compact = (work / "lcov/fixture.lcov").read_text()
            full = subprocess.check_output([
                str(tools[1]), "export", "--format=lcov", "-object", str(binary),
                f"--instr-profile={work / 'profdata/fixture.profdata'}",
            ], text=True)

            def workspace_lines(report):
                lines = {}
                source = None
                for line in report.splitlines():
                    if line.startswith("SF:"):
                        source = Path(line[3:]).resolve()
                    elif line.startswith("DA:") and source.is_relative_to(root):
                        number, count, *_ = line[3:].split(",")
                        lines[source, int(number)] = int(count)
                return lines

            self.assertTrue(workspace_lines(full))
            self.assertEqual(workspace_lines(compact), workspace_lines(full))
            self.assertIn(str(dependency), full)
            self.assertNotIn(str(dependency), compact)
            self.assertIn("FNDA:", full)
            self.assertNotIn("FNDA:", compact)
            self.assertLess(len(compact), len(full))


class Signature(unittest.TestCase):
    def test_signature_is_normalized_and_scoped(self):
        verbose = ("rustc 1.99.0 (1234abcd 2026-10-21)\nbinary: rustc\ncommit-hash: 1234abcd\n"
                   "commit-date: 2026-10-21\nhost: x86_64-unknown-linux-gnu\nrelease: 1.99.0\n")
        source = {"head": "0" * 40, "tree": "1" * 40, "dirty": False,
                  "initial_profiles": [], "input_environment": {"RUSTFLAGS": "-Cinstrument-coverage"},
                  "toolchain": verbose, "features": "all", "target": None,
                  "profile": "ci"}
        signature = json.loads(coverage_graph.execution_signature(source))
        self.assertEqual(sorted(signature),
                         ["environment", "features", "profile", "target", "toolchain"])
        self.assertEqual(signature["environment"], {"RUSTFLAGS": "-Cinstrument-coverage"})
        self.assertEqual(signature, json.loads(coverage_graph.execution_signature(dict(source))))

    def test_toolchain_identity_ignores_host_and_rejects_incomplete_output(self):
        verbose = ("rustc 1.99.0 (1234abcd 2026-10-21)\nbinary: rustc\ncommit-hash: 1234abcd\n"
                   "commit-date: 2026-10-21\nhost: aarch64-apple-darwin\nrelease: 1.99.0\n"
                   "LLVM version: 21.1.3\n")
        identity = coverage_graph.toolchain_identity(verbose)
        self.assertEqual(identity, "rustc 1.99.0 (1234abcd)")
        # The same release and commit on another host map to one identity.
        linux = verbose.replace("host: aarch64-apple-darwin", "host: x86_64-unknown-linux-gnu")
        self.assertEqual(coverage_graph.toolchain_identity(linux), identity)
        for broken in ["rustc 1.99.0\n", "release: 1.99.0\n", "commit-hash: 1234abcd\n",
                       "release: \ncommit-hash: 1234abcd\n"]:
            with self.assertRaises(ValueError, msg=broken):
                coverage_graph.toolchain_identity(broken)


if __name__ == "__main__":
    unittest.main()
