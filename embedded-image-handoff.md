# Embedded Jira Image Handoff

Date: 2026-09-25

## Status

Embedded Jira attachment images render correctly in ticket comments. While the user scrolls, direct-Kitty images disappear; they are redrawn about 80 ms after scrolling becomes idle. This fallback is acceptable for now.

The eventual target is seamless scrolling: visible images move and clip with comment content without disappearing, leaving stale placements, slowing input, or hanging the terminal.

## Why the fallback exists

Finery runs inside Zellij. Updating direct-Kitty placements while a scroll animation is active produced severe lag, visual glitches, and terminal hangs. Zellij issue [#5573](https://github.com/zellij-org/zellij/issues/5573) describes related failures around repeated Kitty placement updates and deletion with reused image and placement IDs. Treat this as the leading compatibility hypothesis, not a proven root cause.

The current fallback removes direct-Kitty graphics from the desired graphics frame while either relevant scroll container is moving. The renderer deletes the active image. When the debounce expires, a redraw registers, transmits, and places the image again at its settled position.

## Current implementation

### Finery

`src/components/ticket_content/mod.rs` owns shared ticket text and embedded-image rendering.

- `TicketDocument` converts parsed ticket blocks into Markdown and image nodes.
- `InlineImage` loads Jira attachment bytes on a background thread through `AppService::load_jira_attachment_image`.
- A ready image uses `ImageProtocol::Kitty`, is sized in terminal cells, and is preloaded.
- `DIRECT_KITTY_SCROLL_PAUSE` is 80 ms.
- `TicketContent` enables `ScrollContainer::pause_direct_kitty_while_scrolling`.

`src/pages/backlog/page.rs` owns the outer `TicketCommentsPane` scroll container. Both construction and rebuild paths enable the same 80 ms direct-Kitty pause. The inner and outer containers both need the policy because either can move the embedded image.

### tuicore

`src/components/scroll_container.rs` provides the opt-in builder:

```rust
.pause_direct_kitty_while_scrolling(Duration::from_millis(80))
```

Accepted scroll movement arms the pause. Active smooth scrolling keeps resetting the debounce. The final debounce tick clears suppression and requests a redraw.

`src/overlay.rs` carries a nested direct-Kitty suppression scope through normal rendering and deferred portal rendering. `register_direct_kitty` ignores both image residency and placement intents while suppression is active.

The same render context also transforms and clips direct-Kitty placements through nested scroll viewports. `src/components/image.rs` supplies source-crop metadata. `src/runtime/renderer.rs` reconciles resident image payloads and visible placement intents into transmit, place, placement-delete, and image-delete commands.

`src/node.rs` and `src/runtime/dispatcher.rs` allow focus-driven scroll reveals to request ticks, so the debounce also completes after navigation reveals rather than only mouse or keyboard scroll events.

## Relevant tests

tuicore:

- `scrolling_pauses_direct_kitty_until_movement_is_idle_for_the_debounce`
- `direct_kitty_intents_use_nested_scroll_transforms_and_clips`
- `direct_kitty_suppression_nests_and_propagates_to_portals`
- Renderer reconciliation tests in `src/runtime/renderer.rs`

Finery:

- `loaded_inline_images_use_and_preload_the_direct_kitty_protocol`
- Backlog comment-pane render and interaction coverage in `src/pages/backlog/tests/mod.rs`

Verification at handoff:

```text
tuicore: 1,776 tests passed
Finery:    508 tests passed
cargo check: clean in both repositories
cargo fmt --check: clean in both repositories
git diff --check: clean in both repositories
```

Run from each repository before continuing:

```bash
rtk cargo test
rtk cargo check
rtk cargo fmt --check
rtk git diff --check
```

## Repository state

Finery repository: `/home/marlo/dev/finery`

tuicore repository: `/home/marlo/dev/tuicore`

The work is uncommitted in both repositories. The Finery working tree contains other Jira description, ticket-row, backlog, Composer, and store work. `src/components/ticket_content/` is currently untracked as a directory. Do not reset, clean, or revert either working tree to isolate this feature.

The handoff was recorded from these repository heads:

```text
finery:  ee02708 release: v0.27.11
tuicore: 62cd05e release: v0.40.10
```

## Next investigation

Keep the pause behavior as the safe fallback. Develop seamless movement as a backend capability rather than weakening the fallback globally.

1. Build a minimal tuicore reproduction with one preloaded direct-Kitty image inside one vertical `ScrollContainer`; compare direct Kitty with Kitty-inside-Zellij using identical scroll input.
2. Capture emitted Kitty command sequences and timing for transmit, place, placement delete, and image delete. Determine which command or identity reuse causes the latency or hang.
3. Test fresh placement IDs for each moved placement while retaining a stable transmitted image ID. Explicitly bound and delete old placements to avoid terminal-side leaks.
4. Retest against the current Zellij release and check the upstream issue before adding protocol-specific machinery.
5. Add a renderer capability or strategy only after a reliable sequence exists. Preserve the current pause policy for multiplexers or versions that cannot move placements safely.

## Completion criteria

- An image remains visible while its comment scrolls.
- Input and animation stay responsive during rapid wheel and keyboard scrolling.
- No stale, duplicated, or ghost image placements remain.
- Images clip correctly at nested viewport edges.
- Settled images appear at the correct cell position and dimensions.
- Portal rendering and nested suppression remain correct.
- Direct terminal behavior remains unchanged.
- The full Finery and tuicore test suites, checks, formatting, and diff checks pass without warnings.
