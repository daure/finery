"""Add tested systemd lifecycle hooks to the pinned cargo-dist shell installer."""

import argparse
from pathlib import Path

MOVE = '    for _bin_name in $_bins; do\n        ensure mv "$_install_temp/$_bin_name" "$_install_dir"\n'
CLEANUP = '    ignore rm -rf "$_install_temp" "$_lib_install_temp"\n'
ENTRY = 'download_binary_and_run_installer "$@" || exit 1\n'


def prepare(source, hooks, app):
    package = {"finery": "finery", "tuido": "tuitodo"}[app]
    if source.count(f'APP_NAME="{package}"') != 1 or "mcp_service_prepare" in source:
        raise ValueError("Expected an unmodified cargo-dist installer for the selected app")
    for anchor in (MOVE, CLEANUP, ENTRY):
        if source.count(anchor) != 1:
            raise ValueError("Unsupported cargo-dist installer layout; review lifecycle hook locations")
    hooks = hooks.replace("__MCP_APP__", app).replace("__MCP_SKIP_ENV__", f"{app.upper()}_NO_SERVICE_RESTART")
    source = source.replace(MOVE, '    mcp_service_prepare "$_install_dir" "$_install_temp" || return 1\n' + MOVE)
    source = source.replace(CLEANUP, "    mcp_service_resume || return 1\n" + CLEANUP)
    return source.replace(ENTRY, hooks + "\n" + ENTRY + "mcp_service_reminder\n")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--app", required=True, choices=["finery", "tuido"])
    parser.add_argument("installer", type=Path)
    args = parser.parse_args()
    hooks = (Path(__file__).parent / "installer-service.sh").read_text()
    args.installer.write_text(prepare(args.installer.read_text(), hooks, args.app))
