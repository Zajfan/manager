// Thin IPC wrappers around the compare/sync commands — no logic of its own,
// the same way jobs.ts and searchApi.ts aren't. Only one comparison runs at
// a time, same as the terminal version's single Option<Compare>. Syncing
// starts jobs through the ordinary job engine: its progress, conflicts and
// errors arrive as the same job-progress/job-conflict/job-error events
// jobs.ts already listens for — nothing new to wire up there.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  CompareProgressEvent,
  CompareReportDto,
  DiffEntryDto,
  JobStartedDto,
  SyncDirection,
} from "./types";

export function startCompare(
  left: string,
  right: string,
  byContent: boolean,
  includeHidden: boolean,
): Promise<void> {
  return invoke("start_compare", { left, right, byContent, includeHidden });
}

export function cancelCompare(): Promise<void> {
  return invoke("cancel_compare");
}

export function syncCompare(keys: string[], direction: SyncDirection): Promise<JobStartedDto[]> {
  return invoke<JobStartedDto[]>("sync_compare", { keys, direction });
}

export function onCompareFound(handler: (entry: DiffEntryDto) => void): Promise<UnlistenFn> {
  return listen<DiffEntryDto>("compare-found", (e) => handler(e.payload));
}

export function onCompareProgress(
  handler: (event: CompareProgressEvent) => void,
): Promise<UnlistenFn> {
  return listen<CompareProgressEvent>("compare-progress", (e) => handler(e.payload));
}

export function onCompareFinished(
  handler: (report: CompareReportDto) => void,
): Promise<UnlistenFn> {
  return listen<CompareReportDto>("compare-finished", (e) => handler(e.payload));
}
