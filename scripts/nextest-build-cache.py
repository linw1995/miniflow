#!/usr/bin/env python3

import json
import os
from pathlib import Path
import shutil
import subprocess
import tomllib


def main():
    workspace = Path(os.environ["NEXTEST_WORKSPACE_ROOT"])
    cargo = shutil.which(os.environ.get("CARGO", "cargo"))
    if cargo is None:
        raise RuntimeError("Cargo is unavailable")
    metadata = json.loads(
        subprocess.check_output(
            [cargo, "metadata", "--offline", "--locked", "--no-deps", "--format-version", "1"],
            cwd=workspace,
        )
    )
    # A different run must not replace an executable another test is validating.
    directory = (
        Path(metadata["target_directory"])
        / "nextest-build-cache"
        / os.environ["NEXTEST_RUN_ID"]
    )
    project = directory / "warmup"
    target = directory / "target"
    (project / "src").mkdir(parents=True)
    compiler_manifest = tomllib.loads(
        (workspace / "crates/mf-compiler/Cargo.toml").read_text()
    )
    manifest = (
        '[package]\nname = "mf-test-warmup"\nversion = "0.0.0"\nedition = "2024"\n'
        '\n[workspace]\n\n[dependencies]\n'
        f'serde_json = {json.dumps(compiler_manifest["dependencies"]["serde_json"])}\n'
    )
    # Match the feature graph emitted for telemetry-enabled runners.
    for name, path in [
        ("mf-runtime", "crates/mf-runtime"),
        ("mf-compiler", "crates/mf-compiler"),
        ("mf-telemetry", "crates/mf-telemetry"),
        ("mfn-core", "crates/builtin-nodes/core"),
        ("mfn-code", "crates/builtin-nodes/code"),
    ]:
        features = ', features = ["otlp", "snapshot"]' if name == "mf-telemetry" else ""
        manifest += (
            f"{name} = {{ path = {json.dumps(str(workspace / path))}, "
            f"default-features = false{features} }}\n"
        )
    (project / "Cargo.toml").write_text(manifest)
    (project / "src/main.rs").write_text("fn main() {}\n")
    env = dict(os.environ, CARGO_TARGET_DIR=str(target))
    # Preserve wrappers, flags, and coverage instrumentation used by the tests.
    for profile in ["release", "dev"]:
        subprocess.run(
            [cargo, "build", "--offline", "--profile", profile],
            cwd=project,
            env=env,
            check=True,
        )
    with open(os.environ["NEXTEST_ENV"], "a") as output:
        for name, value in {
            "CARGO": workspace / "scripts/nextest-cargo.sh",
            "MF_TEST_REAL_CARGO": cargo,
            "MF_TEST_TARGET_DIR": target,
        }.items():
            output.write(f"{name}={value}\n")


if __name__ == "__main__":
    main()
