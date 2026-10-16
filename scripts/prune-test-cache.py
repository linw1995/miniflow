#!/usr/bin/env python3

import os
from pathlib import Path
import subprocess
import sys


def prune_target(target):
    if not target.is_dir():
        return
    # Preparation already discards checkout artifacts. Do not transfer them
    # through the remote cache just to delete them on the next runner.
    for profile in ["dev", "release"]:
        subprocess.run(
            ["cargo", "clean", "--workspace", "--profile", profile],
            env=dict(os.environ, CARGO_TARGET_DIR=str(target)),
            check=True,
        )
    for profile in ["debug", "release"]:
        for directory in [target / profile, target / profile / "deps"]:
            if not directory.is_dir():
                continue
            for path in directory.iterdir():
                # Temporary generated projects have new identities on each run.
                # Keep libraries and dependency build scripts, not their runners.
                if (
                    path.is_file()
                    and not path.name.startswith(".")
                    and path.suffix in ["", ".exe"]
                    and os.access(path, os.X_OK)
                ):
                    path.unlink()


def main():
    for value in sys.argv[1:]:
        prune_target(Path(value))


if __name__ == "__main__":
    main()
