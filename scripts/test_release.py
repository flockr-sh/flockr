from __future__ import annotations

import hashlib
import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path

from scripts import release

ROOT = Path(__file__).resolve().parent.parent

CHANGELOG = """# Changelog

## [1.1.0] - 2026-11-01

### Fixed
- Newer fix.

## [1.0.0] - 2026-10-18

First release.

## [0.1.0] - 2026-01-01

Old.
"""


def sums_for(version: str) -> dict:
    return {name: f"{index:064x}" for index, name in enumerate(release.TARGETS.values())}


class ReleaseNotesTests(unittest.TestCase):
    def test_extracts_only_the_requested_section(self) -> None:
        self.assertEqual(release.changelog_notes(CHANGELOG, "1.0.0"), "First release.")
        self.assertEqual(release.changelog_notes(CHANGELOG, "1.1.0"), "### Fixed\n- Newer fix.")

    def test_missing_section_fails(self) -> None:
        with self.assertRaises(SystemExit):
            release.changelog_notes(CHANGELOG, "2.0.0")


class ManifestTests(unittest.TestCase):
    def test_keeps_older_flockr_releases_and_drops_foreign_ones(self) -> None:
        first = release.build_manifest(
            {"releases": {"0.9.5": {"assets": {"linux-x86_64": "https://github.com/herdrdev/herdr/x"}}}},
            "1.0.0",
            "First release.",
            22,
            1,
            sums_for("1.0.0"),
        )
        self.assertEqual(list(first["releases"]), ["1.0.0"])

        second = release.build_manifest(first, "1.1.0", "Newer.", 23, 1, sums_for("1.1.0"))
        self.assertEqual(sorted(second["releases"]), ["1.0.0", "1.1.0"])
        self.assertEqual(second["version"], "1.1.0")
        self.assertEqual(
            second["assets"]["linux-x86_64"],
            "https://github.com/flockr-sh/flockr/releases/download/v1.1.0/flockr-linux-x86_64",
        )
        self.assertEqual(second["releases"]["1.0.0"]["protocol"], 22)

    def test_missing_checksum_fails(self) -> None:
        sums = sums_for("1.0.0")
        del sums["flockr-windows-x86_64.zip"]
        with self.assertRaises(SystemExit):
            release.build_manifest({}, "1.0.0", "notes", 22, 1, sums)


class InstallScriptTests(unittest.TestCase):
    def test_installs_binary_from_generated_manifest(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = Path(tmp)
            binary = b"#!/bin/sh\necho flockr test\n"
            digest = hashlib.sha256(binary).hexdigest()
            (tmp_path / "asset").write_bytes(binary)

            manifest = release.build_manifest(
                {}, "1.0.0", "notes", 22, 1, {name: digest for name in release.TARGETS.values()}
            )
            # Point the generated URLs at the local file so curl can fetch it.
            for target in manifest["assets"]:
                manifest["assets"][target] = (tmp_path / "asset").as_uri()
            (tmp_path / "latest.json").write_text(json.dumps(manifest, indent=2) + "\n")

            install_dir = tmp_path / "bin"
            env = dict(os.environ)
            env["FLOCKR_MANIFEST_URL"] = (tmp_path / "latest.json").as_uri()
            env["FLOCKR_INSTALL_DIR"] = str(install_dir)
            result = subprocess.run(
                ["sh", str(ROOT / "distribution" / "install.sh")],
                env=env,
                capture_output=True,
                text=True,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual((install_dir / "flockr").read_bytes(), binary)
            self.assertTrue(os.access(install_dir / "flockr", os.X_OK))


if __name__ == "__main__":
    unittest.main()
