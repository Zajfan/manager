// The whole window: two panels side by side, Tab switching which one keys
// go to — the same shape `manager-tui`'s two-pane layout has, just drawn
// with the browser's own DOM instead of Ratatui's cells.

import { Show, createSignal, onMount } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { Panel } from "./Panel";
import "./app.css";

export default function App() {
  const [home, setHome] = createSignal<string | null>(null);
  const [active, setActive] = createSignal<0 | 1>(0);

  onMount(async () => {
    setHome(await invoke<string>("home_dir"));
  });

  function handleKey(event: KeyboardEvent) {
    if (event.key === "Tab") {
      setActive((a) => (a === 0 ? 1 : 0));
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
            />
            <Panel
              initialPath={startPath()}
              active={() => active() === 1}
              onActivate={() => setActive(1)}
            />
          </div>
        )}
      </Show>
    </div>
  );
}
