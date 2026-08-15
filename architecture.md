# Ratatui TUI Architecture Guidelines

Date: 2026-06-17

## Purpose

This document defines the generic Ratatui architecture baseline for keyboard-first terminal user interfaces built on top of the `tuicore` library. It combines reusable terminal patterns with `tuicore` contracts for project layout, runtime lifecycle, state management, rendering, service boundaries, testing, and architectural invariants.

All applications adopting this baseline must adhere to these guidelines to ensure consistency, testability, and responsiveness.

## Core Invariants

Every application using this architecture must adhere to these invariants:

- **Configurable Bindings:** Runtime and component keybindings resolve through `tuicore::KeyBindings`. Built-in defaults are provided, but hard-coded-only shortcuts are prohibited. Application-specific actions may use an app-owned keymap layer, but it must be configurable and must reuse `tuicore::KeySpec` parsing/labels where possible.
- **Semantic Themes:** Every UI style and color resolves through semantic roles in `tuicore::Theme` (loaded from `tui.toml`). Per-role overrides must be supported.
- **Dynamic Help:** Shortcut help is dynamic, reflectively generated from resolved keybindings using `KeySpec::label()`. Hard-coded shortcut labels are prohibited.
- **Explicit Side Effects:** Remote writes or state modifications are explicit, user-visible actions. Destructive operations require confirmation, and failures must become visible UI states.
- **Accessible Design:** Icons enhance the visual layout, but text carries the primary meaning. Every Nerd Font icon must have a clear ASCII/Unicode fallback or be omitted when no fallback exists.

## Design Goals

A well-structured application built on this architecture must be:

- **Recoverable:** Terminal state is restored on normal exit and drop paths through `tuicore`'s runner and `TerminalGuard`. Panic and signal restoration require application-level hooks unless provided by the active `tuicore` version.
- **Responsive:** Rendering and input processing stay responsive on the main thread, while blocking I/O and remote service calls run asynchronously in the background.
- **Testable:** State transitions, event handler routines, and screen layouts can be verified programmatically without requiring a physical terminal.
- **Separable:** UI components do not own business rules, protocol mappings, database/network I/O, or persistent storage.
- **Navigable:** Codebase boundaries are explicit, separating state reduction, effect execution, layout structure, and external services.
- **Stable under refresh:** Active list selections, focused nodes, and scroll positions must survive sorting, filtering, pagination, or snapshot updates.

## Core Architecture Principles

### 1. Centralize Terminal Lifecycle

Terminal initialization and cleanup are managed by `tuicore::run` or `tuicore::TreeApp`. Applications must not scatter raw crossterm raw-mode, alternate-screen, or event-loop logic.

The runtime handles:
- Entering and exiting alternate screen;
- Enabling and disabling raw mode;
- Mouse capture and paste mode configuration;
- Drop-based terminal restoration through `TerminalGuard`;
- Configurable runtime quit handling through `tuicore::KeyBindings::runtime()`.

Applications that need panic or signal restoration must install explicit hooks before entering the runtime, unless the active `tuicore` release provides those hooks directly.

### 2. Separate Domain State from View State

Do not mix mutable business data with interface navigation structures.

- **Shared State:** Domain data, workflow invariants, and UI state coordinated across components belong in one application `Store`.
- **Local View State:** Focus, open menus, input cursors, scroll positions, and state used by only one component remain owned by that component.

UI nodes (`TuiNode`) must be strictly presentation-oriented: they read store snapshots, maintain local view details, and emit messages. Components never mutate the store directly or use it as an event bus. The application maps component messages to store actions.

### 3. Event and Action Update Flow

Non-trivial applications must use a structured unidirectional data flow:

```text
Terminal Event -> Keymap Resolver -> Action -> Reducer/Update -> Effect -> Async Worker -> Action -> Render
```

1. **Terminal Event:** Raw keys or mouse clicks are received via `tuicore::TuiEvent`.
2. **Keymap Resolver:** Maps the event to an application-specific `Action` using configurable bindings. Component and runtime bindings come from `tuicore::KeyBindings`; product-specific actions may use an app-owned configurable map.
3. **Reducer/Update:** The root reducer delegates to the owning domain slice, which mutates its portion of `AppState` and produces `Effect`s.
4. **Effect:** Desired side-effects (file write, network query, clipboard copy).
5. **Async Worker:** Executes the effect on a background channel, returning success or failure as a new `Action`.
6. **Render:** The UI is redrawn based on the updated state.

### 4. Keyboard Focus and Modal Overlays

Behavior is governed by explicit focus chains and modal state.

- **Focus Control:** Focus traversal is defined via `tuicore::FocusChain` or `tuicore::FocusRouter`, and runtime focus is coordinated by `tuicore` focus management. Focus shifts are triggered by configurable focus bindings.
- **Modals:** Modal overlays are rendered via `tuicore::Overlay`. Modals capture keyboard inputs first.

### 5. Compositional Layouts

UI structures are composed recursively using `TuiNode` implementations.

- Use layout components provided by `tuicore` (`Split`, `Flex`, `Grid`, `Stack`, `Panel`, `Tabs`) instead of manual geometry computations.
- Render logic inside `TuiNode::render` must be deterministic and cheap. Avoid performing heavy operations, memory allocations, locking mutexes, or executing side-effects during render loops.
- Large lists must only render viewport-visible rows (virtual scroll support via `tuicore::components::List` or `DataView`).

### 6. Asynchronous Backend Services

Long-running operations must never block input parsing or rendering.

- Workers run on background threads/futures, transmitting results back to the main thread via event channels or thread-safe state snapshots.
- Every asynchronous request must carry a request identifier or generation token to prevent stale responses from overwriting newer user interactions.

### 7. Graceful Degradation and User Warnings

Failed network queries, missing local configurations, or input validation errors must be handled gracefully:
- Remote errors display error messages in the affected panel or status bar if available.
- Invalid configurations cause clean startup exits before terminal initialization.
- Thread channel disconnection initiates a safe application shutdown instead of panicking.

### 8. Responsibility-Based Decomposition

Single responsibility is mandatory. Each module or component owns one cohesive capability and one reason to change. Page composition, page-local controls, shared controls, domain state transitions, and side effects must not share a module. When describing a module requires "and," split it.

- Every tab body is implemented by a component in its own `pages/<page>/` directory.
- Components used only by one page live in that page's `components/` directory.
- Components reused by multiple pages live in the root `components/` directory.
- Tests always live in a sibling `tests/` directory next to the code they verify. Inline test modules are prohibited.
- Decomposition is driven by responsibility and ownership, never line count.

## Recommended Package Shape

A standard application should follow this feature-oriented single-package structure before splitting into workspace crates:

```text
src/
  main.rs
  lib.rs
  app.rs

  store/
    mod.rs                   # Compose and delegate; no domain behavior
    work_items/
      mod.rs                 # State, actions, update rules, and selectors
      tests/
    composer/
      mod.rs
      tests/
    shell/
      mod.rs                 # Selected tab and global overlays
      tests/

  pages/
    backlog/
      page.rs
      components/
      tests/
    sprint/
      page.rs
      components/
      tests/
    issues/
      page.rs
      components/
      tests/
    composer/
      page.rs
      components/
      tests/

  components/
    status_bar/
      mod.rs
      tests/
```

Store slices follow domain responsibility, never technical role or screen layout. Each slice co-locates its state, actions, update rules, and selectors. The root store only composes slices and delegates actions. A page may consume several slices, and several pages may consume one slice. For example, Backlog, Sprint, and Issues may be different projections over one `work_items` slice rather than three duplicated page-shaped stores.

### Extraction Triggers

Only extract into workspace crates when:
- Part of the domain engine is reused in another client (e.g., CLI, Web);
- Cargo dependency graphs or build times become a bottleneck;
- Strict security/platform borders are required.

## Runtime Lifecycle

1. **Parse & Configure:** Parse command line inputs and merge application configuration. UI configuration is loaded from `tui.toml` and `keybindings.toml` through `tuicore::init`, `tuicore::try_init`, `tuicore::init_from_dir`, or `tuicore::try_init_from_dir`.
2. **Initialize:** Setup logging, optional panic/signal hooks, and instantiate services/API clients.
3. **Setup State:** Build the initial `AppState` containing default values.
4. **Enter Runtime:** Call `tuicore::run` passing the root `TuiNode`, or use `tuicore::TreeApp::new(root)` when message handling or runtime options are required.
5. **Run Loop:** `tuicore` coordinates the event loop, forwarding inputs, resizing events, and animation ticks to the root node.
6. **Shutdown:** On exit, pending tasks are cancelled, volatile states are saved, and the `TerminalGuard` owned by `tuicore` restores the shell.

## Event, Action, and Effect Interface

Define clear boundaries for updates:

```rust
pub enum AppAction {
    WorkItems(WorkItemsAction),
    Composer(ComposerAction),
    Shell(ShellAction),
}

pub fn update(state: &mut AppState, action: AppAction) -> DispatchOutcome {
    match action {
        AppAction::WorkItems(action) => work_items::update(&mut state.work_items, action),
        AppAction::Composer(action) => composer::update(&mut state.composer, action),
        AppAction::Shell(action) => shell::update(&mut state.shell, action),
    }
}
```

- Raw keys map to `Action` variants using configurable bindings. Runtime/component keys use `tuicore::KeyBindings`; app-specific action bindings may be stored in application config but should parse and label keys consistently with `tuicore::KeySpec`.
- Each domain slice owns its state type, action type, update function, and selectors. Do not create global `state.rs`, `action.rs`, or `update.rs` files organized only by technical role.
- Domain update functions are the sole place where shared state changes.
- `Effect` variants execute inside background handlers and yield new `Action` responses.

## Configuration Contract

Config is separated into three concern areas:
1. **Application Config (`config.toml`):** Contains site settings, paths, server endpoints, and application parameters.
2. **Keymap Config (`keybindings.toml`):** Configures `tuicore` runtime and component keyboard bindings.
3. **Theme Config (`tui.toml`):** Defines theme choice and overrides semantic color styles.

### Merge Order:
1. Built-in system defaults.
2. Configuration files in application home (e.g., `~/.config/app/` or `~/.app/`).
3. Environment variables.
4. Command line arguments.

### Keymap Shape:
`tuicore::KeyBindings` currently supports fixed sections such as `[runtime]`, `[nav]`, `[focus]`, `[button]`, `[tabs]`, `[toggle]`, `[data_view]`, and `[dropdown]`. Application-specific action keymaps belong in the application configuration layer unless promoted into `tuicore`.

Startup validation must reject malformed keys and should reject duplicated or ambiguous bindings within the same active context. When the application introduces its own action bindings, missing actions must fail startup before terminal initialization.

## Decision Ledger

| Decision | Status | Evidence | Tradeoff | Verdict | Action |
|---|---|---|---|---|---|
| Use `tuicore` for terminal lifecycle, runtime dispatch, focus, layout, themes, and component keybindings | Settled | Core invariants and runtime lifecycle sections | Reduces app boilerplate, but apps must track actual `tuicore` contracts instead of assuming features | Leave alone | Keep this as the baseline |
| Keep application domain/update/effect logic outside UI nodes | Settled | State separation and event/action/effect flow sections | Adds structure for small apps, but preserves testability and side-effect clarity | Leave alone | Apply once app exceeds trivial demo scope |
| Keep apps as single packages before workspace extraction | Settled | Recommended package shape and extraction triggers | Avoids premature crate boundaries, but allows later split when reuse/security/build pressure appears | Leave alone | Revisit only on extraction triggers |
| App-specific keymaps are app-owned unless `tuicore` grows generic action binding support | Implicit | `tuicore::KeyBindings` is component/runtime oriented | Avoids forcing product action concepts into `tuicore`, but apps need a thin configurable action-keymap layer | Decision captured | Document app keymap format when first needed |
| Panic and signal restoration are not assumed to be handled by `tuicore` | Settled | Runtime lifecycle explicitly scopes current guarantee to guard/drop restoration | Avoids false safety claims, but apps needing stronger recovery must add hooks | Leave alone | Revisit if `tuicore` adds native panic/signal hooks |

## Fitness Functions / Guardrails

These guardrails keep implementations aligned with this architecture without adding heavy process:

1. **No raw terminal lifecycle in apps:** A search or lint check should fail app code that calls raw crossterm terminal setup/teardown directly outside approved integration hooks.
2. **No hard-coded-only shortcuts:** Key handling tests should prove runtime/component shortcuts resolve through `tuicore::KeyBindings`, and app-specific shortcuts resolve through a configurable app keymap.
3. **Dynamic shortcut help:** Help/status widgets should render labels from resolved key specs (`KeySpec::label()` or matching app wrapper), not copied literal labels.
4. **Pure render path:** Render tests or review checks should reject network calls, file I/O, mutex-heavy work, or state mutation inside `TuiNode::render`.
5. **Stale async response protection:** Backend-flow tests should prove generation/request identifiers prevent older worker responses from overwriting newer user-visible state.
6. **Page ownership:** Every tab body must come from its own component under `pages/<page>/`; root composition must not contain page implementation details.
7. **Component locality:** Page-only components stay under that page. Only components used by multiple pages belong in root `components/`.
8. **Domain store slices:** Store modules are split by domain capability and co-locate state, actions, updates, and selectors. Folder-by-technical-role store layouts are prohibited.
9. **External test modules:** Tests live in sibling `tests/` directories. Inline `#[cfg(test)] mod tests { ... }` blocks are prohibited.

## Leave-Alone Guidance

| Area | Why good enough | Revisit trigger |
|---|---|---|
| Single-package application structure | Lowest ceremony and easiest navigation for early TUI apps | Domain reused by another client, build graph hurts, or security/platform boundary becomes real |
| `tuicore` component/layout primitives | Existing primitives cover common TUI composition without manual geometry | Repeated custom geometry, missing layout capability, or performance bottleneck in real screens |
| App-owned action/effect layer | Product workflows differ by app and should not be forced into `tuicore` prematurely | Multiple apps duplicate the same action-keymap/effect runner abstractions |
| Guardrail tests over ADR-heavy process | Architecture is small and implementation-facing | Hard-to-reverse vendor/data/public-contract decisions appear |

## Testing Strategy

Write high-value, behavior-driven integration tests rather than checking isolated implementations. Every tested module or component keeps its tests in a sibling `tests/` directory; split test files by behavior when multiple concerns emerge. Production files must not contain inline test bodies.

1. **Config Merges:** Verify that user configurations combine correctly with default templates.
2. **Key-to-Action mapping:** Simulate terminal events and assert that they resolve to the expected `Action`.
3. **State Transitions:** Feed actions to the reducer and check the modified `AppState` invariants.
4. **Mock Backend Flows:** Exercise service adapters with fake clients to ensure that error/stale results lead to visible UI warnings.
5. **Deterministic Render Tests:** Assert that important headers, status labels, and help widgets render correctly into a test backend. Avoid assertions on individual terminal style cells or positions.
