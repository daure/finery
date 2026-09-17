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

## Ticket open command

**Open command** in Settings stores a trusted host shell command. Press `Ctrl+;` on a ticket in Jira Search (`Ctrl+F`), Recent Tickets (`Ctrl+E`), Backlog, or change-set detail to run it in the background through `sh -c`. An empty or whitespace-only command does nothing. Saving the setting does not execute it.

The command inherits Finery's working directory and receives `FINERY_TICKET_KEY` and `FINERY_TICKET_URL` as environment variables. Quote these variables, for example `my-ticket-tool "$FINERY_TICKET_KEY"`. The URL is empty when no Jira base URL is configured. Local drafts and non-ticket rows do nothing. Jira Search and Recent Tickets close when a command is triggered; a blank command leaves them open. An **Open command started** notification identifies the ticket after the shell launches. Commands run with your local user privileges; launch and nonzero-exit failures appear in Finery.

The Backlog `.` menu includes **Open command** with its configured shortcut. Choose that action or press `Ctrl+;` from the menu to trigger it for the first selected ticket and close the menu.

The database settings are `tickets.open_command` and `tickets.open_command_key` (default `ctrl+;`). This shortcut requires a terminal that reports Ctrl+; distinctly. In Backlog, Enter opens the highlighted ticket's Description/Comments view; during search or reordering, Enter confirms that operation. Ctrl+Enter opens the ticket in Jira.

## Composer archives

In the Composer overview, highlight a change set and press `a` to choose **Done**, **Reject**, or **Cancel**. Done archives the set with every unsubmitted item marked **Concluded**; Reject archives it with every unsubmitted item marked **Cancelled**. Cancel dismisses the dialog. Submitted items retain their submission snapshots, and archiving only changes local Composer state.

Press `.` for searchable change-set actions. **Rename** (`r`), **Delete** (`x`), and **Archive** (`a`) open their dialogs; archived sets offer **Clone** (`c`), **Rename**, and **Delete**. Action shortcuts take precedence over menu search.

The **Archived** filter includes fully submitted sets and sets archived through Done or Reject. Archived ticket content is read-only, while their change sets can be renamed or cloned. An active or unresolved Jira submission must be resolved before archiving.

The overview lists open sets first, then archived sets by closure time, newest first. The Archived filter uses the same closure-time order. Sets without a recorded closure time appear last among archives.

The change-set action shortcuts are configurable through `composer.change_set_actions_key`, `composer.rename_change_set_key`, `composer.clone_change_set_key`, `composer.delete_change_set_key`, and `composer.archive_key` (defaults: `.`, `r`, `c`, `x`, and `a`). MCP change-set reads expose `archive_outcome` (`cancelled`, `concluded`, or null); it applies to tickets whose `submitted` flag is false.

## Install

Prebuilt releases support **Ubuntu 24.04 or newer on x86_64**. Rust is not required. Download and run the installer from the [latest GitHub Release](https://github.com/daure/finery/releases/latest):

```bash
installer="$(mktemp)"
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/daure/finery/releases/latest/download/finery-installer.sh \
  -o "$installer" && sh "$installer"
rm -f "$installer"
export PATH="${CARGO_HOME:-$HOME/.cargo}/bin:$PATH"
finery --help
```

The installer places the binary in `$CARGO_HOME/bin` (default `~/.cargo/bin`). Add that directory to your shell's PATH permanently if needed. The Releases page also provides a `.tar.xz` archive and SHA-256 checksum for manual installation; the latest 30 stable releases remain available for rollback.

### Update

Close running Finery TUI sessions, then rerun the installer commands above to download the latest stable binary. On Ubuntu, the installer automatically stops an already-running `finery-mcp.service` after download, verification, and staging, then starts it after replacing the executable. It checks that the service is active and uses the installed executable. Identical installations skip an unnecessary restart; inactive, failed, or uninstalled services stay as they are. The installer preserves the service definition, enablement, credentials, and database URL.

Automatic restart applies only when the service uses the destination executable. Services using another path are left untouched with a warning; unavailable user systemd also produces a manual-restart notice. Set `FINERY_NO_SERVICE_RESTART=1` on the installer process to disable service management. If installation is interrupted after stopping a service, the installer attempts to start it again; a restart failure returns an error with recovery guidance. It does not roll back binaries or database migrations automatically.

Stdio MCP servers are owned by the MCP client: reconnect them after updating; the installer prints a reminder and does not kill them. Your settings and database stay in their normal data directories; back up the database before an upgrade or rollback because older binaries may not support newer database schemas.

## Build from source

Install Rust, then clone and build the repository for your platform:

```bash
git clone https://github.com/daure/finery.git
cd finery
cargo test --locked
cargo install --path . --locked
```

For source-based updates, run these commands inside the checkout:

```bash
git pull --ff-only
cargo install --path . --locked --force
```

Source-based installations and manual binary copies require manual MCP service restarts.

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

From `main` with committed source changes, a sibling `../tuicore` checkout, Git push access, Python 3.11+, Rust, and an authenticated GitHub CLI (`gh auth login`):

```bash
cargo release          # patch: 0.27.0 -> 0.27.1
cargo release minor    # minor: 0.27.0 -> 0.28.0
cargo release major   # major: 0.27.0 -> 1.0.0
# Equivalent command without compiling the tiny Cargo helper:
./scripts/release.sh patch
```

The command checks the branch and local Tuicore path, then runs the release workflow's formatting, script tests, Clippy, and Rust tests before changing the version. It then bumps Finery, refreshes and commits the local lockfile, creates an annotated tag without opening an editor, and atomically pushes `main` and its `vX.Y.Z` tag. Generated `Cargo.lock` changes are accepted; all other files must be clean. It returns without waiting for GitHub Actions. Existing `cargo patch`, `cargo minor`, and `cargo major` aliases use the same release flow.

GitHub Actions runs `scripts/prepare_ci.py` before Cargo checks and builds. It selects the highest stable, non-yanked Tuicore version on crates.io across all major versions, pins that exact version in the CI manifest, and resolves its registry lockfile. Checks and distribution builds use that resolution. These edits remain in the disposable CI checkout. The selected version is printed in the workflow log; rerunning a workflow can select a newer published version. Registry or compatibility failures stop the build. Publish required Tuicore changes before releasing Finery.

The [Release workflow](https://github.com/daure/finery/actions/workflows/release.yml) checks formatting, runs Clippy with warnings denied, runs tests, then uses cargo-dist to build and smoke-test an Ubuntu x86_64 archive. After checks pass it publishes the GitHub Release and installer using GitHub's built-in repository token. GitHub Releases is the distribution channel for current Finery versions. All work runs in one job, with no Actions artifact uploads. Release downloads persist until deleted.

Normal pushes to `main` run the same checks and warm debug and optimized dependency caches. Tagged releases restore those caches; only `main` saves them so later tags can access them. Release-version commits skip the redundant branch build. Distribution builds disable LTO and strip symbols to favor build speed. The first build after a toolchain or dependency change can take longer. Cache storage is separate from release downloads and temporary Actions artifact storage. To warm the cache manually, run `gh workflow run release.yml --ref main`; this builds without publishing.

After a successful tagged release, a serialized retention job keeps the 30 most recently published stable releases, ordered by publication time. It deletes older GitHub release records and their downloads using the official GitHub API through `gh`, preserving all Git tags, drafts, and prereleases. Older version-specific download URLs stop working; their source tags remain available. The script checks all pages and rechecks each candidate before deletion; it refuses cleanup if the triggering release is missing or outside the retained set. This policy applies to Finery and Tuido only.

Preview cleanup without deleting anything:

```bash
python3 scripts/prune-releases.py --repo daure/finery --keep 30
```

Deletion requires both `--apply` and `--published-tag TAG`; the release workflow supplies these only after publication succeeds. Main-branch builds run the preview only.

Cleanup also refuses to delete the release designated as GitHub's **Latest**, protecting the installer URL if an older version is pinned there.

Inspect runs with `gh run list --workflow release.yml` and `gh run watch RUN_ID`. Retry a transient failure with `gh run rerun RUN_ID --failed`. For a source fix, commit the fix and run a new patch release. Tags are immutable: do not move a published tag. If the local push fails, inspect the release commit/tag and use the exact retry command printed by the script.

### Local Tuicore development

Finery declares a direct relative path dependency:

```toml
tuicore = { path = "../tuicore" }
```

Keep the repositories side by side and run `cargo run -- dev`. Local builds always compile that Tuicore working copy, regardless of its version. Cargo maintains the local lockfile automatically; `cargo release` handles it when releasing. Personal `[patch.crates-io]` overrides are unnecessary for this workflow and can produce unused-patch warnings in helper crates.
