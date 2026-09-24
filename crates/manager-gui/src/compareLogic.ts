// Pure decisions the compare view makes — which differences a sync can act
// on, what "mark all" and "the current selection" mean — pulled out the same
// way panelLogic.ts's are. `toggleMark` from panelLogic.ts is reused as-is:
// it's already just a Set<string> toggle, nothing panel-specific about it.

import type { DiffEntryDto, DiffStatus } from "./types";

/** Whether a sync could do anything with a difference in this state —
 * mirrors `manager_core::compare::DiffStatus::is_syncable`. */
export function isSyncable(status: DiffStatus): boolean {
  return status === "leftOnly" || status === "rightOnly" || status === "differs";
}

/** Every syncable entry's key, marked — the rest left alone. */
export function markAllSyncable(entries: DiffEntryDto[]): Set<string> {
  return new Set(entries.filter((e) => isSyncable(e.status)).map((e) => e.key));
}

/**
 * What an action like sync should act on: every marked entry, or — when
 * nothing is marked — just the one under the cursor. Mirrors
 * `manager-tui`'s own `Compare::selection`.
 */
export function selection(
  entries: DiffEntryDto[],
  marked: ReadonlySet<string>,
  cursor: number,
): DiffEntryDto[] {
  const markedEntries = entries.filter((e) => marked.has(e.key));
  if (markedEntries.length > 0) return markedEntries;
  const current = entries[cursor];
  return current ? [current] : [];
}
