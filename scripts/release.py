#!/usr/bin/env python3
"""Queue a versioned release; GitHub Actions owns validation and publication."""

import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import tomllib
import urllib.error
import urllib.request


def run(*args, **kwargs):
    return subprocess.run(args, check=True, text=True, **kwargs)


def output(*args):
    return run(*args, stdout=subprocess.PIPE).stdout.strip()


def registry_version(crate, version):
    request = urllib.request.Request(
        f"https://crates.io/api/v1/crates/{crate}/{version}",
        headers={"User-Agent": "finery-release"},
    )
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            return json.load(response)["version"]
    except urllib.error.HTTPError as error:
        if error.code == 404:
            return None
        raise


def next_version(version, bump):
    if not re.fullmatch(r"(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)", version):
        raise ValueError("Finery version must be a stable X.Y.Z version")
    parts = list(map(int, version.split(".")))
    index = {"major": 0, "minor": 1, "patch": 2}[bump]
    parts[index] += 1
    parts[index + 1:] = [0] * (2 - index)
    return ".".join(map(str, parts))


def registry_cargo(*args):
    # Local Cargo patches must never select the source used by CI releases.
    manifest = str(Path("Cargo.toml").resolve())
    with tempfile.TemporaryDirectory(prefix="finery-release-cargo-") as directory:
        home = Path(directory)
        registry = Path(os.environ.get("CARGO_HOME", Path.home() / ".cargo")) / "registry"
        if registry.is_dir():
            (home / "registry").symlink_to(registry.resolve(), target_is_directory=True)
        run("cargo", args[0], "--manifest-path", manifest, *args[1:],
            cwd=home, env={**os.environ, "CARGO_HOME": str(home)})


def release(bump):
    os.chdir(Path(__file__).resolve().parent.parent)
    if output("git", "status", "--porcelain", "--untracked-files=all"):
        raise ValueError("Git working tree must be clean, including Cargo.lock and untracked files")
    branch = output("git", "symbolic-ref", "--quiet", "--short", "HEAD")
    if branch != "main":
        raise ValueError("Release from main")
    run("gh", "auth", "status", stdout=subprocess.DEVNULL)
    run("git", "fetch", "origin", "main", "--tags")
    run("git", "merge-base", "--is-ancestor", "origin/main", "HEAD")
    manifest_path = Path("Cargo.toml")
    manifest = manifest_path.read_text()
    metadata = tomllib.loads(manifest)
    old_version = metadata["package"]["version"]
    version = next_version(old_version, bump)
    tag = f"v{version}"
    if output("git", "tag", "--list", tag):
        raise ValueError(f"Tag {tag} already exists; rerun its failed workflow instead of retagging")
    if registry_version("finery", version) is not None:
        raise ValueError(f"Finery {version} already exists on crates.io")
    tuicore = metadata["dependencies"]["tuicore"]
    if not isinstance(tuicore, str) or not re.fullmatch(r"\d+\.\d+\.\d+", tuicore):
        raise ValueError("Tuicore must declare a crates.io X.Y.Z version")
    published = registry_version("tuicore", tuicore)
    if published is None or published["yanked"]:
        raise ValueError(f"Publish non-yanked Tuicore {tuicore} before releasing Finery")

    manifest_path.write_text(manifest.replace(f'version = "{old_version}"', f'version = "{version}"', 1))
    try:
        registry_cargo("update", "--workspace")
        registry_cargo("update", "-p", "tuicore", "--precise", tuicore)
        run("git", "diff", "--check")
        run("git", "add", "Cargo.toml", "Cargo.lock")
        run("git", "commit", "-m", f"release: {tag}")
        run("git", "tag", "-a", tag, "-m", f"release: {tag}")
        run("git", "push", "--atomic", "origin", "HEAD:refs/heads/main", f"refs/tags/{tag}")
    except Exception:
        print(f"Release stopped. Inspect git status, git diff, and tag {tag}; do not rerun the bump blindly.", file=sys.stderr)
        print(f"If the commit/tag are correct, retry: git push --atomic origin HEAD:refs/heads/main refs/tags/{tag}", file=sys.stderr)
        raise
    print(f"Queued {tag}: https://github.com/daure/finery/actions/workflows/release.yml")
    print("GitHub Actions runs checks, builds, and publishes; the local command does not wait.")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("bump", nargs="?", default="patch", choices=["patch", "minor", "major"])
    args = parser.parse_args()
    try:
        release(args.bump)
    except (ValueError, OSError, KeyError, subprocess.CalledProcessError) as error:
        sys.exit(f"error: {error}")


if __name__ == "__main__":
    main()
