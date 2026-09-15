import importlib.util
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

SCRIPTS = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("prepare_installer", SCRIPTS / "prepare-installer.py")
packaging = importlib.util.module_from_spec(spec)
spec.loader.exec_module(packaging)
HOOKS = (SCRIPTS / "installer-service.sh").read_text()

INSTALLER = '''#!/bin/sh
set -u
APP_NAME="__PACKAGE__"
ensure() { "$@" || exit 1; }
ignore() { "$@" || :; }
mv() {
    printf 'swap\\n' >> "$TEST_LOG"
    [ "${TEST_FAIL_SWAP:-0}" = 0 ] || return 1
    if [ "${TEST_INTERRUPT:-0}" = 1 ]; then kill -TERM $$; sleep 1; fi
    command mv "$@"
}
download_binary_and_run_installer() {
    [ "${1:-}" != --help ] || { printf 'installer help\\n'; exit 0; }
    printf 'download\\n' >> "$TEST_LOG"
    [ "${TEST_FAIL_DOWNLOAD:-0}" = 0 ] || return 1
    printf 'verified\\n' >> "$TEST_LOG"
    _install_dir="$TEST_INSTALL_DIR"
    _install_temp="$TEST_STAGE"
    _lib_install_temp="$TEST_LIB_STAGE"
    _bins="$TEST_APP"
    for _bin_name in $_bins; do
        ensure mv "$_install_temp/$_bin_name" "$_install_dir"
    done
    ignore rm -rf "$_install_temp" "$_lib_install_temp"
}
download_binary_and_run_installer "$@" || exit 1
'''

SYSTEMCTL = '''#!/bin/sh
printf '%s\\n' "$2" >> "$TEST_LOG"
IFS= read -r state < "$TEST_STATE"
case "$2" in
    show)
        [ "$3" = "$TEST_APP-mcp.service" ] || exit 99
        [ "$state" != unavailable ] || exit 1
        case "$4" in
            --property=ActiveState) printf '%s\\n' "$state" ;;
            --property=MainPID) printf '777\\n' ;;
            *) exit 99 ;;
        esac ;;
    stop)
        printf 'inactive\\n' > "$TEST_STATE"
        [ "${TEST_FAIL_STOP:-0}" = 0 ] || exit 1 ;;
    start)
        [ "${TEST_FAIL_START:-0}" = 0 ] || exit 1
        printf 'active\\n' > "$TEST_STATE"
        touch "$TEST_STARTED" ;;
    is-active) [ "$state" = active ] ;;
    *) exit 99 ;;
esac
'''

READLINK = '''#!/bin/sh
case "$1" in
    /proc/*/exe)
        path="${TEST_RUNNING_PATH:-$TEST_INSTALL_DIR/$TEST_APP}"
        if [ "${TEST_DELETED:-0}" = 1 ] && [ ! -e "$TEST_STARTED" ]; then path="$path (deleted)"; fi
        printf '%s\\n' "$path" ;;
    *) exit 99 ;;
esac
'''


def executable(path, source):
    path.write_text(source)
    path.chmod(0o755)


class InstallerTests(unittest.TestCase):
    def run_install(self, app="finery", state="active", same=False, fresh=False, args=(), **overrides):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            destination = root / "install path"
            stage, lib_stage, commands = root / "stage", root / "lib-stage", root / "commands"
            for path in (destination, stage, lib_stage, commands):
                path.mkdir()
            new = '#!/bin/sh\nprintf "new version\\n"\n'
            old_version = '#!/bin/sh\nprintf "old version\\n"\n'
            old = new if same else old_version
            if not fresh:
                executable(destination / app, old)
            executable(stage / app, new if not overrides.pop("bad_binary", False) else "#!/bin/sh\nexit 1\n")
            executable(commands / "systemctl", SYSTEMCTL)
            executable(commands / "readlink", READLINK)
            live = new if same and (overrides.get("TEST_DELETED") != "1" or overrides.get("TEST_RUNNING_IDENTICAL") == "1") else old_version
            executable(root / "running-binary", live)
            executable(commands / "cmp", '#!/bin/sh\ncase "$3" in /proc/*/exe) exec /usr/bin/cmp "$1" "$2" "$TEST_RUNNING_BINARY" ;; *) exec /usr/bin/cmp "$@" ;; esac\n')
            state_path, log = root / "state", root / "log"
            state_path.write_text(state + "\n")
            log.touch()
            package = "finery" if app == "finery" else "tuitodo"
            installer = root / "installer.sh"
            installer.write_text(packaging.prepare(INSTALLER.replace("__PACKAGE__", package), HOOKS, app))
            env = {**os.environ, "PATH": f"{commands}:{os.environ['PATH']}", "TEST_APP": app,
                   "TEST_INSTALL_DIR": str(destination), "TEST_STAGE": str(stage), "TEST_LIB_STAGE": str(lib_stage),
                   "TEST_STATE": str(state_path), "TEST_LOG": str(log), "TEST_STARTED": str(root / "started"),
                   "TEST_RUNNING_BINARY": str(root / "running-binary"),
                   f"{app.upper()}_NO_SERVICE_RESTART": "0", **overrides}
            result = subprocess.run(["sh", str(installer), *args], env=env, capture_output=True, text=True, timeout=15)
            return result, log.read_text().splitlines(), (destination / app).read_text() if (destination / app).exists() else "", state_path.read_text().strip()

    def test_active_service_stops_after_verification_and_starts_after_swap(self):
        for app in ("finery", "tuido"):
            with self.subTest(app=app):
                result, events, binary, state = self.run_install(app)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertLess(events.index("verified"), events.index("stop"))
                self.assertLess(events.index("stop"), events.index("swap"))
                self.assertLess(events.index("swap"), events.index("start"))
                self.assertEqual(events.count("start"), 1)
                self.assertIn("new version", binary)
                self.assertEqual(state, "active")
                self.assertIn("Stdio MCP", result.stderr)

    def test_inactive_failed_unavailable_and_fresh_installs_do_not_start_services(self):
        for state, fresh in [("inactive", False), ("failed", False), ("unavailable", False), ("inactive", True)]:
            with self.subTest(state=state, fresh=fresh):
                result, events, binary, final_state = self.run_install(state=state, fresh=fresh)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertNotIn("stop", events)
                self.assertNotIn("start", events)
                self.assertEqual(final_state, state)
                self.assertIn("new version", binary)

    def test_restart_depends_on_running_and_installed_binary_contents(self):
        result, events, _, _ = self.run_install(same=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn("stop", events)
        result, events, _, _ = self.run_install(same=True, TEST_DELETED="1", TEST_RUNNING_IDENTICAL="1")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn("stop", events)
        result, events, _, _ = self.run_install(same=True, TEST_DELETED="1")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("stop", events)
        self.assertIn("start", events)

    def test_other_install_path_and_opt_out_leave_running_service_untouched(self):
        for options in [{"TEST_RUNNING_PATH": "/other/finery"}, {"FINERY_NO_SERVICE_RESTART": "1"}]:
            result, events, _, state = self.run_install(**options)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertNotIn("stop", events)
            self.assertNotIn("start", events)
            self.assertEqual(state, "active")

    def test_download_validation_and_transition_failures_do_not_stop_services(self):
        for options in [{"TEST_FAIL_DOWNLOAD": "1"}, {"bad_binary": True}, {"state": "activating"}]:
            result, events, binary, _ = self.run_install(**options)
            self.assertNotEqual(result.returncode, 0)
            self.assertNotIn("stop", events)
            self.assertNotIn("start", events)
            self.assertIn("old version", binary)

    def test_stop_swap_and_interruption_failures_restore_running_service(self):
        for option in ("TEST_FAIL_STOP", "TEST_FAIL_SWAP", "TEST_INTERRUPT"):
            with self.subTest(option=option):
                result, events, binary, state = self.run_install(**{option: "1"})
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("old version", binary)
                self.assertEqual(events.count("start"), 1)
                self.assertEqual(state, "active")

    def test_failed_restart_is_reported_without_binary_or_database_rollback(self):
        result, events, binary, state = self.run_install(TEST_FAIL_START="1")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(events.count("start"), 1)
        self.assertIn("new version", binary)
        self.assertEqual(state, "inactive")
        self.assertIn("Could not start", result.stderr)

    def test_help_and_template_validation_prevent_side_effects(self):
        result, events, _, _ = self.run_install(args=("--help",))
        self.assertEqual(result.returncode, 0)
        self.assertEqual(events, [])
        source = INSTALLER.replace("__PACKAGE__", "finery")
        for invalid in [source.replace(packaging.MOVE, ""), source.replace(packaging.CLEANUP, ""), source.replace(packaging.ENTRY, ""), source.replace('APP_NAME="finery"', 'APP_NAME="other"'), packaging.prepare(source, HOOKS, "finery")]:
            with self.assertRaises(ValueError):
                packaging.prepare(invalid, HOOKS, "finery")


if __name__ == "__main__":
    unittest.main()
