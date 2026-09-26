#!/usr/bin/env python3
"""Package and verify support crates against an isolated Cargo registry source."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parent.parent
PACKAGES = [
    ("mf-runtime", ROOT / "crates/mf-runtime"),
    ("mfn-constant", ROOT / "crates/builtin-nodes/constant"),
    ("mfn-identity", ROOT / "crates/builtin-nodes/identity"),
    ("mf-compiler", ROOT / "crates/mf-compiler"),
]


def run(args, *, env, cwd=ROOT):
    subprocess.run(args, cwd=cwd, env=env, check=True)


def add_package(archive, vendor):
    with tarfile.open(archive) as package:
        package.extractall(vendor, filter="data")
    directory = vendor / archive.name.removesuffix(".crate")
    checksums = {
        str(path.relative_to(directory)): hashlib.sha256(path.read_bytes()).hexdigest()
        for path in directory.rglob("*")
        if path.is_file() and path.name != ".cargo-checksum.json"
    }
    (directory / ".cargo-checksum.json").write_text(json.dumps({
        "files": checksums,
        "package": hashlib.sha256(archive.read_bytes()).hexdigest(),
    }))
    return directory


def prepare(output):
    output = output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    if any(output.iterdir()):
        raise SystemExit("output directory must be empty")
    vendor = output / "vendor"
    cargo = os.environ.get("CARGO", "cargo")
    env = dict(os.environ, CARGO_TARGET_DIR=str(output / "target"))
    subprocess.run(
        [cargo, "vendor", "--offline", "--locked", "--respect-source-config", "--versioned-dirs", str(vendor)],
        cwd=ROOT, env=env, stdout=subprocess.DEVNULL, check=True,
    )
    config = output / "source.toml"
    config.write_text(
        '[source.crates-io]\nreplace-with = "mf-package-fixture"\n'
        '[source.mf-package-fixture]\ndirectory = ' + json.dumps(str(vendor)) + '\n'
    )
    version = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]["package"]["version"]
    archives = {}
    for name, crate in PACKAGES:
        run([cargo, "package", "--offline", "--allow-dirty", "--no-verify", "--manifest-path", str(crate / "Cargo.toml"), "--config", str(config)], env=env)
        archive = output / "target/package" / f"{name}-{version}.crate"
        add_package(archive, vendor)
        archives[name] = str(archive)
    with tempfile.TemporaryDirectory(prefix="mf-package-verification-") as scratch:
        for name, _ in PACKAGES:
            extracted = vendor / f"{name}-{version}"
            verification = Path(scratch) / name
            shutil.copytree(extracted, verification)
            run([cargo, "check", "--offline", "--lib", "--manifest-path", str(verification / "Cargo.toml"), "--config", str(config)], env=env, cwd=verification)
    compiler = tomllib.loads((vendor / f"mf-compiler-{version}/Cargo.toml").read_text())
    assert "mf-bundle" not in compiler["dependencies"]
    for dependency in compiler["dependencies"].values():
        assert "path" not in dependency
    report = {"version": version, "config": str(config), "vendor": str(vendor), "archives": archives}
    (output / "packages.json").write_text(json.dumps(report, indent=2) + "\n")
    return report


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    print(json.dumps(prepare(args.output), indent=2))
