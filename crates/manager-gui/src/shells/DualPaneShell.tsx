// crates/manager-gui/src/shells/DualPaneShell.tsx
// Two panels side by side, Tab switching which one keys go to — the same
// shape `manager-tui`'s two-pane layout has, just drawn with the browser's
// own DOM instead of Ratatui's cells. The original layout, now one of
// several a person can pick between — see ../shells/index.ts.

import { Show, createSignal, onMount } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { Compare } from "../Compare";
import { Duplicates } from "../Duplicates";
import { Panel } from "../Panel";
import { Search } from "../Search";
import { createOverlayState } from "../overlayState";

export function DualPaneShell() {
  const [home, setHome] = createSignal<string | null>(null);
  const [active, setActive] = createSignal<0 | 1>(0);
  // Each panel reports its own current folder here so the other one can
  // default F5/F6's destination to it, the same way manager-tui's transfer
  // dialog starts pre-filled with the *other* panel's path.
  const [pathOf0, setPathOf0] = createSignal("");
  const [pathOf1, setPathOf1] = createSignal("");
  const overlay = createOverlayState<0 | 1>();

  onMount(async () => {
    setHome(await invoke<string>("home_dir"));
  });

  function handleKey(event: KeyboardEvent) {
    if (event.key === "Tab") {
      setActive((a) => (a === 0 ? 1 : 0));
      event.preventDefault();
    } else if (event.ctrlKey && event.key === "F9") {
      overlay.setCompareOpen(true);
      event.preventDefault();
    }
  }

  return (
    <div class="app" onKeyDown={handleKey}>
      <Show when={home()} fallback={<div class="panel-status">Starting...</div>}>
        {(startPath) => (
          <div class="panels">
            <Panel
              initialPath={startPath()}
              active={() => active() === 0}
              onActivate={() => setActive(0)}
              onPathChange={setPathOf0}
              otherPath={pathOf1}
              onOpenSearch={(root) => overlay.openSearch(0, root)}
              onOpenDuplicates={overlay.openDuplicates}
              gotoTarget={overlay.gotoTargetFor(0)}
              onGotoHandled={() => overlay.clearGotoTarget(0)}
            />
            <Panel
              initialPath={startPath()}
              active={() => active() === 1}
              onActivate={() => setActive(1)}
              onPathChange={setPathOf1}
              otherPath={pathOf0}
              onOpenSearch={(root) => overlay.openSearch(1, root)}
              onOpenDuplicates={overlay.openDuplicates}
              gotoTarget={overlay.gotoTargetFor(1)}
              onGotoHandled={() => overlay.clearGotoTarget(1)}
            />
          </div>
        )}
      </Show>
      <Show when={overlay.search()}>
        {(s) => (
          <Search rootPath={s().root} onClose={overlay.closeSearch} onGoto={overlay.handleGoto} />
        )}
      </Show>
      <Show when={overlay.compareOpen()}>
        <Compare
          left={pathOf0()}
          right={pathOf1()}
          onClose={() => overlay.setCompareOpen(false)}
        />
      </Show>
      <Show when={overlay.duplicatesRoot()}>
        {(root) => <Duplicates rootPath={root()} onClose={overlay.closeDuplicates} />}
      </Show>
    </div>
  );
}
