import json
from pathlib import Path
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


class Signature(unittest.TestCase):
    def test_signature_is_normalized_and_scoped(self):
        source = {"head": "0" * 40, "tree": "1" * 40, "dirty": False,
                  "initial_profiles": [], "input_environment": {"RUSTFLAGS": "-Cinstrument-coverage"},
                  "toolchain": "rustc 1.99.0", "features": "all", "target": None,
                  "profile": "ci"}
        signature = json.loads(coverage_graph.execution_signature(source))
        self.assertEqual(sorted(signature),
                         ["environment", "features", "profile", "target", "toolchain"])
        self.assertEqual(signature["environment"], {"RUSTFLAGS": "-Cinstrument-coverage"})
        self.assertEqual(signature, json.loads(coverage_graph.execution_signature(dict(source))))


if __name__ == "__main__":
    unittest.main()
