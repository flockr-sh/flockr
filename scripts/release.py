#!/usr/bin/env python3
"""Release helpers used by .github/workflows/release.yml.

notes:    print the CHANGELOG.md section for a version.
manifest: rewrite distribution/latest.json for a published release.
"""

import argparse
import json
import re
import sys
from pathlib import Path

REPO = "flockr-sh/flockr"
TARGETS = {
    "linux-x86_64": "flockr-linux-x86_64",
    "linux-aarch64": "flockr-linux-aarch64",
    "macos-x86_64": "flockr-macos-x86_64",
    "macos-aarch64": "flockr-macos-aarch64",
    "windows-x86_64": "flockr-windows-x86_64.zip",
}


def changelog_notes(changelog: str, version: str) -> str:
    heading = re.compile(rf"^## \[{re.escape(version)}\]")
    lines = changelog.splitlines()
    for start, line in enumerate(lines):
        if heading.match(line):
            break
    else:
        raise SystemExit(f"CHANGELOG.md has no '## [{version}]' section")
    body = []
    for line in lines[start + 1 :]:
        if line.startswith("## ["):
            break
        body.append(line)
    notes = "\n".join(body).strip()
    if not notes:
        raise SystemExit(f"CHANGELOG.md section for {version} is empty")
    return notes


def read_sha256sums(text: str) -> dict:
    sums = {}
    for line in text.splitlines():
        parts = line.split()
        if len(parts) == 2:
            sums[parts[1].lstrip("*")] = parts[0].lower()
    return sums


def build_manifest(
    previous: dict,
    version: str,
    notes: str,
    protocol: int,
    endpoint_generation: int,
    sums: dict,
) -> dict:
    assets = {}
    sha256 = {}
    for target, name in TARGETS.items():
        if name not in sums:
            raise SystemExit(f"SHA256SUMS has no entry for {name}")
        assets[target] = f"https://github.com/{REPO}/releases/download/v{version}/{name}"
        sha256[target] = sums[name]

    release = {
        "notes": notes,
        "protocol": protocol,
        "endpoint_generation": endpoint_generation,
        "assets": assets,
        "sha256": sha256,
    }
    # Keep older flockr releases so remote hosts can still fetch the binary
    # that matches an older client. Drop entries that point anywhere else.
    releases = {
        old_version: entry
        for old_version, entry in previous.get("releases", {}).items()
        if old_version != version
        and all(
            url.startswith(f"https://github.com/{REPO}/")
            for url in entry.get("assets", {}).values()
        )
    }
    releases[version] = release

    # Top-level "assets" must come before "releases": install.sh reads the
    # first "assets" block.
    return {
        "version": version,
        "protocol": protocol,
        "endpoint_generation": endpoint_generation,
        "notes": notes,
        "assets": assets,
        "sha256": sha256,
        "releases": releases,
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="command", required=True)

    notes = sub.add_parser("notes")
    notes.add_argument("--version", required=True)
    notes.add_argument("--changelog", default="CHANGELOG.md")

    manifest = sub.add_parser("manifest")
    manifest.add_argument("--version", required=True)
    manifest.add_argument("--changelog", default="CHANGELOG.md")
    manifest.add_argument("--sha256sums", required=True)
    manifest.add_argument("--manifest", default="distribution/latest.json")
    manifest.add_argument("--wire", default="src/protocol/wire.rs")
    manifest.add_argument("--endpoint", default="src/protocol/endpoint.rs")

    args = parser.parse_args()
    notes_text = changelog_notes(Path(args.changelog).read_text(), args.version)
    if args.command == "notes":
        sys.stdout.write(notes_text + "\n")
        return

    protocol = read_const(Path(args.wire), "PROTOCOL_VERSION")
    endpoint_generation = read_const(Path(args.endpoint), "ENDPOINT_PROTOCOL_GENERATION")
    path = Path(args.manifest)
    previous = json.loads(path.read_text()) if path.exists() else {}
    result = build_manifest(
        previous,
        args.version,
        notes_text,
        protocol,
        endpoint_generation,
        read_sha256sums(Path(args.sha256sums).read_text()),
    )
    path.write_text(json.dumps(result, indent=2) + "\n")


def read_const(path: Path, name: str) -> int:
    match = re.search(rf"pub const {name}: u32 = (\d+);", path.read_text())
    if match is None:
        raise SystemExit(f"{path} has no {name}")
    return int(match.group(1))


if __name__ == "__main__":
    main()
