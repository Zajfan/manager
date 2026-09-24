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

/**
 * The entry Enter should open, or `null` if there isn't one (an empty
 * folder, or the entry under the cursor isn't something Enter can open).
 */
export function entryToOpen(entries: EntryDto[], cursor: number): EntryDto | null {
  const entry = entries[cursor];
  return entry?.isDirLike ? entry : null;
}
