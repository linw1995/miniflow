#!/usr/bin/env python3
"""Exercise the default CLI outside the checkout using packaged registry dependencies."""

import argparse
import json
import os
from pathlib import Path
import runpy
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parent.parent
PACKAGING = runpy.run_path(str(ROOT / "scripts/prepare-support-packages.py"))


def check(prepared=None):
    with tempfile.TemporaryDirectory(prefix="mf-packaged-cli-") as scratch_name:
        scratch = Path(scratch_name)
        if prepared:
            report = json.loads((prepared / "packages.json").read_text())
        else:
            report = PACKAGING["prepare"](scratch / "packages")
        cargo = os.environ.get("CARGO", "cargo")
        target = scratch / "cli-target"
        env = dict(os.environ, CARGO_TARGET_DIR=str(target))
        subprocess.run([cargo, "build", "--release", "--locked", "--offline", "-p", "mf-cli", "--no-default-features"], cwd=ROOT, env=env, check=True)
        cli = scratch / "installed" / "mf"
        cli.parent.mkdir()
        shutil.copy2(target / "release/mf", cli)
        fixture = ROOT / "crates/mf-compiler/tests/fixtures/multi-nodes/Cargo.toml"
        subprocess.run([cargo, "package", "--manifest-path", str(fixture), "--offline", "--allow-dirty", "--no-verify", "--config", report["config"]], cwd=ROOT, env=env, check=True)
        PACKAGING["add_package"](target / "package/fixture-multi-nodes-0.1.0.crate", Path(report["vendor"]))
        home = scratch / "cargo-home"
        home.mkdir()
        shutil.copyfile(report["config"], home / "config.toml")
        env.update(CARGO_HOME=str(home), CARGO_NET_OFFLINE="true", MF_DEV_SUPPORT_ROOT=str(scratch / "unavailable-checkout"))
        project = scratch / "user-project"
        project.mkdir()
        definition = project / "flow.json"
        definition.write_text(json.dumps({
            "version": "2026-09-26",
            "dependencies": {"external": {"package": "fixture-multi-nodes", "version": "=0.1.0", "features": ["double"]}},
            "nodes": [{"id": "source", "kind": "fixture.source"}, {"id": "echo", "kind": "fixture.echo"}],
            "edges": [{"from_node": "source", "from_output": "value", "to_node": "echo", "to_input": "input"}],
            "outputs": [{"name": "result", "node": "echo", "port": "value"}],
        }))
        build = scratch / "build"
        output = project / "flow"
        command = [str(cli), "compile", str(definition), "--output", str(output), "--build-dir", str(build)]
        subprocess.run(command, cwd=scratch, env=env, check=True)
        locked = (project / "flow.lock").read_bytes()
        repeated = subprocess.run(command + ["--locked"], cwd=scratch, env=env, check=True, capture_output=True, text=True)
        assert "reused compiled runner" in repeated.stderr
        assert (project / "flow.lock").read_bytes() == locked
        manifest = (build / "Cargo.toml").read_text()
        assert "path =" not in manifest and "mf-bundle" not in manifest
        runtime_dir = scratch / "runtime"
        runtime_dir.mkdir()
        standalone = runtime_dir / "flow"
        shutil.copy2(output, standalone)
        shutil.rmtree(project)
        shutil.rmtree(build)
        result = subprocess.run([str(standalone)], cwd=runtime_dir, env={"PATH": ""}, capture_output=True, text=True, check=True)
        assert json.loads(result.stdout) == {"result": 14}
        print("Packaged CLI acceptance passed: registry packages, locked reuse, standalone execution")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--prepared", type=Path)
    check(parser.parse_args().prepared)
