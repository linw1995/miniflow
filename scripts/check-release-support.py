#!/usr/bin/env python3
"""Check ownership and availability of the support packages required by a CLI release."""

import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parent.parent
PACKAGES = ("mf-runtime", "mf-compiler", "mfn-constant", "mfn-identity")


def cargo_output(args, cwd):
    command = [os.environ.get("CARGO", "cargo"), *args]
    result = subprocess.run(command, cwd=cwd, capture_output=True, text=True)
    if result.returncode:
        raise RuntimeError(f"support package prerequisite failed ({' '.join(args)}):\n{result.stderr}")
    return result.stdout


def check(owner, version):
    with tempfile.TemporaryDirectory(prefix="mf-release-support-") as directory:
        root = Path(directory)
        for package in PACKAGES:
            owners = cargo_output(["owner", "--list", "--registry", "crates-io", "--color", "never", package], root)
            logins = {line.split()[0] for line in owners.splitlines() if line.split()}
            if owner not in logins:
                raise RuntimeError(f"{package}: required owner {owner!r} is absent; verify registry ownership before releasing the CLI")
        (root / "src").mkdir()
        (root / "src/main.rs").write_text("fn main() {}\n")
        manifest = '[package]\nname = "mf-release-support-check"\nversion = "0.0.0"\nedition = "2024"\n[workspace]\n[dependencies]\n'
        manifest += ''.join(f'{package} = "={version}"\n' for package in PACKAGES)
        (root / "Cargo.toml").write_text(manifest)
        metadata = json.loads(cargo_output(["metadata", "--format-version", "1"], root))
        available = {(package["name"], package["version"]) for package in metadata["packages"] if (package.get("source") or "").startswith("registry+")}
        missing = [package for package in PACKAGES if (package, version) not in available]
        if missing:
            raise RuntimeError(f"required support versions are unavailable: {', '.join(missing)} at {version}")
    print(f"Support packages for CLI {version} are available and owned by {owner}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--owner", required=True)
    args = parser.parse_args()
    version = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]["package"]["version"]
    try:
        check(args.owner, version)
    except RuntimeError as error:
        raise SystemExit(str(error)) from error
