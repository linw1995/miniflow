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
        packaged_fixture = Path(report["vendor"]) / "fixture-multi-nodes-0.1.0"
        local_nodes = project / "local-nodes"
        git_nodes = scratch / "git-nodes"
        shutil.copytree(packaged_fixture, local_nodes)
        shutil.copytree(packaged_fixture, git_nodes)
        git_env = dict(env, GIT_CONFIG_GLOBAL=os.devnull, GIT_CONFIG_NOSYSTEM="1")
        git = ["git", "-c", "core.hooksPath=" + os.devnull, "-c", "commit.gpgsign=false", "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid"]
        for args in [["init", "--quiet"], ["add", "."], ["commit", "--quiet", "-m", "fixture"]]:
            subprocess.run(git + args, cwd=git_nodes, env=git_env, check=True)
        revision = subprocess.check_output(git + ["rev-parse", "HEAD"], cwd=git_nodes, env=git_env, text=True).strip()
        for name, dependency, expected in [
            ("local", {"package": "fixture-multi-nodes", "path": "local-nodes"}, 7),
            ("git", {"package": "fixture-multi-nodes", "git": git_nodes.as_uri(), "rev": revision, "features": ["double"]}, 14),
        ]:
            flow = json.loads(definition.read_text())
            flow["dependencies"] = {name: dependency}
            source = project / f"{name}.json"
            source.write_text(json.dumps(flow))
            executable = project / name
            build_dir = scratch / f"{name}-build"
            invocation = [str(cli), "compile", str(source), "--output", str(executable), "--build-dir", str(build_dir)]
            # Cargo's offline mode also prohibits the first checkout of a file:// repository.
            initial_env = dict(env, CARGO_NET_OFFLINE="false") if name == "git" else env
            subprocess.run(invocation, cwd=cli.parent, env=initial_env, check=True)
            saved_lock = source.with_suffix(".lock").read_bytes()
            subprocess.run(invocation + ["--locked"], cwd=cli.parent, env=env, check=True)
            assert source.with_suffix(".lock").read_bytes() == saved_lock
            result = subprocess.run([str(executable)], cwd=scratch, env={"PATH": ""}, capture_output=True, text=True, check=True)
            assert json.loads(result.stdout) == {"result": expected}
        assert (project / "local.lock").read_bytes() != (project / "git.lock").read_bytes()
        runtime_dir = scratch / "runtime"
        runtime_dir.mkdir()
        standalone = runtime_dir / "flow"
        shutil.copy2(output, standalone)
        shutil.rmtree(project)
        shutil.rmtree(build)
        validation = subprocess.run([str(standalone), "--validate"], cwd=runtime_dir, env={"PATH": ""}, capture_output=True, text=True, check=True)
        assert validation.stdout == ""
        invalid = subprocess.run([str(standalone), "--unknown"], cwd=runtime_dir, env={"PATH": ""}, capture_output=True, text=True)
        assert invalid.returncode != 0 and "usage:" in invalid.stderr
        result = subprocess.run([str(standalone)], cwd=runtime_dir, env={"PATH": ""}, capture_output=True, text=True, check=True)
        assert json.loads(result.stdout) == {"result": 14}
        print("Packaged CLI acceptance passed: registry/Git/path packages, independent locks, warm reuse, standalone execution")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--prepared", type=Path)
    check(parser.parse_args().prepared)
