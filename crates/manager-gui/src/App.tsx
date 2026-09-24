// The whole window: two panels side by side, Tab switching which one keys
// go to — the same shape `manager-tui`'s two-pane layout has, just drawn
// with the browser's own DOM instead of Ratatui's cells.

import { Show, createSignal, onMount } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { Compare } from "./Compare";
import { Duplicates } from "./Duplicates";
import { Panel } from "./Panel";
import { Search } from "./Search";
import "./app.css";

export default function App() {
  const [home, setHome] = createSignal<string | null>(null);
  const [active, setActive] = createSignal<0 | 1>(0);
  // Each panel reports its own current folder here so the other one can
  // default F5/F6's destination to it, the same way manager-tui's transfer
  // dialog starts pre-filled with the *other* panel's path.
  const [pathOf0, setPathOf0] = createSignal("");
  const [pathOf1, setPathOf1] = createSignal("");
  // Alt+F7: which panel opened it (its results go back to that panel) and
  // what folder it searches. `null` means the overlay is closed.
  const [search, setSearch] = createSignal<{ panel: 0 | 1; root: string } | null>(null);
  const [gotoTarget0, setGotoTarget0] = createSignal<string | null>(null);
  const [gotoTarget1, setGotoTarget1] = createSignal<string | null>(null);
  // Ctrl+F9: compares the left panel's folder against the right one's,
  // same as the terminal version — not tied to whichever panel is active.
  const [compareOpen, setCompareOpen] = createSignal(false);
  // Ctrl+D: which panel opened the duplicate finder, and where it searches.
  const [duplicates, setDuplicates] = createSignal<string | null>(null);

  onMount(async () => {
    setHome(await invoke<string>("home_dir"));
  });

  function handleKey(event: KeyboardEvent) {
    if (event.key === "Tab") {
      setActive((a) => (a === 0 ? 1 : 0));
      event.preventDefault();
    } else if (event.ctrlKey && event.key === "F9") {
      setCompareOpen(true);
      event.preventDefault();
    }
  }

  function handleGoto(path: string) {
    const s = search();
    if (!s) return;
    (s.panel === 0 ? setGotoTarget0 : setGotoTarget1)(path);
    setSearch(null);
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
              onOpenSearch={(root) => setSearch({ panel: 0, root })}
              onOpenDuplicates={setDuplicates}
              gotoTarget={gotoTarget0}
              onGotoHandled={() => setGotoTarget0(null)}
            />
            <Panel
              initialPath={startPath()}
              active={() => active() === 1}
              onActivate={() => setActive(1)}
              onPathChange={setPathOf1}
              otherPath={pathOf0}
              onOpenSearch={(root) => setSearch({ panel: 1, root })}
              onOpenDuplicates={setDuplicates}
              gotoTarget={gotoTarget1}
              onGotoHandled={() => setGotoTarget1(null)}
            />
          </div>
        )}
      </Show>
      <Show when={search()}>
        {(s) => (
          <Search rootPath={s().root} onClose={() => setSearch(null)} onGoto={handleGoto} />
        )}
      </Show>
      <Show when={compareOpen()}>
        <Compare left={pathOf0()} right={pathOf1()} onClose={() => setCompareOpen(false)} />
      </Show>
      <Show when={duplicates()}>
        {(root) => <Duplicates rootPath={root()} onClose={() => setDuplicates(null)} />}
      </Show>
    </div>
  );
}
