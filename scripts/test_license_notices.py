import hashlib
import html
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import tomllib
import unittest


ROOT = Path(__file__).resolve().parent.parent
CLARIFICATION_WARNING = "failed to validate all files specified in clarification for crate "


class LicensePolicyTests(unittest.TestCase):
    def test_additional_licenses_are_package_scoped(self):
        policy = tomllib.loads((ROOT / "about.toml").read_text())
        for license_id in ("MPL-2.0", "bzip2-1.0.6", "OFL-1.1"):
            self.assertNotIn(license_id, policy["accepted"])
        self.assertEqual(policy["cbindgen"]["accepted"], ["MPL-2.0"])
        self.assertEqual(policy["option-ext"]["accepted"], ["MPL-2.0"])
        self.assertEqual(policy["libbz2-rs-sys"]["accepted"], ["bzip2-1.0.6"])
        self.assertEqual(policy["ely-gpui-component"]["accepted"], ["OFL-1.1"])
        self.assertFalse(policy["ignore-build-dependencies"])
        self.assertFalse(policy["ignore-transitive-dependencies"])
        clarification = policy["ely-gpui-component"]["clarify"]
        self.assertEqual(clarification["license"], "MIT AND OFL-1.1 AND ISC")
        self.assertEqual(len(clarification["files"]), 6)
        self.assertEqual(
            [file["license"] for file in clarification["files"]],
            ["MIT", "OFL-1.1", "OFL-1.1", "OFL-1.1", "ISC", "MIT"],
        )


class NoticeFixture:
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="mf license notices ")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        (self.root / "scripts").mkdir()
        shutil.copyfile(ROOT / "scripts/generate-third-party-notices.sh", self.root / "scripts/generate-third-party-notices.sh")
        shutil.copyfile(ROOT / "about.hbs", self.root / "about.hbs")
        (self.root / "tmp").mkdir()
        self.report = self.root / "report.html"
        self.arguments = self.root / "arguments.json"
        cargo = self.root / "cargo-stub"
        cargo.write_text('''#!/usr/bin/env python3
import json, os, pathlib, sys
pathlib.Path(os.environ["ARG_LOG"]).write_text(json.dumps(sys.argv[1:]))
if os.environ.get("STUB_DIAGNOSTIC"):
    print(os.environ["STUB_DIAGNOSTIC"], file=sys.stderr)
status = int(os.environ.get("STUB_STATUS", "0"))
if not status:
    pathlib.Path(sys.argv[sys.argv.index("--output-file") + 1]).write_bytes(b"notices  \\r\\n")
sys.exit(status)
''')
        cargo.chmod(0o700)
        self.env = os.environ | {
            "CARGO": str(cargo), "ARG_LOG": str(self.arguments),
            "TMPDIR": str(self.root / "tmp"), "CARGO_ABOUT_OFFLINE": "1",
        }

    def generate(self, *args):
        return subprocess.run(
            ["bash", str(self.root / "scripts/generate-third-party-notices.sh"), *args, str(self.report)],
            cwd=self.root, env=self.env, capture_output=True, text=True,
        )


class NoticeScriptTests(NoticeFixture, unittest.TestCase):
    def test_scope_flags_normalization_and_diagnostic_forwarding(self):
        self.env["STUB_DIAGNOSTIC"] = "an unrelated audit warning"
        for release in (False, True):
            with self.subTest(release=release):
                result = self.generate(*(["--release"] if release else []))
                self.assertEqual(result.returncode, 0, result.stderr)
                args = json.loads(self.arguments.read_text())
                self.assertEqual(args[:6], ["about", "-L", "warn", "--color", "never", "generate"])
                self.assertEqual("--workspace" in args, not release)
                if release:
                    self.assertEqual(args[args.index("--manifest-path") + 1], str(self.root / "crates/mf-cli/Cargo.toml"))
                self.assertEqual(args[args.index("--config") + 1], str(self.root / "about.toml"))
                for flag in ("--all-features", "--locked", "--fail", "--offline"):
                    self.assertIn(flag, args)
                self.assertEqual(self.report.read_bytes(), b"notices\n")
                self.assertIn("an unrelated audit warning", result.stderr)
                self.assertEqual(list((self.root / "tmp").iterdir()), [])

    def test_explicit_manifest_uses_the_repository_policy(self):
        manifest = "probe with spaces/Cargo.toml"
        result = self.generate("--manifest-path", manifest)
        self.assertEqual(result.returncode, 0, result.stderr)
        args = json.loads(self.arguments.read_text())
        self.assertNotIn("--workspace", args)
        self.assertEqual(args[args.index("--manifest-path") + 1], manifest)
        self.assertEqual(args[args.index("--config") + 1], str(self.root / "about.toml"))

    def test_invalid_scope_options_fail_before_auditing(self):
        for args in (
            ["--manifest-path"], ["--manifest-path", "--release"],
            ["--release", "--manifest-path", "probe.toml"],
            ["--manifest-path", "probe.toml", "output.html", "extra"],
        ):
            with self.subTest(args=args):
                result = subprocess.run(
                    ["bash", str(self.root / "scripts/generate-third-party-notices.sh"), *args],
                    env=self.env, capture_output=True, text=True,
                )
                self.assertEqual(result.returncode, 2, result.stderr)
                self.assertFalse(self.arguments.exists())
                self.assertFalse(self.report.exists())

    def test_clarification_fallback_is_fatal_even_when_cargo_about_succeeds(self):
        self.report.write_text("previous valid report\n")
        self.env["STUB_DIAGNOSTIC"] = CLARIFICATION_WARNING + "ely-gpui-component: checksum mismatch"
        result = self.generate()
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn("review the source files and checksums", result.stderr)
        self.assertEqual(self.report.read_text(), "previous valid report\n")
        self.assertEqual(list((self.root / "tmp").iterdir()), [])

    def test_cargo_about_failure_is_not_hidden(self):
        self.env.update(STUB_STATUS="42", STUB_DIAGNOSTIC="unaccepted license")
        result = self.generate()
        self.assertEqual(result.returncode, 42)
        self.assertIn("unaccepted license", result.stderr)
        self.assertFalse(self.report.exists())
        self.assertEqual(list((self.root / "tmp").iterdir()), [])


class ResourceClarificationTests(NoticeFixture, unittest.TestCase):
    def setUp(self):
        super().setUp()
        self.env["CARGO"] = shutil.which("cargo") or "cargo"
        (self.root / "Cargo.toml").write_text('''[package]
name = "ely-gpui-component"
version = "0.1.1"
edition = "2024"
license = "MIT"
publish = false
[workspace]
''')
        (self.root / "Cargo.lock").write_text('''version = 4
[[package]]
name = "ely-gpui-component"
version = "0.1.1"
''')
        (self.root / "src").mkdir()
        (self.root / "src/lib.rs").write_text("")
        # Synthetic notices keep the fixture offline; production hashes refer to upstream originals.
        self.notices = {
            "LICENSE": "Ely code fixture notice\n",
            "assets/fonts/Inter-LICENSE.txt": "Inter fixture copyright\n",
            "assets/fonts/JetBrainsMono-LICENSE.txt": "JetBrains Mono fixture copyright\n",
            "assets/fonts/ibm-plex-sans/IBMPlexSans-LICENSE.txt": 'IBM fixture copyright; Reserved Font Name "Plex"\n',
            "assets/icons/LICENSE": (
                "Lucide fixture copyright\nOR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.\n\n---\n\n"
                "The following Lucide icons are derived from the Feather project:\nFeather fixture copyright\n"
            ),
        }
        for relative, text in self.notices.items():
            path = self.root / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text)
        config = (ROOT / "about.toml").read_text()
        clarification = tomllib.loads(config)["ely-gpui-component"]["clarify"]
        self.sections = []
        for file in clarification["files"]:
            text = self.notices[file["path"]]
            start = text.index(file["start"]) if "start" in file else 0
            end = text.index(file["end"], start) + len(file["end"]) if "end" in file else len(text)
            section = text[start:end]
            self.sections.append((file["path"], section))
            config = config.replace(file["checksum"], hashlib.sha256(section.encode()).hexdigest())
        (self.root / "about.toml").write_text(config)

    def test_all_resource_notices_are_rendered_with_source_paths(self):
        result = self.generate()
        self.assertEqual(result.returncode, 0, result.stderr)
        report = html.unescape(self.report.read_text())
        for path, text in self.sections:
            with self.subTest(path=path, text=text):
                self.assertIn(text.rstrip(), report)
                self.assertIn(f"Source file: <code>{path}</code>", report)
        self.assertIn("SIL Open Font License 1.1", report)

    def test_changed_resource_checksum_prevents_publishing_a_report(self):
        (self.root / "assets/fonts/Inter-LICENSE.txt").write_text("changed font license\n")
        result = self.generate()
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn(CLARIFICATION_WARNING, result.stderr)
        self.assertIn("License clarification validation failed", result.stderr)
        self.assertFalse(self.report.exists())

    def test_missing_resource_prevents_publishing_a_report(self):
        (self.root / "assets/icons/LICENSE").unlink()
        result = self.generate()
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn("License clarification validation failed", result.stderr)
        self.assertFalse(self.report.exists())


if __name__ == "__main__":
    unittest.main()
