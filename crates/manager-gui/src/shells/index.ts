// crates/manager-gui/src/shells/index.ts
// The set of layouts a person can pick between, and what each one renders.
// Adding a new layout means adding one file next to these two and one
// entry to this map — nothing else in the app needs to know it exists.

import type { Component } from "solid-js";
import type { LayoutId } from "../layouts";
import { DualPaneShell } from "./DualPaneShell";
import { SinglePaneShell } from "./SinglePaneShell";

export const LAYOUTS: Record<LayoutId, { label: string; Shell: Component }> = {
  "dual-pane": { label: "Dual pane", Shell: DualPaneShell },
  "single-pane": { label: "Single pane", Shell: SinglePaneShell },
};
