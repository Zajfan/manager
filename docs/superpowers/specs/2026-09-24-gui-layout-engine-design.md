# GUI layout engine: pluggable shells

## Why

`crates/manager-gui` currently has exactly one layout: a Total Commander–style
dual-pane shell, hardcoded as the entire content of `App.tsx`. The user wants
that to become one *option* among several a person can pick at runtime — not
because TC-style is wrong, but because it was only ever chosen as "the one
file manager I know of," not as a deliberate design goal. This spec covers
turning the current single-layout `App.tsx` into a small engine that can host
several interchangeable layouts, each reusing the same `manager-core`-backed
commands and the same `Panel.tsx` folder-view building block.

Target layouts (this spec's scope is the *engine* plus the second layout as
proof; the remaining three are separate follow-up work each get their own
slice, same incremental discipline the seven GUI-parity features already
used):

1. **Dual-pane (TC-style)** — exists today, becomes shell #1 under the engine.
2. **Single-pane Explorer/Finder-style** — this spec's new shell, thin first
   slice (browse only, no sidebar).
3. Column view (macOS Finder–style) — future slice, not designed here.
4. Tree + list — future slice, not designed here.
5. Tabs — future slice, not designed here.

## Non-goals

- A runtime plugin SDK for third-party layouts. All layouts are first-party,
  built into this crate. (Approach A from the brainstorm — a full registry
  with pluggable external shells — was considered and rejected as more
  machinery than five first-party layouts need.)
- Full feature parity for every new layout on day one. Each new shell starts
  as a thin browse-only slice and grows the same way the dual-pane shell did
  across this project's history (marking, then jobs, then search, etc.).
- Solving F5/F6 "copy to the other pane" or Ctrl+F9 "compare the two panes"
  for layouts that have no natural second pane. Those are per-shell design
  questions each new shell answers when it gets there — out of scope here.
- A full settings/preferences screen. Only the layout switcher itself is
  built now.

## Architecture

```
crates/manager-gui/src/
  App.tsx                 — thin dispatcher + layout switcher button
  settings.ts              — getLayout()/setLayout() wrapping tauri-plugin-store
  overlayState.ts          — createOverlayState(), shared by every shell
  shells/
    DualPaneShell.tsx      — today's App.tsx body, moved here unchanged
    SinglePaneShell.tsx    — new: one Panel, nothing else
    index.ts               — LayoutId type + id → component map
  Panel.tsx                 — unchanged: the folder-view building block
  Search.tsx / Compare.tsx / Duplicates.tsx — unchanged: already layout-agnostic
```

### `App.tsx` (dispatcher)

On mount, calls `settings.getLayout()` (defaults to `"dual-pane"` if the
store has nothing yet, or if the read fails for any reason — a broken or
missing settings file must never block the app from starting). Renders
`SHELLS[layout()]`, a lookup into the `shells/index.ts` map. Also renders one
small always-visible button (independent of whichever shell is active) that
opens a minimal dropdown listing available layouts; picking one calls
`settings.setLayout(id)` (persists) and updates the local `layout` signal
(switches immediately — no restart, matching the "runtime switchable"
decision). This button lives in `App.tsx` itself, not inside any shell, so
it stays available and consistent regardless of which shell renders.

### `settings.ts`

Thin wrapper, the same spirit as `jobs.ts`/`searchApi.ts`/etc. — no logic of
its own:

```ts
export type LayoutId = "dual-pane" | "single-pane";

export async function getLayout(): Promise<LayoutId> { ... }
export async function setLayout(id: LayoutId): Promise<void> { ... }
```

Backed by `tauri-plugin-store`'s JS `Store` class (`@tauri-apps/plugin-store`),
pointed at a `settings.json` file the plugin manages entirely — no custom
Rust command needed for this, which is the plugin's whole point. Requires
adding the plugin crate (`tauri-plugin-store`) to `src-tauri/Cargo.toml` and
registering it in `lib.rs`'s `Builder` (one `.plugin(...)` call, same pattern
`tauri-plugin-opener` already uses), plus the npm package
`@tauri-apps/plugin-store`.

### `overlayState.ts`

Extracts the overlay-wiring logic that's currently tangled into `App.tsx`
alongside its 2-panel assumptions. Generalized to any number of panels, each
identified by an id the calling shell assigns (dual-pane uses `0 | 1`,
single-pane uses a single constant like `"main"`):

```ts
export function createOverlayState<PanelId extends string | number>() {
  // search: { panelId: PanelId; root: string } | null
  // compareOpen: boolean (dual-pane-specific concept — see below)
  // duplicates: { panelId: PanelId; root: string } | null  (root path only needed for display)
  // gotoTargets: Map<PanelId, string | null> + per-id getters/setters
  // handleGoto(path): routes back to whichever panelId opened the search that's open
  return { ... };
}
```

A shell calls this once, then wires each `Panel` instance's
`onOpenSearch`/`onOpenDuplicates`/`gotoTarget`/`onGotoHandled` props to the
returned handlers, keyed by whatever id it assigned that panel. `compareOpen`
stays a dual-pane-only concept for now (it inherently needs two paths); a
single-pane shell simply doesn't wire Ctrl+F9 yet, the same way its first
slice doesn't wire F5/F6 to anything but an empty-destination fallback
`Panel.tsx` already handles.

### `shells/DualPaneShell.tsx`

Pure move of today's `App.tsx` body — same two `Panel`s, same Tab-switching,
same `pathOf0`/`pathOf1` cross-panel destination wiring, same Ctrl+F9 handler
— now built on top of `createOverlayState<0 | 1>()` instead of its own
hand-rolled signals. No behavior change; existing manual verification (this
is UI wiring with no dedicated `App.test.tsx` today) is the same live-window
check already used for this shell throughout the project, plus the full
existing `Panel.test.tsx`/`Search.test.tsx`/`Compare.test.tsx`/
`Duplicates.test.tsx` suites continuing to pass unchanged (they test the
building blocks, not `App.tsx`, so they're a strong regression check that
the pieces DualPaneShell composes still behave).

### `shells/SinglePaneShell.tsx`

The new proof-of-concept layout, deliberately thin: one `Panel`, filling the
window, opened on the home folder, built on `createOverlayState<"main">()`.
Alt+F7 (search), Ctrl+D (duplicates) and Ctrl+N (SFTP connect) work
immediately, since they're already per-panel concepts `Panel.tsx` exposes
generically — connect in particular needs nothing beyond `setPath()`, which
doesn't care how many panels exist. F5/F6 destination defaults to empty
(type a path) since there's no second pane. Ctrl+F9 (compare) isn't wired at
all in this first slice, per `overlayState.ts` above — it inherently needs
two paths, and picking where the second one comes from without a second
pane is exactly the kind of per-shell design question this spec defers.

## Data flow

1. App starts → `App.tsx` mounts → `settings.getLayout()` → renders the
   matching shell.
2. User opens the layout switcher, picks a different one → `setLayout(id)`
   persists it → the `layout` signal updates → `App.tsx` swaps which shell
   is rendered. The outgoing shell unmounts (its `Panel`s' `onCleanup`
   already tears down their job/search/compare/duplicates listeners
   correctly — no new cleanup concern introduced here) and the incoming one
   mounts fresh, starting on the home folder the same way the app does on
   first launch.
3. Within a shell, all existing per-feature data flow (jobs, search,
   compare, duplicates, SFTP) is completely unchanged — every one of those
   already flows through Tauri events filtered by an id the frontend tracks,
   nothing about that depends on the shell arrangement.

## Error handling

- Settings store unreadable/missing/corrupt → `getLayout()` catches and
  returns `"dual-pane"`, the existing default behavior today's users already
  get. Never blocks startup.
- An unknown/stale `layoutId` in the store (e.g., a shell was removed in a
  later version) → `App.tsx`'s lookup falls back to `"dual-pane"` the same
  way, rather than rendering nothing.

## Testing

- `SinglePaneShell.tsx` gets its own `SinglePaneShell.test.tsx`, mirroring
  `Panel.test.tsx`'s mocking pattern: renders, confirms one `Panel` shows the
  home folder's contents, confirms there's no second pane in the DOM.
- `overlayState.ts` gets direct tests instantiating
  `createOverlayState<0 | 1>()` and `createOverlayState<"main">()`,
  confirming goto/search/duplicates routing reaches the right panel id in
  each case — this is the piece most likely to have an off-by-one/wrong-id
  bug, so it's the one most worth a mutation-checked test the way prior
  slices' riskiest logic got one.
- `settings.ts` stays a thin wrapper without dedicated tests, consistent with
  how `jobs.ts`/`searchApi.ts`/`compareApi.ts`/`duplicatesApi.ts` are treated
  — the plugin itself is Tauri's own tested code.
- After the `DualPaneShell.tsx` extraction, the full existing frontend suite
  (currently 86 tests across `Panel`/`Search`/`Compare`/`Duplicates`) must
  stay green unchanged — that's the regression signal for the refactor,
  since none of those tests import `App.tsx` directly.
- Rust side: no new Rust logic beyond registering the store plugin in
  `lib.rs`, so no new Rust tests are needed — `cargo clippy`/`cargo test
  --workspace` staying green is the bar, same as every other slice.
