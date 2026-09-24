// crates/manager-gui/src/settings.ts
// Persists the one setting the app has so far: which layout to show.
// Backed by tauri-plugin-store's own JSON file (settings.json, in the
// app's config directory) — no custom Rust command needed, that's the
// plugin's whole job. A read failure (missing file, corrupt JSON, plugin
// error) never blocks the app from starting: it just falls back to the
// default layout, the same way an unrecognized stored id does.

import { LazyStore } from "@tauri-apps/plugin-store";
import { DEFAULT_LAYOUT, LAYOUT_IDS, type LayoutId } from "./layouts";

const store = new LazyStore("settings.json");

function isLayoutId(value: unknown): value is LayoutId {
  return typeof value === "string" && (LAYOUT_IDS as string[]).includes(value);
}

export async function getLayout(): Promise<LayoutId> {
  try {
    const value = await store.get<LayoutId>("layout");
    return isLayoutId(value) ? value : DEFAULT_LAYOUT;
  } catch {
    return DEFAULT_LAYOUT;
  }
}

export async function setLayout(id: LayoutId): Promise<void> {
  await store.set("layout", id);
  await store.save();
}
