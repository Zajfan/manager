import { describe, expect, it } from "vitest";
import { isSyncable, markAllSyncable, selection } from "./compareLogic";
import type { DiffEntryDto } from "./types";

function diff(key: string, status: DiffEntryDto["status"]): DiffEntryDto {
  return { key, name: key, left: null, right: null, status, newer: null };
}

describe("isSyncable", () => {
  it("is true for leftOnly, rightOnly and differs", () => {
    expect(isSyncable("leftOnly")).toBe(true);
    expect(isSyncable("rightOnly")).toBe(true);
    expect(isSyncable("differs")).toBe(true);
  });

  it("is false for same and kindMismatch", () => {
    expect(isSyncable("same")).toBe(false);
    expect(isSyncable("kindMismatch")).toBe(false);
  });
});

describe("markAllSyncable", () => {
  it("marks every syncable entry and skips the rest", () => {
    const entries = [diff("a", "leftOnly"), diff("b", "same"), diff("c", "differs")];
    const marked = markAllSyncable(entries);
    expect(marked.has("a")).toBe(true);
    expect(marked.has("b")).toBe(false);
    expect(marked.has("c")).toBe(true);
  });
});

describe("selection", () => {
  it("returns marked entries when something is marked", () => {
    const entries = [diff("a", "leftOnly"), diff("b", "differs")];
    expect(selection(entries, new Set(["b"]), 0).map((e) => e.key)).toEqual(["b"]);
  });

  it("falls back to the entry under the cursor when nothing is marked", () => {
    const entries = [diff("a", "leftOnly"), diff("b", "differs")];
    expect(selection(entries, new Set(), 1).map((e) => e.key)).toEqual(["b"]);
  });

  it("is empty for an empty list", () => {
    expect(selection([], new Set(), 0)).toEqual([]);
  });
});
