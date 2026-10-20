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
        }} for package in ["full", "partial", "mfn-lsp"]
    }}


def case(package, name):
    return {"package": package, "binary": package, "name": name}


class SelectionTests(unittest.TestCase):
    def test_mixed_selection_and_inventory_proven_complete_crates(self):
        plan = {"baseline_packages": ["full", "partial", "mfn-lsp"], "packages": ["full"],
                "tests": [case("partial", "changed"), case("mfn-lsp", "changed"), case("mfn-lsp", "stable")]}
        result = selection.resolve(plan, inventory())
        self.assertEqual(result["complete_packages"], ["full"])
        self.assertEqual(result["partial_packages"], ["mfn-lsp", "partial"])
        self.assertIn("test(=changed)", result["filters"]["partial"])
        plan["tests"] += [case("partial", "stable"), case("mfn-lsp", "smoke")]
        result = selection.resolve(plan, inventory())
        self.assertEqual(result["complete_packages"], ["full", "mfn-lsp", "partial"])
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
