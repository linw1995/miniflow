import json
import os
from pathlib import Path
import subprocess
import tempfile
import tomllib
import unittest


ROOT = Path(__file__).resolve().parent.parent
TOOLS = {"mfn-lsp", "mfn-test-affected"}


class ToolBoundaries(unittest.TestCase):
    def test_workspace_ownership_and_product_dependencies(self):
        metadata = json.loads(subprocess.check_output(
            ["cargo", "metadata", "--locked", "--no-deps", "--format-version", "1"], cwd=ROOT,
        ))
        packages = {p["name"]: p for p in metadata["packages"]}
        self.assertTrue(TOOLS <= packages.keys())
        for name in TOOLS:
            package = packages[name]
            manifest = Path(package["manifest_path"])
            self.assertTrue(manifest.is_relative_to(ROOT / "tools/test-affected/nodes"))
            self.assertEqual(package["publish"], [])
            self.assertIsInstance(tomllib.loads(manifest.read_text())["package"]["version"], str)
        for name, package in packages.items():
            if name not in TOOLS:
                self.assertFalse(TOOLS & {d["name"] for d in package["dependencies"]}, name)
        defaults = set(metadata["workspace_default_members"])
        self.assertEqual(defaults, {packages["mf-cli"]["id"]})

    def test_release_entries_exclude_tools_without_narrowing_development_audit(self):
        workflow = (ROOT / ".github/workflows/CD.yaml").read_text()
        for name in TOOLS:
            self.assertIn(f"--exclude {name}", workflow)
            support = (ROOT / "scripts/check-release-support.sh").read_text()
            self.assertNotIn(name, support)
        self.assertIn("generate-third-party-notices.sh --release", workflow)
        self.assertIn("generate-third-party-notices.sh --release", (ROOT / "flake.nix").read_text())
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            cargo = root / "cargo"
            cargo.write_text('''#!/usr/bin/env python3
import json, os, pathlib, sys
pathlib.Path(os.environ['ARG_LOG']).write_text(json.dumps(sys.argv[1:]))
pathlib.Path(sys.argv[sys.argv.index('--output-file') + 1]).write_text('notices\\n')
''')
            cargo.chmod(0o700)
            log = root / "args.json"
            env = os.environ | {"CARGO": str(cargo), "ARG_LOG": str(log)}
            for release in [False, True]:
                command = ["bash", str(ROOT / "scripts/generate-third-party-notices.sh")]
                if release:
                    command.append("--release")
                subprocess.run(command + [str(root / "notices.html")], env=env, check=True)
                args = json.loads(log.read_text())
                self.assertEqual("--workspace" in args, not release)
                if release:
                    self.assertEqual(args[args.index("--manifest-path") + 1], str(ROOT / "crates/mf-cli/Cargo.toml"))
                self.assertIn("--all-features", args)
                self.assertIn("--locked", args)


if __name__ == "__main__":
    unittest.main()
