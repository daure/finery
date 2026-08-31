Load architecture.md before starting any work.
Always load ~/dev/tuicore/SKILL.md on startup.
No PRODUCT.md or DESIGN.md required.

Ticket content is rendered through the shared `components/work_item_rows` model and `ticket_summary_text()` renderer. Use it across Recent Tickets, Jira search, Backlog, Composer, and ticket menus. Surface-specific list behavior, layout, and annotations are allowed, but shared ticket details should remain consistent.
