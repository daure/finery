# Embedded by prepare-installer.py after cargo-dist generates the installer.
_mcp_app='__MCP_APP__'
_mcp_unit="$_mcp_app-mcp.service"
_mcp_restart_pending=0
_mcp_binary=''

mcp_service_error() {
    printf 'error: %s\n' "$*" >&2
    return 1
}

mcp_service_pid() {
    _mcp_pid=$(timeout 30 systemctl --user show "$_mcp_unit" --property=MainPID --value) || return 1
    case "$_mcp_pid" in ''|0|*[!0-9]*) return 1 ;; esac
}

mcp_service_resume() {
    [ "$_mcp_restart_pending" = 1 ] || return 0
    _mcp_restart_pending=0
    printf 'Starting previously running %s...\n' "$_mcp_unit" >&2
    if ! timeout 30 systemctl --user start "$_mcp_unit"; then
        mcp_service_error "Could not start $_mcp_unit. Inspect it with systemctl --user status $_mcp_unit."
        return 1
    fi
    if ! timeout 30 systemctl --user is-active --quiet "$_mcp_unit" || ! mcp_service_pid; then
        mcp_service_error "$_mcp_unit is not running after the update. Inspect it with systemctl --user status $_mcp_unit."
        return 1
    fi
    if ! _mcp_running=$(readlink "/proc/$_mcp_pid/exe"); then
        mcp_service_error "Cannot verify $_mcp_unit after starting it. Inspect it with systemctl --user status $_mcp_unit."
        return 1
    fi
    if [ "$_mcp_running" != "$_mcp_binary" ]; then
        mcp_service_error "$_mcp_unit is not using $_mcp_binary; inspect its service configuration."
        return 1
    fi
    printf '%s is running with the installed executable.\n' "$_mcp_unit" >&2
}

mcp_service_on_exit() {
    _mcp_exit_status=$1
    trap - 0 HUP INT TERM
    if [ "$_mcp_restart_pending" = 1 ]; then
        printf 'Installation interrupted; restoring the previously running MCP service.\n' >&2
        if ! mcp_service_resume; then
            [ "$_mcp_exit_status" -ne 0 ] || _mcp_exit_status=1
        fi
    fi
    exit "$_mcp_exit_status"
}

mcp_service_prepare() {
    case "${__MCP_SKIP_ENV__:-0}" in
        1) printf 'Automatic MCP service restart disabled.\n' >&2; return 0 ;;
        0) ;;
        *) mcp_service_error '__MCP_SKIP_ENV__ must be 0 or 1'; return 1 ;;
    esac
    [ "$(uname -s)" = Linux ] || return 0
    if ! "$2/$_mcp_app" --version >/dev/null; then
        mcp_service_error 'Downloaded executable failed its version check; service left untouched.'
        return 1
    fi
    _mcp_directory=$(CDPATH='' cd -- "$1" && pwd -P) || return 1
    _mcp_binary="$_mcp_directory/$_mcp_app"
    for _mcp_command in flock timeout readlink cmp; do
        command -v "$_mcp_command" >/dev/null 2>&1 || {
            mcp_service_error "Required installer command is missing: $_mcp_command"
            return 1
        }
    done
    exec 9>"$_mcp_directory/.${_mcp_app}-install.lock" || return 1
    if ! flock -w 30 9; then
        mcp_service_error "Another $_mcp_app installation is running."
        return 1
    fi
    if ! command -v systemctl >/dev/null 2>&1; then
        printf 'systemctl unavailable; reconnect any MCP clients after installation.\n' >&2
        return 0
    fi
    if ! _mcp_state=$(timeout 30 systemctl --user show "$_mcp_unit" --property=ActiveState --value 2>/dev/null); then
        printf 'Cannot inspect %s; restart it manually if it is running.\n' "$_mcp_unit" >&2
        return 0
    fi
    case "$_mcp_state" in
        inactive|failed|'') return 0 ;;
        active) ;;
        *) mcp_service_error "$_mcp_unit is $_mcp_state; retry when its state is stable."; return 1 ;;
    esac
    if ! mcp_service_pid; then
        mcp_service_error "Cannot identify the running process for $_mcp_unit."
        return 1
    fi
    if ! _mcp_original_executable=$(readlink "/proc/$_mcp_pid/exe"); then
        mcp_service_error "Cannot verify the running executable for $_mcp_unit; update aborted."
        return 1
    fi
    _mcp_running=${_mcp_original_executable%" (deleted)"}
    if [ -L "$_mcp_binary" ] || [ "$_mcp_running" != "$_mcp_binary" ]; then
        printf '%s uses a different executable; its configuration and process are unchanged.\n' "$_mcp_unit" >&2
        printf 'Update that service configuration manually to use %s.\n' "$_mcp_binary" >&2
        return 0
    fi
    if cmp -s "$2/$_mcp_app" "$_mcp_binary" && cmp -s "$2/$_mcp_app" "/proc/$_mcp_pid/exe"; then
        printf 'Installed executable is identical; %s does not need restarting.\n' "$_mcp_unit" >&2
        return 0
    fi
    trap 'mcp_service_on_exit $?' 0
    trap 'exit 129' HUP
    trap 'exit 130' INT
    trap 'exit 143' TERM
    _mcp_restart_pending=1
    printf 'Stopping %s for the executable update...\n' "$_mcp_unit" >&2
    if ! timeout 30 systemctl --user stop "$_mcp_unit"; then
        mcp_service_error "Could not stop $_mcp_unit; executable replacement aborted."
        return 1
    fi
}

mcp_service_reminder() {
    printf 'Stdio MCP servers are client-managed: reconnect them in your MCP client after updating.\n' >&2
}
