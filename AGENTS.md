Load architecture.md before starting any work.
Always load ~/dev/tuicore/SKILL.md on startup.
No PRODUCT.md or DESIGN.md required.
Fix build warnings when encountered, including warnings from local path dependencies such as tuicore; do not treat a warning-producing build as clean verification.
Cargo commands inherit `RUST_BACKTRACE=full` from `.cargo/config.toml`; inspect and report complete backtraces for development crashes.
Crash logs (TUI and MCP): `$XDG_STATE_HOME/finery/logs` (default `~/.local/state/finery/logs`) or macOS `~/Library/Application Support/finery/logs`; entries include PID and mode. For Linux services also check `journalctl --user -u finery-mcp.service -e`.
Read docs/jira-description-support.md before changing Jira description conversion or overwrite safety.
Keep the MCP `get_change_set_guidance` content current when changing Jira ADF conversion or overwrite safety.

Ticket content is rendered through the shared `components/work_item_rows` model and `ticket_summary_text()` renderer. Use it across Recent Tickets, Jira search, Backlog, Composer, and ticket menus. Surface-specific list behavior, layout, and annotations are allowed, but shared ticket details should remain consistent.
