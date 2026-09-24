// crates/manager-gui/src/shells/SinglePaneShell.tsx
// The thinnest possible alternative to the dual-pane shell: one Panel
// filling the window, opened on the home folder. No sidebar, no tree, no
// second pane — deliberately, the same way the dual-pane shell itself
// started with nothing but two panels and grew from there one slice at a
// time. F5/F6's destination field defaults to empty (Panel already handles
// having no otherPath); Ctrl+F9 compare isn't wired here at all — it
// inherently needs two paths, and this shell only has one.

import { Show, createSignal, onMount } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { Duplicates } from "../Duplicates";
import { Panel } from "../Panel";
import { Search } from "../Search";
import { createOverlayState } from "../overlayState";

const PANEL_ID = "main";

export function SinglePaneShell() {
  const [home, setHome] = createSignal<string | null>(null);
  const overlay = createOverlayState<typeof PANEL_ID>();

  onMount(async () => {
    setHome(await invoke<string>("home_dir"));
  });

  return (
    <div class="app">
      <Show when={home()} fallback={<div class="panel-status">Starting...</div>}>
        {(startPath) => (
          <div class="panels">
            <Panel
              initialPath={startPath()}
              active={() => true}
              onActivate={() => {}}
              onOpenSearch={(root) => overlay.openSearch(PANEL_ID, root)}
              onOpenDuplicates={overlay.openDuplicates}
              gotoTarget={overlay.gotoTargetFor(PANEL_ID)}
              onGotoHandled={() => overlay.clearGotoTarget(PANEL_ID)}
            />
          </div>
        )}
      </Show>
      <Show when={overlay.search()}>
        {(s) => (
          <Search rootPath={s().root} onClose={overlay.closeSearch} onGoto={overlay.handleGoto} />
        )}
      </Show>
      <Show when={overlay.duplicatesRoot()}>
        {(root) => <Duplicates rootPath={root()} onClose={overlay.closeDuplicates} />}
      </Show>
    </div>
  );
}
