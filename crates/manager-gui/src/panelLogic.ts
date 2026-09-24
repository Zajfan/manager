// The pure decisions one panel makes — where the cursor lands, what Enter
// and Backspace do — pulled out of `Panel.tsx` so they can be tested with
// plain data, the same way `manager-core`'s own logic is kept separate from
// its I/O. A browser's DOM and Tauri's IPC are both awkward to drive from a
// test; a function that takes numbers and returns a number isn't.

import type { EntryDto } from "./types";

/** Keeps a cursor inside `[0, length)`, or at 0 for an empty list. */
export function clampCursor(cursor: number, length: number): number {
  if (length <= 0) return 0;
  return Math.min(Math.max(cursor, 0), length - 1);
}

/** Where the cursor lands after moving by `delta`, clamped to the list. */
export function moveCursor(cursor: number, delta: number, length: number): number {
  return clampCursor(cursor + delta, length);
}

/** Toggles `name` in `marked`, returning a new set — `marked` itself is untouched. */
export function toggleMark(marked: ReadonlySet<string>, name: string): Set<string> {
  const next = new Set(marked);
  if (!next.delete(name)) next.add(name);
  return next;
}

/**
 * What an action like delete or copy should act on: every marked entry, in
 * display order, or — when nothing is marked — just the one under the
 * cursor. Mirrors `manager-tui`'s own `App::selection`.
 */
export function selection(
  entries: EntryDto[],
  marked: ReadonlySet<string>,
  cursor: number,
): EntryDto[] {
  const markedEntries = entries.filter((e) => marked.has(e.name));
  if (markedEntries.length > 0) return markedEntries;
  const current = entries[cursor];
  return current ? [current] : [];
}

/**
 * The entry Enter should open, or `null` if there isn't one (an empty
 * folder, or the entry under the cursor isn't something Enter can open).
 */
export function entryToOpen(entries: EntryDto[], cursor: number): EntryDto | null {
  const entry = entries[cursor];
  return entry?.isDirLike ? entry : null;
}
