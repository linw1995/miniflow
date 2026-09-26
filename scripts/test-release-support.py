#!/usr/bin/env python3
"""Test release prerequisites without contacting or publishing to a registry."""

import json
from pathlib import Path
import runpy
import subprocess
import unittest
from unittest.mock import patch

GATE = runpy.run_path(str(Path(__file__).with_name("check-release-support.py")))


class ReleaseSupportTests(unittest.TestCase):
    def response(self, command, **kwargs):
        if command[1] == "owner":
            return subprocess.CompletedProcess(command, 0, "maintainer (Name)\n", "")
        metadata = {"packages": [{"name": name, "version": "0.1.0", "source": "registry+fixture"} for name in GATE["PACKAGES"]]}
        return subprocess.CompletedProcess(command, 0, json.dumps(metadata), "")

    def test_accepts_owned_available_versions_without_publishing(self):
        with patch("subprocess.run", side_effect=self.response) as run:
            GATE["check"]("maintainer", "0.1.0")
            self.assertTrue(all(call.args[0][1] in ("owner", "metadata") for call in run.call_args_list))

    def test_rejects_wrong_owner(self):
        with patch("subprocess.run", side_effect=self.response):
            with self.assertRaisesRegex(RuntimeError, "required owner"):
                GATE["check"]("someone-else", "0.1.0")

    def test_rejects_unavailable_version(self):
        with patch("subprocess.run", side_effect=self.response):
            with self.assertRaisesRegex(RuntimeError, "unavailable"):
                GATE["check"]("maintainer", "0.2.0")

    def test_preserves_cargo_failure_context(self):
        with patch("subprocess.run", return_value=subprocess.CompletedProcess([], 101, "", "package not found")):
            with self.assertRaisesRegex(RuntimeError, "package not found"):
                GATE["check"]("maintainer", "0.1.0")


if __name__ == "__main__":
    unittest.main()
