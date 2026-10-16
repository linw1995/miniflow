import hashlib
import json
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest
from unittest.mock import patch

from cargo_cache import clean_workspace


class CargoCacheTests(unittest.TestCase):
    def test_all_protected_packages_do_not_fall_back_to_unfiltered_clean(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "mf-packaged-sources.json").write_text(json.dumps({"support": ["0.1.0", "digest"]}))
            with patch("cargo_cache.subprocess.run") as run:
                clean_workspace("cargo", root, root, ["release"], {"packages": [{"name": "support"}]})
            run.assert_not_called()

    def test_pruning_preserves_registry_fingerprints_with_workspace_package_names(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            target = root / "target"
            env = dict(os.environ, CARGO_TARGET_DIR=str(target))
            for name in ["CARGO", "CARGO_ENCODED_RUSTFLAGS", "CARGO_BUILD_TARGET", "RUSTFLAGS"]:
                env.pop(name, None)

            def cargo(*args):
                result = subprocess.run(["cargo", *args], cwd=root, env=env, capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                return result

            (root / "Cargo.toml").write_text('[workspace]\nmembers = ["support", "app"]\nresolver = "3"\n')
            for name in ["support", "app", "consumer"]:
                (root / name / "src").mkdir(parents=True)
                manifest = f'[package]\nname = "fixture-cache-{name}"\nversion = "0.1.0"\nedition = "2024"\n'
                if name == "app":
                    manifest += '[dependencies]\nfixture-cache-support = { path = "../support" }\n'
                if name == "consumer":
                    manifest += '[workspace]\n[dependencies]\nfixture-cache-support = "=0.1.0"\n'
                (root / name / "Cargo.toml").write_text(manifest)
                source = "pub fn value() -> u32 { 1 }\n" if name == "support" else "fn main() { assert_eq!(fixture_cache_support::value(), 1); }\n"
                (root / name / "src" / ("lib.rs" if name == "support" else "main.rs")).write_text(source)
            cargo("build", "--offline", "--workspace")
            cargo("package", "--offline", "--allow-dirty", "--no-verify", "-p", "fixture-cache-support")
            archive = target / "package/fixture-cache-support-0.1.0.crate"
            vendor = root / "vendor"
            with tarfile.open(archive) as package:
                package.extractall(vendor, filter="data")
            source = vendor / "fixture-cache-support-0.1.0"
            checksums = {
                str(path.relative_to(source)): hashlib.sha256(path.read_bytes()).hexdigest()
                for path in source.rglob("*") if path.is_file()
            }
            (source / ".cargo-checksum.json").write_text(json.dumps({
                "files": checksums, "package": hashlib.sha256(archive.read_bytes()).hexdigest(),
            }))
            config = root / "source.toml"
            config.write_text('[source.crates-io]\nreplace-with = "fixture"\n[source.fixture]\ndirectory = ' + json.dumps(str(vendor)) + '\n')
            build = ["build", "--offline", "--manifest-path", str(root / "consumer/Cargo.toml"), "--config", str(config)]
            cargo(*build)
            fingerprints = {path: path.stat().st_mtime_ns for path in (target / "debug/.fingerprint").glob("fixture-cache-support-*/invoked.timestamp")}
            self.assertGreaterEqual(len(fingerprints), 2)
            (target / "mf-packaged-sources.json").write_text(json.dumps({"fixture-cache-support": ["0.1.0", "digest"]}))
            result = subprocess.run([
                "python3", str(Path(__file__).with_name("prune-test-cache.py").resolve()), str(target),
            ], cwd=root, env=env, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertTrue(all(path.is_file() and path.stat().st_mtime_ns == time for path, time in fingerprints.items()))
            cargo(*build)
            self.assertTrue(all(path.stat().st_mtime_ns == time for path, time in fingerprints.items()))


if __name__ == "__main__":
    unittest.main()
