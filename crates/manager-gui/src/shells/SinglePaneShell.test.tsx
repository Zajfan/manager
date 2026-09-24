// crates/manager-gui/src/shells/SinglePaneShell.test.tsx
// Confirms the thinnest possible layout actually shows one folder's
// contents and nothing more — no second pane. Mocking follows the same
// pattern Panel.test.tsx uses, since Panel itself is what SinglePaneShell
// renders underneath.

import { cleanup, render, screen } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";
import { SinglePaneShell } from "./SinglePaneShell";
import type { ListingDto } from "../types";

afterEach(cleanup);

function listing(path: string, names: [string, boolean][]): ListingDto {
  return {
    path,
    parent: null,
    entries: names.map(([name, isDir]) => ({
      name,
      path: `${path}${name}`,
      kind: isDir ? "dir" : "file",
      isDirLike: isDir,
      size: 100,
      modifiedMs: null,
      hidden: false,
    })),
  };
}

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => (globalThis as any).__invoke(...args),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: () => Promise.resolve(() => {}),
}));

describe("SinglePaneShell", () => {
  it("shows one panel opened on the home folder, and no second pane", async () => {
    const invoke = vi.fn(async (command: string) => {
      if (command === "home_dir") return "file:///home/";
      if (command === "list_dir") return listing("file:///home/", [["docs", true]]);
      throw new Error(`unexpected command: ${command}`);
    });
    (globalThis as any).__invoke = invoke;

    render(() => <SinglePaneShell />);

    expect(await screen.findByText("docs")).toBeInTheDocument();
    expect(document.querySelectorAll(".panel").length).toBe(1);
  });
});
