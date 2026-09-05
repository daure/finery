# Finery Composer MCP + Jira Staging Test Cases

## Safety and operating rules

> **This Atlassian site is a staging environment.** Agents may create, update, move, transition, and delete Jira work items as required by these cases. Do not use production Jira data. Still verify every Jira write directly; an MCP success response alone is not proof.

- Give every run a marker such as `FINERY-TC-20260827-1430` in change-set names, summaries, and descriptions.
- Use a fresh change set for each P0 case. Record the full `get_change_set` view and its **inner change-set revision** before every MCP mutation.
- After every local mutation, refresh, submit, or error, reread the change set. After every Jira submit attempt, fetch every affected Jira issue directly.
- Use explicit `selected_ticket_ids` for submission. Stored UI selection must never be treated as permission to submit additional tickets.
- Run destructive Jira cases last. Record created Jira keys so they can be cleaned up afterwards.
- Do not expect a fixed revision increment for submissions containing drafts: durable create-attempt persistence may add an intermediate revision.

## Shared hierarchy fixture

Create or reuse disposable staging work items with the current run marker. Record their Jira keys.

| Alias | Kind | Parent | Summary |
|---|---|---|---|
| `E-A` | Epic | Root | `[RUN] Resilient checkout migration` |
| `E-B` | Epic | Root | `[RUN] Payment observability` |
| `S-A` | Story | `E-A` | `[RUN] Preserve cart during retry` |
| `T-A` | Task | `E-A` | `[RUN] Add idempotency ledger` |
| `B-A` | Bug | `E-A` | `[RUN] Duplicate authorization after retry` |
| `ST-A` | Sub-task | `S-A` | `[RUN] Instrument retry correlation` |

Confirm the project's actual hierarchy before testing. Common expected rules are: root may contain epics, stories, tasks, and bugs; epics may contain stories/tasks/bugs; stories/tasks/bugs may contain subtasks; subtasks cannot have children.

## Long-content fixture

Use this structure for at least one Story, Task, Bug, and Sub-task. Expand the numbered scenarios until the description is roughly 8–12 KB. Use meaningful distinct content, not lorem ipsum.

~~~~markdown
# BEGIN [RUN]

## Customer outcome

Retries preserve the basket and reuse the existing payment intent without creating a second authorization.

## Failure model

- Gateway timeout before authorization
- Gateway timeout after authorization
- Duplicate client retry using the same idempotency key
- Inventory change between the first attempt and the retry

## MIDDLE [RUN]

### Expected sequence

1. Reserve the basket using the stable cart identifier.
2. Reuse the payment intent when the request has the same idempotency key.
3. Confirm exactly one order after trusted payment confirmation.

```text
run_marker=[RUN]
authorization_count=1
order_count=1
```

## Rollout and rollback

1. Add at least eight distinct operational and customer scenarios here.
2. Include acceptance criteria, non-goals, observability, and recovery steps.

# END [RUN]

No retry may create a second authorization.
~~~~

Change one factual line near `BEGIN`, `MIDDLE`, and `END` during diff tests.

## Attachment fixture

Create local files named with the current run marker: a small UTF-8 text file, a PDF, and valid PNG and JPEG images. Record each file's byte length and SHA-256. Make the text file and PDF available from a disposable HTTPS endpoint as well. Prepare invalid fixtures: an empty file, a file larger than 5 MiB, a PNG renamed `.jpg`, and a non-image file named `.png`.

## Mermaid fixture

Prepare two valid, visibly different diagrams with the current run marker in their titles: a state diagram and a sequence diagram. Also prepare invalid inputs: blank title, blank type, blank markup, and syntactically invalid Mermaid markup. Record the active TUI theme ID before each rendered-diagram assertion. Legacy fixture data may omit both `rendered_png` and `rendered_theme`.

## Test cases

### Local MCP and revision safety

- [ ] **L-01 — Local patch is Jira-free and atomic.** Include one Jira issue and capture its Jira history. Apply a valid multi-operation patch. Then apply a patch whose final operation references a missing ticket. The valid patch persists once; the invalid patch changes neither local state nor revision. Neither action changes Jira.
- [ ] **L-02 — Every mutator rejects a stale revision.** Read revision `R`, make a separate local save to produce `R+1`, then call patch, refresh, and submit with `R`. Each returns `stale_revision`; no selection, marker, ticket, or Jira state changes. Reread and retry only with the current revision.
- [ ] **L-03 — UI and MCP do not silently overwrite each other.** Keep a change set open in the UI. Apply an MCP title edit, then edit a different field in the UI. Reread by MCP and restart Finery. Both edits survive, or the UI visibly blocks/reloads stale state. Silent loss is a failure.
- [ ] **L-04 — Explicit submit selection is authoritative.** Stage pending tickets A and B, store UI selection for B, and submit only A through MCP. Only A reaches Jira. B remains pending and selected.
- [ ] **L-05 — Invalid selection fails before Jira I/O.** Try empty, duplicate, missing, already-submitted, and retry-blocked ticket IDs. Each request fails atomically with no revision or Jira change.
- [ ] **L-06 — Mixed artifact patches remain atomic.** Apply one patch that adds a local attachment, Mermaid diagram, web link, and issue link, then ends with an invalid artifact or link operation. It fails with no revision change: no staged artifact, link, bytes, or rendered-diagram metadata appears in the updated snapshot, and Jira history is unchanged.

### New drafts, hierarchy, and display

- [ ] **H-01 — Include and render every work-item kind.** Include `E-A`, `E-B`, `S-A`, `T-A`, `B-A`, and `ST-A`. The UI renders a depth-first hierarchy. All included items are synced, not modified.
- [ ] **H-02 — Valid moves preserve unrelated structure.** In separate patches move `S-A` from `E-A` to `E-B`, `ST-A` from `S-A` to `T-A`, `T-A` to Root, and `B-A` to Root then under `E-B`. Confirm only moved rows gain a parent delta and every unrelated sibling/descendant retains its parent.
- [ ] **H-03 — Invalid moves are rejected atomically.** In a multi-operation patch, change a title then attempt an invalid move. Separately attempt a subtask to Root, a parent beneath its descendant, and a ticket beneath a missing parent. Each fails without persisting the preceding title change.
- [ ] **H-04 — Draft hierarchy submits parent-first.** Add a local epic, story under it, and subtask under that story, all with long content. Submit only the child ID. The service includes unsent ancestors, creates Epic → Story → Sub-task, replaces every local `NEW-*` parent reference with real Jira keys, and leaves unrelated drafts untouched.
- [ ] **H-05 — Draft labels are presentation only.** A pending added KAN ticket renders `A • KAN-DRAFT • To Do` while preserving its internal `NEW-*` identity. After submit it shows its actual Jira key, remains submitted, and no longer uses the draft label.

### Jira creates, updates, and field preservation

- [ ] **J-01 — Create exactly one Jira issue.** Submit a uniquely marked draft. Search Jira for the marker before and after. There is exactly one matching issue; its project, type, title, long description, parent, status, priority, and assignee match the intended draft.
- [ ] **J-02 — New-ticket statuses follow the workflow.** For each supported draft kind, choose `To Do`, `In Progress`, `In Review`, and `Done` where the UI offers them. On submission, Jira creates in its initial state then transitions to the selected status. Unsupported statuses are not offered or cause a visible pending failure, never a silent wrong status.
- [ ] **J-03 — Title-only change preserves all other fields.** Seed an issue with a non-default status, priority, assignee, parent, rich ADF description, labels/components/fix versions/custom fields where available, comment, attachment, link, and worklog. Change only the title and submit. Only the summary changes; every unrelated field and related object survives.
- [ ] **J-04 — Long description diffs are complete.** Apply the long-content fixture. Change one line around each marker. In Source, Changes, and Diff modes at narrow and wide terminal sizes, all edits have usable context; scrolling reaches `END`; no wrapping, clipping, or mode switch loses text. Submit and verify Jira preserves headings, lists, and code blocks.
- [ ] **J-05 — Status, priority, and assignee are distinct changes.** Change all three on an existing ticket. Diff distinguishes them. Submit and verify the Jira status, priority, and account ID. A failed transition/assignment remains pending and displays an error instead of silently applying a subset.
- [ ] **J-06 — Unsupported ADF is not overwritten implicitly.** Seed an issue with unsupported content such as media or underline. A title-only submission preserves that description. A staged description rewrite must be blocked or visibly fail while leaving Jira unchanged.
- [ ] **J-07 — Unmanaged Jira fields survive.** Stage a title change. Change labels, components, fix versions, or another unmanaged field directly in Jira. Submit. The local title changes, while Jira-only unmanaged fields remain intact.

### Attachments, previews, and submission

- [ ] **A-01 — Add local and HTTPS attachments without touching Jira.** On a staged existing ticket, add the text file and PNG from local paths, then add the PDF from HTTPS. Exercise inferred and explicit filename/MIME type. The patch advances once, preserves bytes and metadata in the updated snapshot, marks every new file `added`, and makes no Jira history entry. The original snapshot remains unchanged.
- [ ] **A-02 — Attachment input validation is atomic.** In separate attempts add the empty file, over-5-MiB file, a directory, a non-HTTP(S) URL, a filename with a path/control character, the renamed PNG, and the fake PNG. Each fails before persistence: the revision, attachment list, and Jira history remain unchanged. A valid non-image file may use a non-image MIME type; a valid image must have matching bytes, extension, and MIME type.
- [ ] **A-03 — Attachment snapshots and MCP reads are precise.** Read the change set, then fetch the original remote attachment and each staged file using its ticket ID, snapshot, attachment ID, and current revision. Confirm original/updated selection is exact; local bytes match their recorded SHA-256; image, UTF-8 text, and other binary responses use their intended MCP content form. Request one missing attachment alongside valid files: valid files still return, while the missing request reports its own error. Verify the 5 MiB per-file and 20 MiB batch limits fail safely.
- [ ] **A-04 — Removal and recovery distinguish local from Jira files.** Remove a locally added attachment and confirm it disappears from the updated snapshot with no Jira write. Remove a synced Jira attachment and confirm it remains in the snapshot as `deleted`; remove it again and expect rejection. Restore it and confirm it returns to `synced`. Before submission, direct Jira reads must still show the original attachment.
- [ ] **A-05 — Attachment UI actions target the selected file.** Select an added attachment, a synced attachment, and a deleted attachment in turn. `Ctrl+X` shows only the applicable Remove/Delete/Restore action, never Open; successful actions close the dialog. `Ctrl+R` on a deleted attachment opens Restore rather than “No ticket is selected”, restores it, and closes the dialog. `Ctrl+Enter` remains the explicit open-file shortcut. Verify the row badge and detail pane reflect `A`, `S`, and `D` states, and a local-file rename persists after navigation and restart.
- [ ] **A-06 — Submit attachment additions and deletions exactly once.** Submit an existing ticket with one staged addition and one staged deletion, then fetch Jira directly. The deletion is absent; the addition has the intended filename, MIME type, byte length, and SHA-256. The ticket is reconciled as submitted and a later submit does not upload/delete again. Repeat with a draft ticket: Jira creates the ticket before its attachment upload, and the uploaded file matches the source.
- [ ] **A-07 — Attachment failures do not falsely reconcile.** Use a controlled invalid upload or fault injector to fail an attachment upload after other ticket updates are possible. The change set surfaces the failure and does not mark the ticket submitted. Fetch Jira before recovery, record which attachment operations landed, and do not blindly retry a possibly completed deletion or upload.

### Mermaid diagrams, previews, and submission

- [ ] **M-01 — Add a local diagram with rendered metadata.** Add each valid Mermaid fixture through one atomic MCP patch. The patch advances once, keeps title, type, and markup only in the local updated snapshot, records `rendered=true` and the active `rendered_theme`, and makes no Jira write. The original snapshot remains unchanged.
- [ ] **M-02 — Mermaid validation and updates are atomic.** In separate patches try every invalid Mermaid fixture. Also put a valid title or attachment update before an invalid diagram operation. Each failure leaves the revision, existing diagram title/type/markup, rendered state/theme, attachment bytes, and Jira history unchanged. A title-only update preserves the existing rendered PNG/theme; a markup update replaces the rendered PNG and records the current theme.
- [ ] **M-03 — Missing and stale renders recover locally.** Load a legacy diagram with no PNG/theme, then open the change set in the TUI. It renders, displays, and persists a PNG with the active theme without Jira I/O. Change the TUI theme, verify the preview rerenders and the stored theme changes, then restart Finery and confirm the current-theme PNG loads without a fresh render. `Ctrl+Enter` opens the current PNG in the external image program.
- [ ] **M-04 — Diagram removal is local and selection stays valid.** Remove a selected diagram through MCP and confirm it disappears from the updated snapshot with no Jira write. In the TUI, remove a selected diagram through its confirmation dialog; the dialog closes and focus moves to the next DataView row, or the previous row when no next row exists.
- [ ] **M-05 — Submit diagrams as PNG attachments exactly once.** Submit a draft and an existing ticket containing diagrams. Jira receives one PNG per diagram with the sanitized title filename and valid PNG bytes; title, type, markup, and render-theme metadata never appear as Jira fields or description text. Re-fetch Jira after submission and confirm a later submit cannot upload the diagrams again.

### Web and issue links

- [ ] **K-01 — Link edits stage atomically and preserve diffs.** On an existing staged ticket, add a web link and directional issue links using both `blocks` and `is blocked by`; then edit and remove one of each. Confirm every valid local patch advances once with no Jira write, Diff shows additions/removals distinctly, and an invalid final link operation rolls back all preceding link changes. The issue-link target selector excludes the current ticket and Jira search results use the shared two-line ticket summary.
- [ ] **K-02 — Submit links exactly once and preserve unrelated links.** Seed Jira with an unrelated web link and issue link. Submit staged web-link and issue-link additions, replacements, and removals for both an existing ticket and a draft. Fetch Jira directly: the requested directional relationships and URLs match, unrelated links remain, and a later submit creates, updates, or deletes no link again.

### Refresh and concurrency

- [ ] **R-01 — Refresh is read-only and atomic.** Include two issues. Edit one remotely; delete or make the other unreadable. Refresh must fail as a whole: no partial local baseline update, no Jira write, and no revision change. Repeat with one healthy issue: refresh advances once, updates `original`, and leaves `updated` absent.
- [ ] **R-02 — Refresh preserves local intent and remote-only fields.** Locally edit only a title. In Jira, independently change description, priority, assignee, status, and parent. Refresh, inspect Diff, then submit. Expected: local title applies; all remote-only changes survive. Treat any remote-field reversion as P0 data loss.
- [ ] **R-03 — Remote tracked-field conflict prevents all writes.** Stage changes to two existing tickets. Change a tracked field directly in Jira on one. Submit both without refresh. The result is conflict; neither Jira issue receives Finery's pending changes. Refresh and retry intentionally.
- [ ] **R-04 — Fresh baseline retry works.** Local title becomes `LOCAL [RUN]`; direct Jira title becomes `REMOTE [RUN]`; first submit conflicts. Refresh must make Diff show `REMOTE → LOCAL`; a retry with the new revision succeeds and Jira ends at `LOCAL [RUN]`.

### Deletion, recovery, and partial outcomes

- [ ] **D-01 — Stage deletion is local until submit.** Stage a disposable Jira issue for deletion and verify it still exists in Jira. Submit it, then direct GET must return 404. A remotely changed issue staged for deletion must conflict and remain in Jira.
- [ ] **D-02 — Local removal differs from Jira deletion.** Add a draft story and draft subtask plus an unrelated draft. Remove the local subtree. Both subtree drafts disappear locally, the unrelated draft remains, and Jira receives no write. `stage_jira_deletion` must reject a local-only draft.
- [ ] **D-03 — Delete children before parents.** Stage a disposable parent and child for deletion, submit in reverse input order, and verify Jira deletes the child first. Both changes reconcile as submitted.
- [ ] **D-04 — Restore/reset behaves safely.** Stage a remote ticket for deletion, restore it, modify it, reset it, and stage deletion again. Restore returns it to synced/modified as appropriate; reset returns it to the refreshed baseline; no Jira write occurs before submit.
- [ ] **D-05 — Submitted tickets are immutable.** After a successful submit, try edit, move, delete-stage, sync, reset, refresh, and resubmit. Each is rejected; Jira receives no second operation.
- [ ] **D-06 — Real partial batch reconciles per ticket.** Submit a valid parent draft with a deterministically invalid child draft. The parent receives one Jira key and is submitted; child remains pending and can be repaired/retried without recreating the parent.
- [ ] **D-07 — Ambiguous create never duplicates.** Only with a controllable proxy/fault injector: interrupt the create response after Jira may accept it. Restart Finery and retry. The durable create marker blocks blind retry; Jira contains zero or one matching issue, never two. Reconcile manually before cleanup.
- [ ] **D-08 — Ambiguous update/delete requires Jira reconciliation.** Only with fault injection: lose the response after an update/delete may land. The ticket is never falsely marked submitted. Fetch Jira directly before any recovery action; do not retry a destructive operation blindly.

### Change-set lifecycle and UI visibility

- [ ] **C-01 — Lifecycle persists through navigation and restart.** Create a set in the UI, add a hierarchy, leave and reopen it, then restart Finery. Tickets, hierarchy, selection, and diffs persist. Leaving the editor does not close the set.
- [ ] **C-02 — Local change-set deletion is confirmed.** Cancel deletion once and confirm nothing changed. Then confirm deletion and verify only local snapshots disappear; Jira issues remain.
- [ ] **C-03 — Fully submitted set closes.** Submit every pending ticket. The set reports `closed=true`, clears selection, appears closed in the list, and rejects patch, refresh, and submit mutations.

## Required direct Jira verification after every P0 case

- [ ] Fetch every affected issue with fields for summary, description, issue type, parent, status, priority, assignee, project, and supported extra fields.
- [ ] Inspect Jira history for unexpected updates or transitions.
- [ ] Search every unique create marker and assert zero or one match.
- [ ] Fetch every unselected issue separately and prove it was untouched.
- [ ] For every attachment case that submits, fetch Jira attachment metadata and content directly. Confirm each expected filename, MIME type, byte length, and checksum; confirm every staged deletion is absent.
- [ ] On any timeout, partial response, conflict, or persistence error, stop and reconcile from Jira before retrying.

## Deliberate non-goals

Do not multiply cases across every issue-kind permutation, every theme, every terminal width, every keybinding, or exact error text. These cases cover each distinct hierarchy, persistence, refresh, submission, recovery, and data-integrity branch once.
