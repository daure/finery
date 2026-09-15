#!/usr/bin/env python3
"""Resolve published Tuicore in the disposable GitHub Actions checkout."""

import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tomllib
import urllib.request


def latest_tuicore():
    request = urllib.request.Request(
        "https://crates.io/api/v1/crates/tuicore",
        headers={"User-Agent": "tuicore-ci-preparation"},
    )
    with urllib.request.urlopen(request, timeout=30) as response:
        versions = json.load(response)["versions"]
    stable = [
        version["num"] for version in versions
        if not version["yanked"]
        and re.fullmatch(r"(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)", version["num"])
    ]
    if not stable:
        raise ValueError("No stable, non-yanked Tuicore release is available")
    return max(stable, key=lambda version: tuple(map(int, version.split("."))))


def prepare(root):
    if os.environ.get("GITHUB_ACTIONS") != "true":
        raise ValueError("Registry preparation is only allowed in GitHub Actions")
    manifest_path = root / "Cargo.toml"
    manifest = manifest_path.read_text()
    if tomllib.loads(manifest)["dependencies"]["tuicore"] != {"path": "../tuicore"}:
        raise ValueError("Tuicore must use the local ../tuicore path")
    version = latest_tuicore()
    manifest, replacements = re.subn(
        r'^tuicore\s*=\s*\{\s*path\s*=\s*"\.\./tuicore"\s*\}\s*$',
        f'tuicore = "={version}"', manifest, flags=re.MULTILINE,
    )
    if replacements != 1:
        raise ValueError("Expected exactly one inline Tuicore path dependency")
    manifest_path.write_text(manifest)
    # Changing sources requires resolving the workspace before a package can be targeted.
    subprocess.run(["cargo", "update", "--workspace"], cwd=root, check=True)
    lock = tomllib.loads((root / "Cargo.lock").read_text())
    packages = [package for package in lock["package"] if package["name"] == "tuicore"]
    if (len(packages) != 1
            or packages[0]["version"] != version
            or packages[0].get("source") != "registry+https://github.com/rust-lang/crates.io-index"
            or not packages[0].get("checksum")):
        raise ValueError("CI requires the selected Tuicore version and checksum from crates.io")
    print(f"CI dependency: tuicore {version} (crates.io); Cargo.toml and Cargo.lock are CI-only edits")


if __name__ == "__main__":
    try:
        prepare(Path(__file__).resolve().parent.parent)
    except (ValueError, OSError, KeyError, subprocess.CalledProcessError) as error:
        sys.exit(f"error: {error}")
