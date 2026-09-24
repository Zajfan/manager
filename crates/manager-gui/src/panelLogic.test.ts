import { describe, expect, it } from "vitest";
import { clampCursor, entryToOpen, moveCursor } from "./panelLogic";
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
