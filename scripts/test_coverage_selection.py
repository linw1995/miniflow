import json
from pathlib import Path
import tempfile
import unittest

import coverage_selection as selection


def inventory():
    return {"rust-suites": {
        package: {"package-name": package, "testcases": {
            name: {"ignored": ignored, "filter-match": {"status": "matches"}}
            for name, ignored in [("changed", False), ("stable", False), ("smoke", True)]
        }} for package in ["full", "partial", "extra"]
    }}


def case(package, name):
    return {"package": package, "binary": package, "name": name}


class SelectionTests(unittest.TestCase):
    def test_mixed_selection_and_inventory_proven_complete_crates(self):
        plan = {"baseline_packages": ["full", "partial", "extra"], "packages": ["full"],
                "tests": [case("partial", "changed"), case("extra", "changed")]}
        result = selection.resolve(plan, inventory())
        self.assertEqual(result["complete_packages"], ["full"])
        self.assertEqual(result["partial_packages"], ["extra", "partial"])
        self.assertIn("test(=changed)", result["filters"]["partial"])
        plan["tests"] += [case("partial", "stable"), case("extra", "stable")]
        result = selection.resolve(plan, inventory())
        self.assertEqual(result["complete_packages"], ["extra", "full", "partial"])
        self.assertEqual(result["partial_packages"], [])
        self.assertEqual(result["filters"]["partial"], "package(=partial)")

    def test_missing_and_ignored_exact_cases_widen_to_baseline(self):
        for name in ["missing", "smoke"]:
            with self.subTest(name=name):
                result = selection.resolve({"packages": [], "baseline_packages": ["full", "partial"],
                                            "tests": [case("partial", name)]}, inventory())
                self.assertEqual(result["complete_packages"], ["full", "partial"])
                self.assertIsNotNone(result["fallback"])

    def test_filtered_inventory_cannot_certify_an_exact_case(self):
        cases = inventory()
        cases["rust-suites"]["partial"]["testcases"]["changed"]["filter-match"]["status"] = "mismatch"
        result = selection.resolve({"packages": [], "baseline_packages": ["partial"],
                                    "tests": [case("partial", "changed")]}, cases)
        self.assertEqual(result["complete_packages"], ["partial"])
        self.assertIsNotNone(result["fallback"])

    def test_graph_derived_selectors_pass_through_with_provenance(self):
        # Plans carrying coverage-graph provenance keep their exact selectors.
        provenance = {"commit": "a" * 40, "run_id": "7", "attempt": 1, "schema_version": 1,
                      "reason": None, "widened": []}
        plan = {"baseline_packages": ["full", "partial"], "packages": [],
                "tests": [case("partial", "changed")], "coverage_graph": provenance}
        result = selection.resolve(plan, inventory())
        self.assertIsNone(result["fallback"])
        self.assertEqual(result["partial_packages"], ["partial"])
        self.assertIn("test(=changed)", result["filters"]["partial"])
        # Identity mismatches widen to the recorded baseline with a diagnostic.
        plan["tests"] = [case("partial", "missing")]
        result = selection.resolve(plan, inventory())
        self.assertEqual(result["complete_packages"], ["full", "partial"])
        self.assertIsNotNone(result["fallback"])

    def test_reports_exclude_partial_unexecuted_and_dependency_sources(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            directory = root / "result"
            directory.mkdir()
            metadata = {"workspace_members": ["full", "partial", "unexecuted"], "packages": [
                {"id": name, "name": name, "manifest_path": str(root / name / "Cargo.toml")}
                for name in ["full", "partial", "unexecuted"]
            ]}
            metadata["packages"].append({"id": "dependency", "name": "dependency",
                                         "manifest_path": str(root / "full/dependency/Cargo.toml")})
            sources = ["full/src/lib.rs", "partial/src/lib.rs", "unexecuted/src/lib.rs", "full/dependency/src/lib.rs"]
            (directory / "lcov.info").write_text("".join(
                f"SF:{path}\nDA:1,1\nLF:1\nend_of_record\n" for path in sources))
            result = {"complete_packages": ["full", "unexecuted"], "completed": ["full", "partial"]}
            selection.publish(root, directory, result, metadata)
            self.assertEqual([path.name for path in (directory / "crates").iterdir()], ["full.lcov"])
            self.assertEqual((directory / "crates/full.lcov").read_text(),
                             "SF:full/src/lib.rs\nDA:1,1\nLF:1\nend_of_record\n")
            self.assertEqual(json.loads((directory / "selection.json").read_text())["report_packages"], ["full"])
            selection.publish(root, directory, result | {"complete_packages": []}, metadata)
            self.assertEqual(list((directory / "crates").iterdir()), [])


if __name__ == "__main__":
    unittest.main()
