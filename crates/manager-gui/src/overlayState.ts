// crates/manager-gui/src/overlayState.ts
// The Search/Compare/Duplicates overlay wiring every shell needs, pulled
// out of what used to be App.tsx's own hand-rolled, hardcoded-to-2-panels
// signals. Generalized over PanelId so a shell with any number of panels
// (a single-pane shell's one panel, a dual-pane shell's two, a future
// column view's many) can use the same logic — each shell just picks its
// own id space (numbers, strings, whatever it finds natural) and passes
// those ids in.
//
// compareOpen has no panel id: comparing inherently needs two paths, which
// only a dual-pane-shaped layout has lying around today, so shells with no
// natural "left/right" just don't wire it up yet (see the design spec).
// duplicatesRoot likewise carries no panel id — the duplicate finder never
// calls back to whichever panel opened it, unlike search's goto.

import { createSignal } from "solid-js";

interface OpenSearch<PanelId> {
  panelId: PanelId;
  root: string;
}

export function createOverlayState<PanelId extends string | number>() {
  const [search, setSearch] = createSignal<OpenSearch<PanelId> | null>(null);
  const [compareOpen, setCompareOpen] = createSignal(false);
  const [duplicatesRoot, setDuplicatesRoot] = createSignal<string | null>(null);
  const [gotoTargets, setGotoTargets] = createSignal<Map<PanelId, string>>(new Map());

  function openSearch(panelId: PanelId, root: string) {
    setSearch({ panelId, root });
  }

  function closeSearch() {
    setSearch(null);
  }

  function handleGoto(path: string) {
    const opened = search();
    if (!opened) return;
    setGotoTargets((targets) => new Map(targets).set(opened.panelId, path));
    setSearch(null);
  }

  function gotoTargetFor(panelId: PanelId): () => string | null {
    return () => gotoTargets().get(panelId) ?? null;
  }

  function clearGotoTarget(panelId: PanelId) {
    setGotoTargets((targets) => {
      if (!targets.has(panelId)) return targets;
      const next = new Map(targets);
      next.delete(panelId);
      return next;
    });
  }

  function openDuplicates(root: string) {
    setDuplicatesRoot(root);
  }

  function closeDuplicates() {
    setDuplicatesRoot(null);
  }

  return {
    search,
    openSearch,
    closeSearch,
    handleGoto,
    gotoTargetFor,
    clearGotoTarget,
    compareOpen,
    setCompareOpen,
    duplicatesRoot,
    openDuplicates,
    closeDuplicates,
  };
}
