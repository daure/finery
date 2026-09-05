# Composer selection model

Composer ticket checkboxes show effective submission scope. Finery separately
records explicit intent so Jira hierarchy can never create a downward cascade.

## Three submission sets

- **Explicit** — ticket IDs selected in the TUI, or supplied by the current MCP
  `submit_change_set` call.
- **Required** — pending, unsent `NEW-*` ancestors that Jira requires Finery to
  create before an explicitly selected draft.
- **Effective** — the parent-first union of explicit and required tickets that
  the submission planner may send to Jira.

`ChangeSet::selected_ticket_ids` persists only explicit TUI intent. An MCP submit
uses only the IDs in that invocation; it never inherits the stored TUI list. The
sole implicit expansion is upward through required unsent draft ancestors.

## TUI behavior

- Selecting a parent selects only that ticket.
- Selecting an existing child does not select its Jira parent.
- Any subset of existing parents, children, or siblings may be selected.
- Selecting a draft child automatically checks each unsent draft ancestor that
  is required for submission, without making those ancestors explicit intent.
- A required ancestor cannot be unchecked while a selected descendant depends
  on it. The TUI restores the check and shows a warning explaining what to
  deselect first.
- A collapsed branch reports how many explicitly selected tickets it hides.
- Attachment and diagram rows are navigable but cannot be selected for submit.
- The commit dialog lists the complete effective scope and identifies parents
  that were added automatically.

Checkboxes show effective commit membership, never aggregate or indeterminate
subtree state. Required provenance remains internal and is disclosed in the
commit dialog when it affects Jira scope.

## Notifications

Routine selection and derived parent inclusion do not create transient
notifications. Trying to deselect a required parent is blocked with one warning.
Other notifications are reserved for blocked or failed planning/submission actions.

The commit action is available for any nonempty explicit selection so a planning
failure can produce a useful notification. Structural row annotations use
explicit descendant counts for collapsed branches. Required checks use
`required_ancestor_ids` even when readiness validation fails. Commit previews,
confirmation revalidation, durable claims, and MCP submissions use
`ComposerState::submission_plan` so effective scope cannot drift between clients.
