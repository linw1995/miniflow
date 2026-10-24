#!/usr/bin/env python3

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess

from cargo_cache import clean_workspace


def main(optimized=False):
    # Behavior fixtures need release semantics, but not optimizer throughput.
    # Keep caller overrides and coverage instrumentation intact.
    os.environ.setdefault("CARGO_PROFILE_RELEASE_OPT_LEVEL", "0")
    if optimized:
        os.environ["CARGO_PROFILE_RELEASE_OPT_LEVEL"] = "3"
        os.environ["CARGO_PROFILE_RELEASE_LTO"] = "true"
        os.environ["CARGO_PROFILE_RELEASE_CODEGEN_UNITS"] = "1"
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
    cache_directory = os.environ.get("MF_TEST_BUILD_CACHE_DIR")
    directory = (
        Path(cache_directory)
        if cache_directory
        else Path(metadata["target_directory"])
        / "nextest-build-cache"
        / os.environ["NEXTEST_RUN_ID"]
    )
    if optimized:
        directory /= "optimized"
    # Package steps share immutable binaries and one preparation session. A new
    # coverage invocation must still discard restored workspace artifacts.
    session = os.environ.get("MF_TEST_BUILD_CACHE_SESSION")
    marker = directory / "prepared-session.json"
    identity = {
        "session": session,
        "workspace": str(workspace.resolve()),
        "cargo": cargo,
        "environment": hashlib.sha256(json.dumps({
            name: value for name, value in os.environ.items()
            if name.startswith(("CARGO_", "RUST"))
        }, sort_keys=True).encode()).hexdigest(),
    }
    prepared = None
    if marker.is_file():
        try:
            prepared = json.loads(marker.read_text())
        except (ValueError, OSError):
            pass
    if not session or prepared != identity or not all(
        path.exists() for path in [directory / "target", directory / "vendor", directory / "source.toml"]
    ):
        marker.unlink(missing_ok=True)
        prepare_cache(workspace, cargo, metadata, directory, bool(cache_directory))
        if session:
            pending = marker.with_suffix(".tmp")
            pending.write_text(json.dumps(identity, sort_keys=True))
            pending.replace(marker)
    else:
        print("Reusing generated-project dependencies prepared for this coverage session.")
    target = directory / "target"
    source_config = directory / "source.toml"
    vendor = directory / "vendor"
    with open(os.environ["NEXTEST_ENV"], "a") as output:
        environment = {
            "CARGO": workspace / "scripts/nextest-cargo.sh",
            # Selected fixtures use local packages and already-fetched registry dependencies.
            "CARGO_NET_OFFLINE": "true",
            "CARGO_PROFILE_RELEASE_OPT_LEVEL": os.environ["CARGO_PROFILE_RELEASE_OPT_LEVEL"],
            "MF_TEST_REAL_CARGO": cargo,
            "MF_TEST_TARGET_DIR": target,
            "MF_TEST_PACKAGE_TARGET_DIR": directory / "packaged-target",
            "MF_TEST_SOURCE_CONFIG": source_config,
            "MF_TEST_VENDOR_DIR": vendor,
        }
        if optimized:
            environment["CARGO_PROFILE_RELEASE_LTO"] = "true"
            environment["CARGO_PROFILE_RELEASE_CODEGEN_UNITS"] = "1"
        for name, value in environment.items():
            output.write(f"{name}={value}\n")


def prepare_cache(workspace, cargo, metadata, directory, persistent):
    project = directory / "warmup"
    target = directory / "target"
    (project / "src").mkdir(parents=True, exist_ok=True)
    vendor = directory / "vendor"
    source_config = directory / "source.toml"
    subprocess.run(
        [
            cargo,
            "vendor",
            "--offline",
            "--locked",
            "--respect-source-config",
            "--versioned-dirs",
            str(vendor),
        ],
        cwd=workspace,
        check=True,
        stdout=subprocess.DEVNULL,
    )
    # Cargo fingerprints include source paths; fixtures must use the same vendor source.
    source_config.write_text(
        '[source.crates-io]\nreplace-with = "mf-package-fixture"\n'
        f'[source.mf-package-fixture]\ndirectory = {json.dumps(str(vendor))}\n'
    )
    compiler = next(
        package for package in metadata["packages"] if package["name"] == "mf-compiler"
    )
    serde_json_requirement = next(
        dependency["req"]
        for dependency in compiler["dependencies"]
        if dependency["name"] == "serde_json" and dependency["kind"] is None
    )
    manifest = (
        '[package]\nname = "mf-test-warmup"\nversion = "0.0.0"\nedition = "2024"\n'
        '\n[workspace]\n\n[dependencies]\n'
        f'serde_json = {json.dumps(serde_json_requirement)}\n'
    )
    # Match the feature graph emitted for telemetry-enabled runners.
    for name, path in [
        ("mf-runtime", "crates/mf-runtime"),
        ("mf-compiler", "crates/mf-compiler"),
        ("mf-telemetry", "crates/mf-telemetry"),
        ("mfn-core", "crates/builtin-nodes/core"),
        ("mfn-code", "crates/builtin-nodes/code"),
        ("mfn-llm", "crates/builtin-nodes/llm"),
    ]:
        features = ', features = ["otlp", "snapshot"]' if name == "mf-telemetry" else ""
        manifest += (
            f"{name} = {{ path = {json.dumps(str(workspace / path))}, "
            f"default-features = false{features} }}\n"
        )
    manifest += "\n[build-dependencies]\n"
    for name, path in [
        ("mf-runtime", "crates/mf-runtime"),
        ("mf-compiler", "crates/mf-compiler"),
        ("mfn-core", "crates/builtin-nodes/core"),
        ("mfn-code", "crates/builtin-nodes/code"),
        ("mfn-llm", "crates/builtin-nodes/llm"),
    ]:
        features = ', features = ["codegen"]' if name == "mf-compiler" else ""
        manifest += (
            f"{name} = {{ path = {json.dumps(str(workspace / path))}, "
            f"default-features = false{features} }}\n"
        )
    (project / "build.rs").write_text("fn main() {}\n")
    (project / "Cargo.toml").write_text(manifest)
    (project / "src/main.rs").write_text("fn main() {}\n")
    env = dict(os.environ, CARGO_TARGET_DIR=str(target))
    if persistent:
        # Checkout mtimes can precede restored artifacts. Keep dependencies, never old workspace code.
        targets = [(target, ["release"])]
        package_target = directory / "packaged-target"
        if package_target.exists():
            targets.append((package_target, ["release"]))
        for build_target, profiles in targets:
            clean_workspace(cargo, workspace, build_target, profiles, metadata)
    # Older caches prewarmed a second profile that no selected fixture uses now.
    if (target / "debug").exists():
        shutil.rmtree(target / "debug")
    # Preserve wrappers, flags, and coverage instrumentation used by the tests.
    subprocess.run(
        [cargo, "build", "--offline", "--release", "--config", str(source_config)],
        cwd=project,
        env=env,
        check=True,
    )
    # Nested targets may omit this tag when an ancestor already has one.
    (target / "CACHEDIR.TAG").write_text(
        "Signature: 8a477f597d28d172789f06886806bc55\n"
    )


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--optimized", action="store_true")
    main(parser.parse_args().optimized)
