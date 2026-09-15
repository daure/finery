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

Close running Finery sessions, then rerun the installer commands above to download the latest stable binary. If you installed the background MCP service, run `finery service stop` before updating and `finery service start` afterwards. Restart stdio MCP clients to load the updated executable. Your settings and database stay in their normal data directories; back up the database before an upgrade or rollback because older binaries may not support newer database schemas.

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

From a clean `main` checkout with Git push access, Python 3.11+, Rust, and an authenticated GitHub CLI (`gh auth login`):

```bash
cargo release          # patch: 0.27.0 -> 0.27.1
cargo release minor    # minor: 0.27.0 -> 0.28.0
cargo release major   # major: 0.27.0 -> 1.0.0
# Equivalent command without compiling the tiny Cargo helper:
./scripts/release.sh patch
```

The command checks the branch and published Tuicore dependency, bumps Finery, resolves the release lockfile against crates.io, commits, and atomically pushes `main` and its `vX.Y.Z` tag. It returns without waiting for compilation. Existing `cargo patch`, `cargo minor`, and `cargo major` aliases use the same release flow.

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

Finery declares Tuicore as a crates.io dependency. To compile against your local working copy, put this in your personal `~/.cargo/config.toml`:

```toml
[patch.crates-io]
tuicore = { path = "/absolute/path/to/tuicore" }
```

Local builds compile the working copy whenever it satisfies the dependency requirement and is selected by Cargo; `cargo update -p tuicore` selects the override when needed. This can modify `Cargo.lock`, which must be committed or deliberately restored before releasing. The release command resolves Tuicore from crates.io without the personal override. Publish required Tuicore changes first, update Finery's declared dependency version, and commit those changes before releasing. CI builds use the committed registry lockfile; users only download the compiled Finery binary.
