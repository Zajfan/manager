import { describe, expect, it } from "vitest";
import { flattenGroups, markAllButFirstOfEachGroup, selection } from "./duplicatesLogic";
import type { DuplicateGroupDto, EntryDto } from "./types";

function file(path: string, size = 10): EntryDto {
  return { name: path.split("/").pop()!, path, kind: "file", isDirLike: false, size, modifiedMs: null, hidden: false };
}

function group(size: number, ...paths: string[]): DuplicateGroupDto {
  return { size, files: paths.map((p) => file(p, size)) };
}

describe("flattenGroups", () => {
  it("puts a groupStart row before each group's files", () => {
    const rows = flattenGroups([group(10, "/a", "/b"), group(20, "/c", "/d")]);
    expect(rows).toEqual([
      { kind: "groupStart", size: 10 },
      { kind: "file", entry: file("/a", 10) },
      { kind: "file", entry: file("/b", 10) },
      { kind: "groupStart", size: 20 },
      { kind: "file", entry: file("/c", 20) },
      { kind: "file", entry: file("/d", 20) },
    ]);
  });

  it("is empty for no groups", () => {
    expect(flattenGroups([])).toEqual([]);
  });
});

describe("markAllButFirstOfEachGroup", () => {
  it("marks every file except the first of each group, by whatever order they arrived", () => {
    const groups = [group(10, "/a", "/b", "/c"), group(20, "/x", "/y")];
    const marked = markAllButFirstOfEachGroup(groups);
    expect(marked.has("/a")).toBe(false);
    expect(marked.has("/b")).toBe(true);
    expect(marked.has("/c")).toBe(true);
    expect(marked.has("/x")).toBe(false);
    expect(marked.has("/y")).toBe(true);
  });
});

describe("selection", () => {
  it("returns marked files when something is marked", () => {
    const rows = flattenGroups([group(10, "/a", "/b")]);
    expect(selection(rows, new Set(["/b"]), 0).map((e) => e.path)).toEqual(["/b"]);
  });

  it("falls back to the file under the cursor when nothing is marked", () => {
    const rows = flattenGroups([group(10, "/a", "/b")]);
    // cursor at index 2 is "/b" (index 0 is the groupStart row)
    expect(selection(rows, new Set(), 2).map((e) => e.path)).toEqual(["/b"]);
  });

  it("is empty when the cursor is on a group heading and nothing is marked", () => {
    const rows = flattenGroups([group(10, "/a", "/b")]);
    expect(selection(rows, new Set(), 0)).toEqual([]);
  });
});
