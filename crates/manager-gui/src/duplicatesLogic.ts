// Pure decisions the duplicate finder makes — flattening groups into rows a
// list can scroll through, what "keep the first of each group" means, and
// what the current selection is. Pulled out the same way panelLogic.ts's
// and compareLogic.ts's are. Marking is keyed by path rather than name:
// unlike a folder listing, entries here come from anywhere in the tree, so
// two different files can share a name.

import type { DuplicateGroupDto, EntryDto } from "./types";

/** One row of the flattened list: a file, or the boundary before a new
 * group — mirrors `manager-tui`'s own `duplicates::Row`. */
export type Row = { kind: "groupStart"; size: number } | { kind: "file"; entry: EntryDto };

export function flattenGroups(groups: DuplicateGroupDto[]): Row[] {
  return groups.flatMap((group) => [
    { kind: "groupStart" as const, size: group.size },
    ...group.files.map((entry) => ({ kind: "file" as const, entry })),
  ]);
}

/** Every group's first copy kept, every other copy marked — "first" is
 * whatever order they were found in, not a judgement about the original. */
export function markAllButFirstOfEachGroup(groups: DuplicateGroupDto[]): Set<string> {
  const marked = new Set<string>();
  for (const group of groups) {
    for (const entry of group.files.slice(1)) marked.add(entry.path);
  }
  return marked;
}

/**
 * What an action like delete should act on: every marked file, or — when
 * nothing is marked — the one under the cursor (empty if it's on a group
 * heading, which nothing can mark). Mirrors `manager-tui`'s own
 * `Duplicates::selection`.
 */
export function selection(rows: Row[], marked: ReadonlySet<string>, cursor: number): EntryDto[] {
  const markedEntries = rows
    .filter((row): row is Extract<Row, { kind: "file" }> => row.kind === "file" && marked.has(row.entry.path))
    .map((row) => row.entry);
  if (markedEntries.length > 0) return markedEntries;
  const current = rows[cursor];
  return current?.kind === "file" ? [current.entry] : [];
}
