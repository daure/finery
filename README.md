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

Jira description conversion support and its safety contract are documented in [docs/jira-description-support.md](docs/jira-description-support.md).

## Composer archives

In the Composer overview, highlight a change set and press `Ctrl+C` to choose **Done**, **Reject**, or **Cancel**. Done archives the set with every unsubmitted item marked **Concluded**; Reject archives it with every unsubmitted item marked **Cancelled**. Cancel dismisses the dialog. Submitted items retain their submission snapshots, and archiving only changes local Composer state.

Press `.` for searchable change-set actions. **Rename** (`Ctrl+R`), **Delete** (`Ctrl+X`), and **Archive** (`Ctrl+C`) open their dialogs; archived sets also offer **Clone** (`Ctrl+O`).

The **Archived** filter includes fully submitted sets and sets archived through Done or Reject. Archived ticket content is read-only, while their change sets can be renamed or cloned. An active or unresolved Jira submission must be resolved before archiving.

The overview lists open sets first, then archived sets by closure time, newest first. The Archived filter uses the same closure-time order. Sets without a recorded closure time appear last among archives.

The change-set action shortcuts are configurable through `composer.change_set_actions_key`, `composer.rename_change_set_key`, `composer.clone_change_set_key`, `composer.delete_change_set_key`, and `composer.archive_key` (defaults: `.`, `ctrl+r`, `ctrl+o`, `ctrl+x`, and `ctrl+c`). MCP change-set reads expose `archive_outcome` (`cancelled`, `concluded`, or null); it applies to tickets whose `submitted` flag is false.

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
