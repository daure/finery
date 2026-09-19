#!/usr/bin/env python3
"""Queue a versioned release; GitHub Actions owns validation and publication."""

import argparse
import os
from pathlib import Path
import re
import subprocess
import sys
import tomllib


def run(*args, **kwargs):
    environment = {
        **os.environ,
        "PAGER": "cat",
        "GIT_PAGER": "cat",
        "CARGO_PAGER": "cat",
        **kwargs.pop("env", {}),
    }
    return subprocess.run(args, check=True, text=True, env=environment, **kwargs)


def output(*args):
    return run(*args, stdout=subprocess.PIPE).stdout.strip()


def next_version(version, bump):
    if not re.fullmatch(r"(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)", version):
        raise ValueError("Finery version must be a stable X.Y.Z version")
    parts = list(map(int, version.split(".")))
    index = {"major": 0, "minor": 1, "patch": 2}[bump]
    parts[index] += 1
    parts[index + 1:] = [0] * (2 - index)
    return ".".join(map(str, parts))


def preflight_cargo(*args):
    run(
        "cargo",
        *args,
        env={
            "CARGO_TARGET_DIR": str(Path("target/release-check").resolve()),
            "CARGO_BUILD_JOBS": "2",
            "CARGO_PROFILE_DEV_DEBUG": "0",
            "CARGO_PROFILE_TEST_DEBUG": "0",
            "CARGO_INCREMENTAL": "0",
        },
    )


def preflight():
    run("cargo", "fmt", "--all", "--check")
    run(
        "cargo",
        "fmt",
        "--manifest-path",
        "tools/release-command/Cargo.toml",
        "--check",
    )
    run("python3", "-m", "unittest", "discover", "-s", "scripts/tests")
    preflight_cargo("update", "--workspace")
    preflight_cargo("clippy", "--locked", "--all-targets", "--", "-D", "warnings")
    preflight_cargo("test", "--locked", "--", "--test-threads=2")


def release(bump):
    os.chdir(Path(__file__).resolve().parent.parent)
    if output("git", "status", "--porcelain", "--untracked-files=all", "--", ".", ":(exclude)Cargo.lock"):
        raise ValueError("Git working tree must be clean except for generated Cargo.lock changes")
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
    tuicore = metadata["dependencies"]["tuicore"]
    if tuicore != {"path": "../tuicore"}:
        raise ValueError("Tuicore must use the local ../tuicore path")
    preflight()

    manifest_path.write_text(manifest.replace(f'version = "{old_version}"', f'version = "{version}"', 1))
    try:
        run("cargo", "update", "--workspace")
        run("git", "diff", "--check")
        run("git", "add", "Cargo.toml", "Cargo.lock")
        run("git", "commit", "-m", f"release: {tag}")
        run(
            "git",
            "tag",
            "-a",
            tag,
            "-m",
            f"release: {tag}",
            env={"GIT_EDITOR": "true"},
        )
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
