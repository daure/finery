#!/usr/bin/env python3
"""Publish once, permitting retries of the same tagged workflow."""

import os
from pathlib import Path
import subprocess
import sys
import tomllib

from release import registry_version


def main():
    version = tomllib.loads(Path("Cargo.toml").read_text())["package"]["version"]
    if registry_version("finery", version) is not None:
        print(f"Finery {version} is already published; continuing this release.")
        return
    if not os.environ.get("CARGO_REGISTRY_TOKEN"):
        sys.exit("error: configure the CARGO_REGISTRY_TOKEN repository secret")
    subprocess.run(["cargo", "publish", "--locked"], check=True)


if __name__ == "__main__":
    main()
