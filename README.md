# Finery

Keyboard-first Jira backlog and Composer with optional MCP access.

## Development

```bash
# TUI only
cargo run

# TUI + HTTP MCP at http://127.0.0.1:7347/mcp
cargo run -- dev

# stdio MCP only
cargo run -- mcp

# foreground HTTP MCP only
cargo run -- serve
```

MCP client configuration:

```json
{"type":"remote","url":"http://127.0.0.1:7345/mcp","enabled":true}
```

Set `FINERY_DATABASE_URL` to use another SQLite database or Postgres. Otherwise Finery uses its default local SQLite database. Default data locations follow each platform: `$XDG_DATA_HOME/finery` (or `~/.local/share/finery`) on Linux, `~/Library/Application Support/finery` on macOS, and the local application-data directory on Windows. `XDG_DATA_HOME` must be absolute.

Jira credentials and defaults are configured in the TUI settings and stored in Finery's local database. `JIRA_BASE_URL`, `JIRA_EMAIL`, `JIRA_API_TOKEN`, `JIRA_DEFAULT_PROJECT`, and `JIRA_DEFAULT_BOARD` override their matching settings for the current process.

## Install

On another machine:

```bash
cargo install finery --locked
```

Update later:

```bash
cargo install finery --locked --force
```

## Build from source

```bash
cargo test
cargo build --release
cargo install --path .
```

After installation:

```bash
finery             # TUI
finery mcp         # stdio MCP
finery dev         # TUI + HTTP MCP on development port 7347
finery serve       # foreground HTTP MCP
```

Installed MCP client configuration:

```json
{"command":"finery","args":["mcp"]}
```

## Persistent MCP service

```bash
finery service install
finery service start
finery service stop
finery service uninstall
```

Service lifecycle supports Linux systemd-user and macOS launchd. HTTP stays loopback-only. Run `finery --help` for details. `service install` snapshots the current `FINERY_DATABASE_URL` into an owner-readable service definition (mode `0600`) so background service and interactive clients use the same local state. Re-run install after changing the database URL. If the variable is unset during install, the service uses the normal default local SQLite path.

## Release

Requires a clean Git tree and crates.io credentials from `cargo login`. Run `cargo patch`, `cargo minor`, or `cargo major`. The release workflow checks crates.io for the latest stable Tuicore and whether the release version is available, updates the dependency and lockfile when needed, then runs tests, package validation, and publish dry-run. After validation it shows exact versions and asks once before commit, tag, and live publish. It never pushes; follow the printed push commands after successful publication.
