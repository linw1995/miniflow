import json
import os
from pathlib import Path
import subprocess


def clean_workspace(cargo, workspace, target, profiles, metadata=None):
    protected = set()
    marker = Path(target) / "mf-packaged-sources.json"
    if marker.is_file():
        try:
            packages = json.loads(marker.read_text())
            if isinstance(packages, dict) and all(
                isinstance(value, list) and len(value) == 2
                and all(isinstance(item, str) for item in value)
                for value in packages.values()
            ):
                protected = set(packages)
        except (ValueError, OSError):
            pass
    selection = ["--workspace"]
    if protected:
        if metadata is None:
            metadata = json.loads(subprocess.check_output(
                [cargo, "metadata", "--offline", "--locked", "--no-deps", "--format-version", "1"],
                cwd=workspace,
            ))
        selection = [
            argument for package in metadata["packages"] if package["name"] not in protected
            for argument in ["-p", package["name"]]
        ]
        if not selection:
            return
    # Cargo cleans fingerprints by package name across sources. Protected
    # packages are invalidated by their content digests before the fixture builds.
    for profile in profiles:
        subprocess.run(
            [cargo, "clean", *selection, "--profile", profile],
            cwd=workspace,
            env=dict(os.environ, CARGO_TARGET_DIR=str(target)),
            check=True,
        )
