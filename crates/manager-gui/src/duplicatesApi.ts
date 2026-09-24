// Thin IPC wrappers around the duplicate-finder commands — no logic of its
// own, the same way searchApi.ts and compareApi.ts aren't. Deleting what's
// found is just startDelete from jobs.ts; nothing duplicate-specific about
// removing a path once you've decided to.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { DuplicateGroupDto, DuplicateReportDto, DuplicatesProgressEvent } from "./types";

export function startDuplicates(root: string, mask: string, includeHidden: boolean): Promise<void> {
  return invoke("start_duplicates", { root, mask, includeHidden });
}

export function cancelDuplicates(): Promise<void> {
  return invoke("cancel_duplicates");
}

export function onDuplicatesFound(
  handler: (group: DuplicateGroupDto) => void,
): Promise<UnlistenFn> {
  return listen<DuplicateGroupDto>("duplicates-found", (e) => handler(e.payload));
}

export function onDuplicatesProgress(
  handler: (event: DuplicatesProgressEvent) => void,
): Promise<UnlistenFn> {
  return listen<DuplicatesProgressEvent>("duplicates-progress", (e) => handler(e.payload));
}

export function onDuplicatesFinished(
  handler: (report: DuplicateReportDto) => void,
): Promise<UnlistenFn> {
  return listen<DuplicateReportDto>("duplicates-finished", (e) => handler(e.payload));
}
