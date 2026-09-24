// crates/manager-gui/src/App.tsx
// Picks which layout to show — persisted in settings, switchable anytime
// at runtime — and renders it. Owns nothing else: every actual feature
// lives inside whichever shell is currently showing. See shells/ for what
// layouts exist.

import { For, Show, createSignal, onMount } from "solid-js";
import { Dynamic } from "solid-js/web";
import { getLayout, setLayout } from "./settings";
import { LAYOUTS } from "./shells";
import type { LayoutId } from "./layouts";
import "./app.css";

export default function App() {
  const [layout, setLayoutSignal] = createSignal<LayoutId | null>(null);
  const [switcherOpen, setSwitcherOpen] = createSignal(false);

  onMount(async () => {
    setLayoutSignal(await getLayout());
  });

  async function chooseLayout(id: LayoutId) {
    setSwitcherOpen(false);
    setLayoutSignal(id);
    await setLayout(id);
  }

  return (
    <Show when={layout()} fallback={<div class="panel-status">Starting...</div>}>
      {(id) => (
        <div class="app-root">
          <button
            class="layout-switcher-button"
            onClick={() => setSwitcherOpen((open) => !open)}
            title="Change layout"
          >
            {LAYOUTS[id()].label}
          </button>
          <Show when={switcherOpen()}>
            <div class="layout-switcher-menu">
              <For each={Object.keys(LAYOUTS) as LayoutId[]}>
                {(candidate) => (
                  <button
                    classList={{ active: candidate === id() }}
                    onClick={() => void chooseLayout(candidate)}
                  >
                    {LAYOUTS[candidate].label}
                  </button>
                )}
              </For>
            </div>
          </Show>
          <Dynamic component={LAYOUTS[id()].Shell} />
        </div>
      )}
    </Show>
  );
}
