// Thin IPC wrappers around the search commands — no logic of its own, the
// same way jobs.ts isn't. Only one search runs at a time (same as the
// terminal version's single results view), so there's no id to track:
// starting a new one cancels whatever was running.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { EntryDto, SearchProgressEvent, SearchReportDto } from "./types";

export function startSearch(
  root: string,
  mask: string,
  text: string,
  caseSensitive: boolean,
  includeHidden: boolean,
): Promise<void> {
  return invoke("start_search", { root, mask, text, caseSensitive, includeHidden });
}

export function cancelSearch(): Promise<void> {
  return invoke("cancel_search");
}

export function onSearchFound(handler: (entry: EntryDto) => void): Promise<UnlistenFn> {
  return listen<EntryDto>("search-found", (e) => handler(e.payload));
}

export function onSearchProgress(
  handler: (event: SearchProgressEvent) => void,
): Promise<UnlistenFn> {
  return listen<SearchProgressEvent>("search-progress", (e) => handler(e.payload));
}

export function onSearchFinished(handler: (report: SearchReportDto) => void): Promise<UnlistenFn> {
  return listen<SearchReportDto>("search-finished", (e) => handler(e.payload));
}
