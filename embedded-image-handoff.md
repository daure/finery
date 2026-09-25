# Embedded Jira Image Handoff

Date: 2026-09-25

## Status

Embedded Jira attachment images use renderer-owned direct-Kitty placements. Outside Zellij, scrolling updates their position and source crop on each frame. Inside Zellij, images disappear during scrolling and return about 80 ms after scrolling becomes idle. Seamless scrolling inside Zellij remains unresolved.

Double-clicking embedded images or Composer attachment previews opens the original downloaded bytes in the OS-associated image viewer. Opening runs on a background thread; file creation and launcher-spawn errors produce a notification.

The eventual target is seamless scrolling: visible images move and clip with comment content without disappearing, leaving stale placements, slowing input, or hanging the terminal.

## Why the fallback exists

The installed Zellij is 0.45.0. Continuous direct-Kitty updates have a history of severe lag, visual glitches, and terminal hangs in this setup. Source inspection and upstream reports identify three relevant defects; their contribution to Finery's hangs still needs a terminal-level reproduction:

1. **Scaled crop caching:** Zellij caches scaled images and host transmissions by destination cell dimensions, excluding the source crop. Different crops with the same dimensions can display stale pixels. Fresh placement IDs do not change that cache key. [Issue #5573](https://github.com/zellij-org/zellij/issues/5573), [fix #5623](https://github.com/zellij-org/zellij/pull/5623), open on 2026-09-25.
2. **Unscaled crop offsets:** Omitting `c` and `r` avoids Zellij's per-placement rescaling, but 0.45.0 ignores the source `x` and `y` offsets on this path. Top/left viewport clipping would show the wrong pixels. [Fix #5568](https://github.com/zellij-org/zellij/pull/5568), open on 2026-09-25.
3. **Host bandwidth:** Zellij forwards image data as uncompressed RGBA even when the application sends PNG. Retransmitting whole images to invalidate crop caches can saturate the terminal connection. [Fix #5608](https://github.com/zellij-org/zellij/pull/5608) is merged upstream; the installed 0.45.0 lacks it.

Zellij 0.45.x also rejects Kitty Unicode placeholders. The tuicore constitution forbids graphical background masks and per-cell image workarounds. Replacing the pause requires a measured, correctly clipped sequence rather than an assumption about placement-ID reuse.

The current fallback removes direct-Kitty graphics from the desired graphics frame while either relevant scroll container is moving. The renderer deletes the active image. When the debounce expires, a redraw registers, transmits, and places the image again at its settled position.

## Current implementation

### Finery

`src/components/ticket_content/mod.rs` owns shared ticket text and embedded-image rendering.

- `TicketDocument` converts parsed ticket blocks into Markdown and image nodes.
- `InlineImage` loads Jira attachment bytes on a background thread through `AppService::load_jira_attachment_image` in `src/service/attachment_images.rs`.
- A ready image uses `ImageProtocol::Kitty`, is sized in terminal cells, and is preloaded.
- `DIRECT_KITTY_SCROLL_PAUSE` is 80 ms.
- `image_scroll_container` enables `ScrollContainer::pause_direct_kitty_while_scrolling` when the `ZELLIJ` environment variable is present; other environments use normal placement updates.
- Image loading keeps the original bytes for double-click opening. The opener creates a unique private temporary file with a format-derived extension and preserves it for the asynchronously launched viewer; OS temporary-directory cleanup owns its eventual removal.
- Jira image downloads validate the configured site origin before sending authentication and have a ten-second timeout.

`src/pages/backlog/page.rs` owns the outer `TicketCommentsPane` scroll container. Construction and rebuild use the shared `image_scroll_container` policy. `TicketCommentCard` forwards routed events into its document so nested image hit regions receive clicks.

### tuicore

`src/components/scroll_container.rs` provides the opt-in builder:

```rust
.pause_direct_kitty_while_scrolling(Duration::from_millis(80))
```

Accepted scroll movement arms the pause. Active smooth scrolling keeps resetting the debounce. The final debounce tick clears suppression and requests a redraw.

`src/overlay.rs` carries a nested direct-Kitty suppression scope through normal rendering and deferred portal rendering. `register_direct_kitty` ignores both image residency and placement intents while suppression is active.

The same render context also transforms and clips direct-Kitty placements through nested scroll viewports. `src/components/image.rs` supplies source-crop metadata. `src/runtime/renderer.rs` reconciles resident image payloads and visible placement intents into transmit, place, placement-delete, and image-delete commands.

`src/node.rs` and `src/runtime/dispatcher.rs` allow focus-driven scroll reveals to request ticks, so the debounce also completes after navigation reveals rather than only mouse or keyboard scroll events.

`Image::on_double_click` registers a hit region matching the fitted image bounds and recognizes nearby left clicks within 500 ms. Wheel input bubbles to scroll containers; padding clicks are ignored. The callback is preserved by protocol selection and background encoding.

## Relevant tests

tuicore:

- `scrolling_pauses_direct_kitty_until_movement_is_idle_for_the_debounce`
- `direct_kitty_intents_use_nested_scroll_transforms_and_clips`
- `direct_kitty_suppression_nests_and_propagates_to_portals`
- Renderer reconciliation tests in `src/runtime/renderer.rs`
- Image interaction and scrolled hit-region routing in `src/components/tests/image_interaction.rs`

Finery:

- `loaded_inline_images_use_and_preload_the_direct_kitty_protocol`
- Backlog comment-pane render and interaction coverage in `src/pages/backlog/tests/mod.rs`
- Inline image double-click routing in `src/components/ticket_content/tests/mod.rs`
- Authenticated image loading and private, byte-preserving viewer files in `src/service/tests/attachment_images.rs`

Verification at handoff:

```text
tuicore: 1,779 tests passed
Finery:    532 tests passed
cargo check: clean in both repositories
cargo build: clean in Finery
cargo fmt --check: clean in both repositories
git diff --check: clean in both repositories
```

Verification is automated, not visual. The host image viewer and continuous scrolling in a real terminal have not been exercised in this pass. `lsp_diagnostics` is unavailable; compiler checks provide compile diagnostics.

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

Both working trees contain unrelated concurrent work. Inspect their current status before editing. Do not reset, clean, or revert either working tree to isolate this feature.

## Next investigation

Keep the pause behavior as the safe fallback. Develop seamless movement as a backend capability rather than weakening the fallback globally.

1. Build a minimal tuicore reproduction with one preloaded direct-Kitty image inside one vertical `ScrollContainer`; compare direct Kitty with Kitty-inside-Zellij using identical scroll input.
2. Capture emitted Kitty command sequences and timing for transmit, place, placement delete, and image delete. Determine which command or identity reuse causes the latency or hang.
3. Retest a Zellij build containing the source-crop cache, unscaled-offset, and host-compression fixes above. A version upgrade alone is insufficient evidence that all three are present.
4. Compare stable and fresh placement IDs only after crop correctness is established. Explicitly bound and delete old placements to avoid terminal-side leaks.
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
