// crates/manager-gui/src/layouts.ts
// The set of layout ids that exist, independent of the components that
// render them — kept separate from shells/index.ts so settings.ts can
// validate a stored id without importing every shell component.

export type LayoutId = "dual-pane" | "single-pane";

export const LAYOUT_IDS: LayoutId[] = ["dual-pane", "single-pane"];

export const DEFAULT_LAYOUT: LayoutId = "dual-pane";
