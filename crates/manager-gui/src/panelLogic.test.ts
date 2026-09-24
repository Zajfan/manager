import { describe, expect, it } from "vitest";
import { clampCursor, entryToOpen, moveCursor, selection, toggleMark } from "./panelLogic";
import type { EntryDto } from "./types";

function file(name: string, isDirLike = false): EntryDto {
  return {
    name,
    path: `file:///home/${name}`,
    kind: isDirLike ? "dir" : "file",
    isDirLike,
    size: 10,
    modifiedMs: null,
    hidden: false,
  };
}

describe("clampCursor", () => {
  it("leaves an in-range cursor alone", () => {
    expect(clampCursor(2, 5)).toBe(2);
  });

  it("pulls a cursor below zero up to zero", () => {
    expect(clampCursor(-3, 5)).toBe(0);
  });

  it("pulls a cursor past the end back to the last row", () => {
    expect(clampCursor(99, 5)).toBe(4);
  });

  it("is zero for an empty list, whatever was asked for", () => {
    expect(clampCursor(3, 0)).toBe(0);
    expect(clampCursor(-3, 0)).toBe(0);
  });
});

describe("moveCursor", () => {
  it("moves down by the given amount", () => {
    expect(moveCursor(1, 2, 10)).toBe(3);
  });

  it("moves up by a negative amount", () => {
    expect(moveCursor(5, -2, 10)).toBe(3);
  });

  it("stops at the last row rather than wrapping", () => {
    expect(moveCursor(4, 10, 5)).toBe(4);
  });

  it("stops at the first row rather than going negative", () => {
    expect(moveCursor(1, -10, 5)).toBe(0);
  });
});

describe("toggleMark", () => {
  it("adds a name that wasn't marked", () => {
    const result = toggleMark(new Set(), "a.txt");
    expect(result.has("a.txt")).toBe(true);
  });

  it("removes a name that was already marked", () => {
    const result = toggleMark(new Set(["a.txt"]), "a.txt");
    expect(result.has("a.txt")).toBe(false);
  });

  it("leaves other marked names untouched", () => {
    const result = toggleMark(new Set(["a.txt", "b.txt"]), "a.txt");
    expect(result.has("b.txt")).toBe(true);
  });

  it("never mutates the set it was given", () => {
    const original = new Set(["a.txt"]);
    toggleMark(original, "b.txt");
    expect(original.has("b.txt")).toBe(false);
  });
});

describe("selection", () => {
  it("returns marked entries in display order when something is marked", () => {
    const entries = [file("a.txt"), file("b.txt"), file("c.txt")];
    const marked = new Set(["c.txt", "a.txt"]);
    expect(selection(entries, marked, 1).map((e) => e.name)).toEqual(["a.txt", "c.txt"]);
  });

  it("falls back to the entry under the cursor when nothing is marked", () => {
    const entries = [file("a.txt"), file("b.txt")];
    expect(selection(entries, new Set(), 1).map((e) => e.name)).toEqual(["b.txt"]);
  });

  it("is empty when nothing is marked and the list is empty", () => {
    expect(selection([], new Set(), 0)).toEqual([]);
  });
});

describe("entryToOpen", () => {
  it("returns the entry under the cursor when it is dir-like", () => {
    const entries = [file("a.txt"), file("docs", true)];
    expect(entryToOpen(entries, 1)).toEqual(file("docs", true));
  });

  it("returns null for a plain file — nothing to open yet", () => {
    const entries = [file("a.txt")];
    expect(entryToOpen(entries, 0)).toBeNull();
  });

  it("returns null when the cursor is out of range", () => {
    expect(entryToOpen([file("a.txt")], 5)).toBeNull();
  });

  it("returns null for an empty list", () => {
    expect(entryToOpen([], 0)).toBeNull();
  });
});
